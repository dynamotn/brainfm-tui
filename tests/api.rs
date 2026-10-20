use std::sync::{Arc, Mutex};

use brainfm::api::{ApiClient, ApiError, EffectLevel, SessionRequest};
use serde_json::json;
use url::Url;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(server: &MockServer) -> ApiClient {
    ApiClient::new(Url::parse(&server.uri()).expect("mock server uri")).with_token("t0")
}

fn ok(result: &serde_json::Value) -> ResponseTemplate {
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
        .respond_with(ok(&json!([{"id": "focus", "displayValue": "Focus"}])))
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
        .respond_with(ok(&json!("jwt")))
        .expect(1)
        .mount(&server)
        .await;
    let c = ApiClient::new(Url::parse(&server.uri()).expect("mock server uri"));
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
        .respond_with(ok(&json!({"servings": [{
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
        .respond_with(ok(&json!([
            "Piano",
            {"name": "Electronic"},
            {"displayValue": "Lofi"}
        ])))
        .mount(&server)
        .await;
    let genres = client(&server).genres("focus").await.unwrap();
    assert_eq!(genres, vec!["Piano", "Electronic", "Lofi"]);
}

#[tokio::test]
async fn null_result_is_decode_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ok(&serde_json::Value::Null))
        .mount(&server)
        .await;
    let err = client(&server).mental_states().await.unwrap_err();
    assert!(matches!(err, ApiError::Decode(_)), "{err:?}");
}

#[tokio::test]
async fn rejects_unsafe_ids() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ok(&json!([])))
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
        .respond_with(ok(&json!({})))
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
            .and(path("/v3/users/u1/tracks/t1/favorites"))
            .respond_with(ok(&json!({})))
            .expect(1)
            .mount(&server)
            .await;
    }
    let c = client(&server);
    c.set_favorite("u1", "t1", true).await.unwrap();
    c.set_favorite("u1", "t1", false).await.unwrap();
}

#[tokio::test]
async fn library_endpoints_use_v3() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v3/users/u1/servings/favorites"))
        .and(query_param("version", "2"))
        .respond_with(ok(&json!([])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v3/users/u1/servings/recent"))
        .and(query_param("platform", "web"))
        .respond_with(ok(&json!([])))
        .expect(1)
        .mount(&server)
        .await;
    let c = client(&server);
    c.favorites("u1").await.unwrap();
    c.recents("u1").await.unwrap();
}

#[tokio::test]
async fn genres_accept_base_and_nature_groups() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v3/mentalStates/dynamic/focus/genres"))
        .respond_with(ok(&json!({
            "baseGenreNames": ["Piano", "Electronic"],
            "natureGenreNames": ["Rain"]
        })))
        .mount(&server)
        .await;
    let genres = client(&server).genres("focus").await.unwrap();
    assert_eq!(genres, vec!["Piano", "Electronic", "Rain"]);
}

#[tokio::test]
async fn library_accepts_wrapped_servings() {
    let server = MockServer::start().await;
    let serving = json!({
        "track": {"id": "t1", "name": "Lumen", "tags": []},
        "trackVariation": {"id": "v1", "url": "a.mp3", "lengthInSeconds": 10}
    });
    Mock::given(method("GET"))
        .and(path("/v3/users/u1/servings/favorites"))
        .respond_with(ok(&json!([
            {"serving": serving, "genreName": "Piano"},
            serving
        ])))
        .mount(&server)
        .await;
    let favs = client(&server).favorites("u1").await.unwrap();
    assert_eq!(favs.len(), 2);
    assert!(favs.iter().all(|s| s.track.name == "Lumen"));
}

#[tokio::test]
async fn network_errors_show_their_cause() {
    // Nothing listens on port 9 (discard) on loopback in the test environment.
    let c = ApiClient::new(Url::parse("http://127.0.0.1:9").unwrap());
    let err = c.mental_states().await.unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("refused") || text.contains("connect"),
        "{text}"
    );
    assert!(text.contains(':'), "cause chain missing: {text}");
    assert!(
        text.matches(": ").count() >= 2,
        "cause chain missing: {text}"
    );
}

#[tokio::test]
async fn unchanged_token_does_not_notify() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"status": 200, "result": [], "token": "t0"})),
        )
        .mount(&server)
        .await;
    let calls = Arc::new(Mutex::new(0u32));
    let sink = calls.clone();
    let c = client(&server).on_token_refresh(move |_| *sink.lock().unwrap() += 1);
    c.mental_states().await.unwrap();
    assert_eq!(*calls.lock().unwrap(), 0);
}
