//! HTTP retry behaviour against a mock source (wiremock):
//! 429 + Retry-After is honored, then success; persistent 5xx gives up after
//! exactly `max_attempts` tries.

use kiwimanga::http::HttpClient;
use kiwimanga::ratelimit::RateLimiter;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client(max_attempts: u32) -> HttpClient {
    HttpClient::new(Duration::from_secs(5), max_attempts).unwrap()
}

fn fast_limiter() -> std::sync::Arc<RateLimiter> {
    RateLimiter::new(1000.0, 1000.0)
}

#[tokio::test]
async fn honors_429_retry_after_then_succeeds() {
    let server = MockServer::start().await;
    // NOTE: wiremock tries the LAST mounted mock first, so the one-shot 429
    // must be mounted after the terminal 200.
    Mock::given(method("GET"))
        .and(path("/flaky"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/flaky"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    let http = client(5);
    let q: Vec<(String, String)> = Vec::new();
    let v: serde_json::Value =
        http.get_json(&format!("{}/flaky", server.uri()), &q, &fast_limiter(), "test").await.unwrap();
    assert_eq!(v["ok"], serde_json::json!(true));

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 2, "expected 1x429 + 1x200");
}

#[tokio::test]
async fn gives_up_after_max_attempts_on_500() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let http = client(3);
    let q: Vec<(String, String)> = Vec::new();
    let res: Result<serde_json::Value, _> =
        http.get_json(&format!("{}/down", server.uri()), &q, &fast_limiter(), "test").await;
    assert!(res.is_err(), "persistent 500 must surface as error");

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 3, "must try exactly max_attempts times");
}

#[tokio::test]
async fn does_not_retry_404() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let http = client(5);
    let q: Vec<(String, String)> = Vec::new();
    let res: Result<serde_json::Value, _> =
        http.get_json(&format!("{}/gone", server.uri()), &q, &fast_limiter(), "test").await;
    assert!(matches!(res, Err(kiwimanga::error::BotError::NotFound(_))));

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1, "404 must not be retried");
}
