//! End-to-end per-profile preferences flow against a real Postgres (UI7).
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
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL"]
async fn preferences_flow() {
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

    let (status, body, cookie) = client.login("jordan", "admin-pass-123").await;
    assert_eq!(status, 200, "{body}");
    let cookie = cookie.expect("session cookie");
    let c = Some(cookie.as_str());
    let profile = body["profiles"][0]["id"].as_str().unwrap().to_string();
    let path = format!("/api/v1/profiles/{profile}");

    // Defaults: every rail visible in default order, skip button
    // shown, autoplay on (D15).
    let p = &body["profiles"][0];
    assert_eq!(
        p["home_rails"],
        json!([
            {"id": "libraries", "visible": true},
            {"id": "continue_watching", "visible": true},
            {"id": "next_up", "visible": true},
            {"id": "recently_added", "visible": true},
            {"id": "latest", "visible": true},
        ])
    );
    assert_eq!(p["segment_skip"], "button");
    assert_eq!(p["autoplay_next"], true);

    // Reorder + hide one rail, change both playback prefs.
    let (status, body, _) = client
        .req(
            Method::PATCH,
            &path,
            c,
            Some(json!({
                "home_rails": [
                    {"id": "recently_added", "visible": true},
                    {"id": "continue_watching", "visible": false},
                    {"id": "next_up", "visible": true},
                ],
                "segment_skip": "auto",
                "autoplay_next": false,
            })),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body["home_rails"],
        json!([
            {"id": "recently_added", "visible": true},
            {"id": "continue_watching", "visible": false},
            {"id": "next_up", "visible": true},
            {"id": "libraries", "visible": true},
            {"id": "latest", "visible": true},
        ])
    );
    assert_eq!(body["segment_skip"], "auto");
    assert_eq!(body["autoplay_next"], false);

    // A PATCH that doesn't mention them leaves them alone.
    let (status, body, _) = client
        .req(
            Method::PATCH,
            &path,
            c,
            Some(json!({"subtitle_mode": "always", "subtitle_lang": "en"})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["subtitle_mode"], "always");
    assert_eq!(body["home_rails"][0]["id"], "recently_added");
    assert_eq!(body["home_rails"][1]["visible"], false);
    assert_eq!(body["segment_skip"], "auto");
    assert_eq!(body["autoplay_next"], false);

    // Clearing a language goes back to "no preference".
    let (status, body, _) = client
        .req(Method::PATCH, &path, c, Some(json!({"audio_lang": "ja"})))
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["audio_lang"], "ja");
    let (status, body, _) = client
        .req(
            Method::PATCH,
            &path,
            c,
            Some(json!({"clear_subtitle_lang": true, "clear_audio_lang": true})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["subtitle_lang"], Value::Null);
    assert_eq!(body["audio_lang"], Value::Null);
    assert_eq!(body["subtitle_mode"], "always");

    // A partial rail list: the rest follow in default order, visible.
    let (status, body, _) = client
        .req(
            Method::PATCH,
            &path,
            c,
            Some(json!({"home_rails": [{"id": "next_up", "visible": false}]})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body["home_rails"],
        json!([
            {"id": "next_up", "visible": false},
            {"id": "libraries", "visible": true},
            {"id": "continue_watching", "visible": true},
            {"id": "recently_added", "visible": true},
            {"id": "latest", "visible": true},
        ])
    );

    // Bad input is a 400 and changes nothing.
    for bad in [
        json!({"segment_skip": "always"}),
        json!({"home_rails": [{"id": "live_tv", "visible": true}]}),
        json!({"home_rails": [
            {"id": "next_up", "visible": true},
            {"id": "next_up", "visible": false},
        ]}),
    ] {
        let (status, body, _) = client.req(Method::PATCH, &path, c, Some(bad)).await;
        assert_eq!(status, 400, "{body}");
    }

    // The active profile in /auth/me carries the same prefs once selected.
    let (status, _, _) = client
        .req(Method::POST, &format!("{path}/select"), c, Some(json!({})))
        .await;
    assert_eq!(status, 200);
    let (status, me, _) = client.req(Method::GET, "/api/v1/auth/me", c, None).await;
    assert_eq!(status, 200, "{me}");
    let active = &me["active_profile"];
    assert_eq!(active["home_rails"][0]["id"], "next_up");
    assert_eq!(active["home_rails"][0]["visible"], false);
    assert_eq!(active["segment_skip"], "auto");
    assert_eq!(active["autoplay_next"], false);

    // Signed out is 401; a profile that is not yours (or does not
    // exist) is a 404, never a 403.
    let (status, _, _) = client.req(Method::GET, "/api/v1/auth/me", None, None).await;
    assert_eq!(status, 401);
    let (status, _, _) = client
        .req(
            Method::PATCH,
            &format!("/api/v1/profiles/{}", uuid::Uuid::new_v4()),
            c,
            Some(json!({"autoplay_next": true})),
        )
        .await;
    assert_eq!(status, 404);
}
