//! HTTP client for `api.brain.fm`, mimicking the requests the web app sends.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::header::{self, HeaderMap, HeaderValue};
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use url::Url;

use super::models::{Activity, MentalState, Serving, SessionRequest, User};

pub const DEFAULT_BASE: &str = "https://api.brain.fm";
const ORIGIN: &str = "https://my.brain.fm";
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0";

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("session expired or not authorized")]
    Unauthorized,
    #[error("rate limited by Brain.fm, try again shortly")]
    RateLimited,
    #[error("Brain.fm returned {code}: {message}")]
    Status { code: u16, message: String },
    #[error("refusing to use unsafe id {0:?}")]
    InvalidId(String),
    #[error("network error: {}", with_causes(.0))]
    Network(#[from] reqwest::Error),
    #[error("unexpected response: {0}")]
    Decode(String),
}

pub type ApiResult<T> = Result<T, ApiError>;

/// True when `e`, or anything it wraps, is an expired or missing session.
pub fn is_unauthorized(e: &anyhow::Error) -> bool {
    e.chain()
        .any(|c| matches!(c.downcast_ref::<ApiError>(), Some(ApiError::Unauthorized)))
}

/// reqwest's own message hides the cause (timeout, reset, TLS…); append it.
fn with_causes(e: &reqwest::Error) -> String {
    let mut out = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(cause) = source {
        out.push_str(": ");
        out.push_str(&cause.to_string());
        source = cause.source();
    }
    out
}

type TokenCallback = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Clone)]
pub struct ApiClient {
    http: reqwest::Client,
    base: Url,
    token: Arc<Mutex<Option<String>>>,
    on_token: Option<TokenCallback>,
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    messages: Option<Vec<String>>,
    #[serde(default)]
    token: Option<String>,
}

#[derive(Deserialize)]
struct SessionResult {
    servings: Vec<Serving>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum GenreItem {
    Name(String),
    Object {
        #[serde(default)]
        name: Option<String>,
        #[serde(default, rename = "displayValue")]
        display_value: Option<String>,
    },
}

/// Library lists hold either servings or objects wrapping one.
#[derive(Deserialize)]
#[serde(untagged)]
enum LibraryItem {
    Serving(Serving),
    Wrapped { serving: Serving },
}

fn unwrap_library(items: Vec<LibraryItem>) -> Vec<Serving> {
    items
        .into_iter()
        .map(|i| match i {
            LibraryItem::Serving(s) | LibraryItem::Wrapped { serving: s } => s,
        })
        .collect()
}

/// The live API groups genres; older shapes were a flat list.
#[derive(Deserialize)]
#[serde(untagged)]
enum GenreList {
    Grouped {
        #[serde(default, rename = "baseGenreNames")]
        base: Vec<GenreItem>,
        #[serde(default, rename = "natureGenreNames")]
        nature: Vec<GenreItem>,
    },
    Flat(Vec<GenreItem>),
}

fn safe_id(id: &str) -> ApiResult<&str> {
    let ok = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(id)
    } else {
        Err(ApiError::InvalidId(id.to_owned()))
    }
}

