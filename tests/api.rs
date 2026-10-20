use std::sync::{Arc, Mutex};

use brainfm::api::{ApiClient, ApiError, EffectLevel, SessionRequest};
use serde_json::json;
use url::Url;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(server: &MockServer) -> ApiClient {
    ApiClient::new(Url::parse(&server.uri()).unwrap()).with_token("t0".into())
}

fn ok(result: serde_json::Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({"status": 200, "result": result}))
}

#[tokio::test]
async fn sends_browser_headers_and_bearer() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v3/mentalStates/dynamic"))
        .and(header("origin", "https://my.brain.fm"))
        .and(header("referer", "https://my.brain.fm/"))
        .and(header("authorization", "Bearer t0"))
        .respond_with(ok(json!([{"id": "focus", "displayValue": "Focus"}])))
        .expect(1)
        .mount(&server)
        .await;

    let states = client(&server).mental_states().await.unwrap();
    assert_eq!(states[0].display_value, "Focus");
}

#[tokio::test]
async fn sliding_token_replaces_and_notifies() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v3/mentalStates/dynamic"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"status": 200, "result": [], "token": "t1"})),
        )
        .mount(&server)
        .await;

    let seen = Arc::new(Mutex::new(None::<String>));
    let sink = seen.clone();
    let c = client(&server).on_token_refresh(move |t| *sink.lock().unwrap() = Some(t.into()));
    c.mental_states().await.unwrap();
    assert_eq!(c.token().as_deref(), Some("t1"));
    assert_eq!(seen.lock().unwrap().as_deref(), Some("t1"));
}

#[tokio::test]
async fn maps_401_to_unauthorized() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({"status": 401})))
        .mount(&server)
        .await;
    let err = client(&server).mental_states().await.unwrap_err();
    assert!(matches!(err, ApiError::Unauthorized), "{err:?}");
}

#[tokio::test]
async fn maps_429_to_rate_limited() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;
    let err = client(&server).mental_states().await.unwrap_err();
    assert!(matches!(err, ApiError::RateLimited), "{err:?}");
}

#[tokio::test]
async fn other_errors_carry_messages() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(500)
                .set_body_json(json!({"status": 500, "messages": ["boom", "bang"]})),
        )
        .mount(&server)
        .await;
    match client(&server).mental_states().await.unwrap_err() {
        ApiError::Status { code, message } => {
            assert_eq!(code, 500);
            assert_eq!(message, "boom; bang");
        }
        e => panic!("unexpected {e:?}"),
    }
}

#[tokio::test]
async fn login_posts_email_and_password() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/auth/email-login"))
        .and(body_json(json!({"email": "a@b.c", "password": "pw"})))
        .respond_with(ok(json!("jwt")))
        .expect(1)
        .mount(&server)
        .await;
    let c = ApiClient::new(Url::parse(&server.uri()).unwrap());
    assert_eq!(c.email_login("a@b.c", "pw").await.unwrap(), "jwt");
    assert_eq!(c.token().as_deref(), Some("jwt"));
}

#[tokio::test]
async fn start_session_posts_body_and_returns_servings() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v3/users/u1/sessions"))
        .and(query_param("platform", "web"))
        .and(body_json(json!({
            "dynamicActivityId": "a1", "version": 3, "neuralEffectLevels": ["High"]
        })))
        .respond_with(ok(json!({"servings": [{
            "track": {"id": "t1", "name": "Ascent", "tags": []},
            "trackVariation": {"id": "v1", "tokenedUrl": "https://cdn/x.mp3",
                               "lengthInSeconds": 300}
        }]})))
        .expect(1)
        .mount(&server)
        .await;
    let req = SessionRequest::new("a1", vec![EffectLevel::High], vec![]);
    let servings = client(&server).start_session("u1", &req).await.unwrap();
    assert_eq!(servings[0].track.name, "Ascent");
}

#[tokio::test]
async fn genres_accept_strings_or_objects() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v3/mentalStates/dynamic/focus/genres"))
        .respond_with(ok(
            json!(["Piano", {"name": "Electronic"}, {"displayValue": "Lofi"}]),
        ))
        .mount(&server)
        .await;
    let genres = client(&server).genres("focus").await.unwrap();
    assert_eq!(genres, vec!["Piano", "Electronic", "Lofi"]);
}

#[tokio::test]
async fn null_result_is_decode_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ok(serde_json::Value::Null))
        .mount(&server)
        .await;
    let err = client(&server).mental_states().await.unwrap_err();
    assert!(matches!(err, ApiError::Decode(_)), "{err:?}");
}

#[tokio::test]
async fn rejects_unsafe_ids() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ok(json!([])))
        .expect(0)
        .mount(&server)
        .await;
    let err = client(&server).activities("../x").await.unwrap_err();
    assert!(matches!(err, ApiError::InvalidId(_)), "{err:?}");
}

#[tokio::test]
async fn dislike_posts_track_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v2/users/u1/dislikes"))
        .and(query_param("version", "3"))
        .and(body_json(json!({"trackId": "t1"})))
        .respond_with(ok(json!({})))
        .expect(1)
        .mount(&server)
        .await;
    client(&server).dislike("u1", "t1").await.unwrap();
}

#[tokio::test]
async fn set_favorite_uses_post_and_delete() {
    let server = MockServer::start().await;
    for m in ["POST", "DELETE"] {
        Mock::given(method(m))
            .and(path("/v2/users/u1/tracks/t1/favorites"))
            .respond_with(ok(json!({})))
            .expect(1)
            .mount(&server)
            .await;
    }
    let c = client(&server);
    c.set_favorite("u1", "t1", true).await.unwrap();
    c.set_favorite("u1", "t1", false).await.unwrap();
}
