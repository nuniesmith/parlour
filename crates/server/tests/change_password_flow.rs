//! End-to-end change-password flow against a real Postgres (A8).
//!
//! Ignored by default: set PARLOUR_TEST_DATABASE_URL to a
//! throwaway database and run `cargo test -- --ignored`.
//! The database is migrated fresh by the server code itself.

use std::net::SocketAddr;

use parlour_server::config::Config;
use parlour_server::http::AppState;
use serde_json::{json, Value};

fn test_config(url: &str) -> Config {
    let mut vars = std::collections::HashMap::new();
    vars.insert("PARLOUR_DATABASE_URL".to_string(), url.to_string());
    vars.insert("PARLOUR_ADMIN_USER".to_string(), "jordan".to_string());
    vars.insert(
        "PARLOUR_ADMIN_PASSWORD".to_string(),
        "admin-pass-123".to_string(),
    );
    Config::from_vars(&vars).expect("config")
}

struct Client {
    http: reqwest::Client,
    base: String,
}

impl Client {
    async fn req(
        &self,
        method: reqwest::Method,
        path: &str,
        cookie: Option<&str>,
        body: Option<Value>,
    ) -> (reqwest::StatusCode, Value, Option<String>) {
        let mut rb = self.http.request(method, format!("{}{path}", self.base));
        if let Some(c) = cookie {
            rb = rb.header("Cookie", format!("parlour_session={c}"));
        }
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let res = rb.send().await.expect("request");
        let status = res.status();
        let set_cookie = res
            .headers()
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| {
                s.split(';')
                    .next()
                    .and_then(|kv| kv.split_once('='))
                    .map(|(_, v)| v.to_string())
            });
        let text = res.text().await.unwrap_or_default();
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        (status, value, set_cookie)
    }

    async fn login(
        &self,
        username: &str,
        password: &str,
    ) -> (reqwest::StatusCode, Value, Option<String>) {
        self.req(
            reqwest::Method::POST,
            "/api/v1/auth/login",
            None,
            Some(json!({"username": username, "password": password})),
        )
        .await
    }

    async fn change_password(
        &self,
        cookie: Option<&str>,
        current: &str,
        new: &str,
    ) -> (reqwest::StatusCode, Value) {
        let (status, body, _) = self
            .req(
                reqwest::Method::POST,
                "/api/v1/auth/change-password",
                cookie,
                Some(json!({"current_password": current, "new_password": new})),
            )
            .await;
        (status, body)
    }
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL"]
async fn change_password_flow() {
    let url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test db url");
    let cfg = test_config(&url);
    let pool = parlour_db::connect(&url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");
    parlour_server::auth::bootstrap_admin(&pool, &cfg)
        .await
        .expect("bootstrap");

    let state = AppState::new(pool, None, false);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            parlour_server::http::router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = Client {
        http: reqwest::Client::new(),
        base: format!("http://{addr}"),
    };
    use reqwest::Method;

    // Two sessions exist BEFORE the change: the one that will make
    // the change, and a second one that must be revoked by it.
    let (status, _, cookie_a) = client.login("jordan", "admin-pass-123").await;
    assert_eq!(status, 200);
    let session_a = cookie_a.expect("changing session cookie");
    let (status, _, cookie_b) = client.login("jordan", "admin-pass-123").await;
    assert_eq!(status, 200);
    let session_b = cookie_b.expect("second session cookie");
    let (status, _, _) = client
        .req(Method::GET, "/api/v1/auth/me", Some(&session_b), None)
        .await;
    assert_eq!(status, 200, "second session is live before the change");

    // Unauthenticated change is refused.
    let (status, _) = client
        .change_password(None, "admin-pass-123", "new-admin-pass-456")
        .await;
    assert_eq!(status, 401, "no session, no change");

    // Wrong current password → 401, nothing changes.
    let (status, body) = client
        .change_password(Some(&session_a), "not-the-password", "new-admin-pass-456")
        .await;
    assert_eq!(status, 401, "{body}");

    // Too-short new password → 400.
    let (status, body) = client
        .change_password(Some(&session_a), "admin-pass-123", "short")
        .await;
    assert_eq!(status, 400, "{body}");

    // New password equal to the current one → 400.
    let (status, body) = client
        .change_password(Some(&session_a), "admin-pass-123", "admin-pass-123")
        .await;
    assert_eq!(status, 400, "{body}");

    // The real change → 204.
    let (status, body) = client
        .change_password(Some(&session_a), "admin-pass-123", "new-admin-pass-456")
        .await;
    assert_eq!(status, 204, "{body}");

    // The changing session is still valid.
    let (status, me, _) = client
        .req(Method::GET, "/api/v1/auth/me", Some(&session_a), None)
        .await;
    assert_eq!(status, 200, "{me}");
    assert_eq!(me["user"]["username"], "jordan");

    // The second session, created before the change, is revoked.
    let (status, _, _) = client
        .req(Method::GET, "/api/v1/auth/me", Some(&session_b), None)
        .await;
    assert_eq!(
        status, 401,
        "other sessions are revoked by a password change"
    );

    // The old password no longer logs in; the new one does.
    let (status, _, _) = client.login("jordan", "admin-pass-123").await;
    assert_eq!(status, 401, "old password is dead");
    let (status, body, cookie_c) = client.login("jordan", "new-admin-pass-456").await;
    assert_eq!(status, 200, "{body}");
    assert!(cookie_c.is_some(), "new password mints a session");
}
