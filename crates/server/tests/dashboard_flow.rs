//! End-to-end Server Dashboard payload against a real Postgres (UI2).
//!
//! Ignored by default: set PARLOUR_TEST_DATABASE_URL to a
//! throwaway database and run `cargo test -- --ignored`.
//! The database is migrated fresh by the server code itself.

use std::net::SocketAddr;

use parlour_server::config::Config;
use parlour_server::dashboard::ServerSettings;
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
    vars.insert("PARLOUR_PORT".to_string(), "8585".to_string());
    vars.insert("PARLOUR_SERVER_NAME".to_string(), "test-box".to_string());
    vars.insert(
        "PARLOUR_URL_LAN".to_string(),
        "http://192.168.1.20:8585".to_string(),
    );
    vars.insert(
        "PARLOUR_URL_TAILNET".to_string(),
        "https://parlour.example.ts.net".to_string(),
    );
    // PARLOUR_URL_PUBLIC deliberately unset: it must come back null.
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
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL"]
async fn dashboard_flow() {
    let url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test db url");
    let cfg = test_config(&url);
    let pool = parlour_db::connect(&url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");
    parlour_server::auth::bootstrap_admin(&pool, &cfg)
        .await
        .expect("bootstrap");

    // Job-queue fixture: one queued, one running, one failed, one
    // succeeded, and one failure too old to count as recent.
    use parlour_jobs::JobKind;
    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(
            parlour_jobs::enqueue(&pool, JobKind::ScanLibrary, None, json!({}))
                .await
                .expect("enqueue"),
        );
    }
    assert!(parlour_jobs::claim_by_id(&pool, ids[1]).await.unwrap());
    parlour_jobs::fail(&pool, ids[2], "boom").await.unwrap();
    parlour_jobs::complete(&pool, ids[3]).await.unwrap();
    parlour_jobs::fail(&pool, ids[4], "old boom").await.unwrap();
    sqlx::query("UPDATE jobs SET finished_at = now() - interval '3 days' WHERE id = $1")
        .bind(ids[4])
        .execute(&pool)
        .await
        .unwrap();

    let mut state = AppState::new(pool, None, false);
    state.server = ServerSettings::from_config(&cfg);
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

    // Anonymous: 401.
    let (status, _, _) = client
        .req(Method::GET, "/api/v1/admin/server", None, None)
        .await;
    assert_eq!(status, 401);

    let (status, _, cookie) = client.login("jordan", "admin-pass-123").await;
    assert_eq!(status, 200);
    let admin = cookie.expect("admin session cookie");

    // Admin: the configured facts, verbatim.
    let (status, info, _) = client
        .req(Method::GET, "/api/v1/admin/server", Some(&admin), None)
        .await;
    assert_eq!(status, 200, "{info}");
    assert_eq!(info["name"], "test-box");
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(info["port"], 8585);
    assert_eq!(info["db"], "ok");
    assert_eq!(info["urls"]["lan"], "http://192.168.1.20:8585");
    assert_eq!(info["urls"]["tailnet"], "https://parlour.example.ts.net");
    assert!(
        info["urls"]["public"].is_null(),
        "an unset URL is null, never guessed: {info}"
    );
    assert_eq!(info["jobs"]["queued"], 1, "{info}");
    assert_eq!(info["jobs"]["running"], 1, "{info}");
    assert_eq!(info["jobs"]["failed_recent"], 1, "{info}");
    assert_eq!(info["jobs"]["succeeded_recent"], 1, "{info}");
    assert_eq!(info["actions"]["restart"], false);
    assert_eq!(info["actions"]["shutdown"], false);

    // A non-admin cannot tell the route exists (A7: 404, not 403).
    let (status, invite, _) = client
        .req(Method::POST, "/api/v1/admin/invites", Some(&admin), None)
        .await;
    assert_eq!(status, 201, "{invite}");
    let token = invite["token"].as_str().unwrap().to_string();
    let (status, body, _) = client
        .req(
            Method::POST,
            "/api/v1/invites/accept",
            None,
            Some(json!({"token": token, "username": "sam", "password": "sam-pass-12345"})),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    let (status, _, cookie) = client.login("sam", "sam-pass-12345").await;
    assert_eq!(status, 200);
    let sam = cookie.expect("sam session cookie");
    let (status, body, _) = client
        .req(Method::GET, "/api/v1/admin/server", Some(&sam), None)
        .await;
    assert_eq!(status, 404, "{body}");
}