impl ApiClient {
    pub fn new(base: Url) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, HeaderValue::from_static(ORIGIN));
        headers.insert(
            header::REFERER,
            HeaderValue::from_static("https://my.brain.fm/"),
        );
        headers.insert(header::ACCEPT, HeaderValue::from_static("*/*"));
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .expect("static reqwest config is valid");
        Self {
            http,
            base,
            token: Arc::new(Mutex::new(None)),
            on_token: None,
        }
    }

    pub fn with_token(self, token: String) -> Self {
        self.set_token(&token);
        self
    }

    /// Called with every new token the API slides forward, so it can be persisted.
    pub fn on_token_refresh(mut self, f: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.on_token = Some(Arc::new(f));
        self
    }

    pub fn token(&self) -> Option<String> {
        self.token.lock().expect("token lock").clone()
    }

    fn set_token(&self, token: &str) {
        *self.token.lock().expect("token lock") = Some(token.to_owned());
    }

    fn url(&self, version: u8, path: &str) -> ApiResult<Url> {
        self.base
            .join(&format!("v{version}{path}"))
            .map_err(|e| ApiError::Decode(format!("bad url: {e}")))
    }

    /// Sends a request, maps error statuses, and slides the token forward.
    async fn send(
        &self,
        method: Method,
        version: u8,
        path: &str,
        body: Option<Value>,
    ) -> ApiResult<Option<Value>> {
        let mut req = self.http.request(method, self.url(version, path)?);
        if let Some(t) = self.token() {
            req = req.bearer_auth(t);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        let env: Option<Envelope> = serde_json::from_str(&text).ok();

        match status {
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => return Err(ApiError::Unauthorized),
            StatusCode::TOO_MANY_REQUESTS => return Err(ApiError::RateLimited),
            s if !s.is_success() => {
                let message = env
                    .and_then(|e| e.messages)
                    .map(|m| m.join("; "))
                    .unwrap_or_else(|| s.canonical_reason().unwrap_or("error").to_owned());
                return Err(ApiError::Status {
                    code: s.as_u16(),
                    message,
                });
            }
            _ => {}
        }

        let env = env.ok_or_else(|| ApiError::Decode("response is not JSON".into()))?;
        if let Some(t) = env.token.as_deref().filter(|t| !t.is_empty())
            && self.token().as_deref() != Some(t)
        {
            self.set_token(t);
            if let Some(cb) = &self.on_token {
                cb(t);
            }
        }
        Ok(env.result.filter(|v| !v.is_null()))
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        version: u8,
        path: &str,
        body: Option<Value>,
    ) -> ApiResult<T> {
        let v = self
            .send(method, version, path, body)
            .await?
            .ok_or_else(|| ApiError::Decode("no result".into()))?;
        serde_json::from_value(v).map_err(|e| ApiError::Decode(e.to_string()))
    }

    async fn get<T: DeserializeOwned>(&self, version: u8, path: &str) -> ApiResult<T> {
        self.request(Method::GET, version, path, None).await
    }

    /// Logs in and keeps the returned token.
    pub async fn email_login(&self, email: &str, password: &str) -> ApiResult<String> {
        let body = json!({ "email": email, "password": password });
        let token: String = self
            .request(Method::POST, 2, "/auth/email-login", Some(body))
            .await?;
        self.set_token(&token);
        Ok(token)
    }

    pub async fn me(&self) -> ApiResult<User> {
        self.get(2, "/users/me").await
    }

    pub async fn mental_states(&self) -> ApiResult<Vec<MentalState>> {
        self.get(3, "/mentalStates/dynamic").await
    }

    pub async fn activities(&self, state_id: &str) -> ApiResult<Vec<Activity>> {
        let id = safe_id(state_id)?;
        self.get(3, &format!("/mentalStates/dynamic/{id}/activities"))
            .await
    }

    pub async fn genres(&self, state_id: &str) -> ApiResult<Vec<String>> {
        let id = safe_id(state_id)?;
        let list: GenreList = self
            .get(3, &format!("/mentalStates/dynamic/{id}/genres"))
            .await?;
        let items = match list {
            GenreList::Grouped { base, nature } => base.into_iter().chain(nature).collect(),
            GenreList::Flat(items) => items,
        };
        Ok(items
            .into_iter()
            .filter_map(|g| match g {
                GenreItem::Name(n) => Some(n),
                GenreItem::Object {
                    name,
                    display_value,
                } => name.or(display_value),
            })
            .collect())
    }

    pub async fn start_session(&self, uid: &str, req: &SessionRequest) -> ApiResult<Vec<Serving>> {
        let uid = safe_id(uid)?;
        let body = serde_json::to_value(req).map_err(|e| ApiError::Decode(e.to_string()))?;
        let res: SessionResult = self
            .request(
                Method::POST,
                3,
                &format!("/users/{uid}/sessions?platform=web"),
                Some(body),
            )
            .await?;
        Ok(res.servings)
    }

    pub async fn favorites(&self, uid: &str) -> ApiResult<Vec<Serving>> {
        let uid = safe_id(uid)?;
        let items = self
            .get(3, &format!("/users/{uid}/servings/favorites?version=2"))
            .await?;
        Ok(unwrap_library(items))
    }

    pub async fn recents(&self, uid: &str) -> ApiResult<Vec<Serving>> {
        let uid = safe_id(uid)?;
        let items = self
            .get(3, &format!("/users/{uid}/servings/recent?platform=web"))
            .await?;
        Ok(unwrap_library(items))
    }

    pub async fn set_favorite(&self, uid: &str, track_id: &str, on: bool) -> ApiResult<()> {
        let (uid, tid) = (safe_id(uid)?, safe_id(track_id)?);
        let method = if on { Method::POST } else { Method::DELETE };
        self.send(
            method,
            3,
            &format!("/users/{uid}/tracks/{tid}/favorites"),
            None,
        )
        .await?;
        Ok(())
    }

    pub async fn dislike(&self, uid: &str, track_id: &str) -> ApiResult<()> {
        let uid = safe_id(uid)?;
        let body = json!({ "trackId": track_id });
        self.send(
            Method::POST,
            2,
            &format!("/users/{uid}/dislikes?version=3"),
            Some(body),
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_unauthorized_sees_through_anyhow_context() {
        use anyhow::Context;
        let e: anyhow::Result<()> = Err(ApiError::Unauthorized).context("loading states");
        assert!(is_unauthorized(&e.unwrap_err()));
        assert!(!is_unauthorized(&anyhow::anyhow!("other")));
    }
}
