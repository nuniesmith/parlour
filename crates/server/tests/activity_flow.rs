//! End-to-end Activity feed against a real Postgres (UI4):
//! sign-in, invite and password events are recorded, the feed
//! pages newest-first and filters by category, only admins can
//! read it, and the purge job removes rows older than 90 days.
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

fn kinds(page: &Value) -> Vec<String> {
    page["entries"]
        .as_array()
        .expect("entries array")
        .iter()
        .map(|e| e["kind"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL"]
async fn activity_flow() {
    let url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test db url");
    let cfg = test_config(&url);
    let pool = parlour_db::connect(&url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");
    parlour_server::auth::bootstrap_admin(&pool, &cfg)
        .await
        .expect("bootstrap");

    let state = AppState::new(pool.clone(), None, false);
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
    let feed = "/api/v1/admin/activity";

    // Anonymous: 401. A failed sign-in records nothing.
    let (status, _, _) = client.req(Method::GET, feed, None, None).await;
    assert_eq!(status, 401);
    let (status, _, _) = client.login("jordan", "wrong-password").await;
    assert_eq!(status, 401);

    let (status, _, cookie) = client.login("jordan", "admin-pass-123").await;
    assert_eq!(status, 200);
    let admin = cookie.expect("admin session cookie");

    let (status, page, _) = client.req(Method::GET, feed, Some(&admin), None).await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(kinds(&page), ["auth.login"], "{page}");
    let first = &page["entries"][0];
    assert_eq!(first["summary"], "jordan signed in");
    assert_eq!(first["actor"], "jordan");
    assert_eq!(first["detail"]["ip"], "127.0.0.1");
    assert!(first["user_id"].is_string());
    assert!(page["next_before"].is_null(), "one entry, no next page");

    // Invite created, revoked; a second invite accepted.
    let (status, invite, _) = client
        .req(Method::POST, "/api/v1/admin/invites", Some(&admin), None)
        .await;
    assert_eq!(status, 201, "{invite}");
    let revoke = format!("/api/v1/admin/invites/{}", invite["id"].as_str().unwrap());
    let (status, _, _) = client
        .req(Method::DELETE, &revoke, Some(&admin), None)
        .await;
    assert_eq!(status, 204);
    let (_, invite, _) = client
        .req(Method::POST, "/api/v1/admin/invites", Some(&admin), None)
        .await;
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

    // Password change.
    let (status, body, _) = client
        .req(
            Method::POST,
            "/api/v1/auth/change-password",
            Some(&admin),
            Some(json!({
                "current_password": "admin-pass-123",
                "new_password": "new-admin-pass-456"
            })),
        )
        .await;
    assert_eq!(status, 204, "{body}");

    // Newest first.
    let (_, page, _) = client.req(Method::GET, feed, Some(&admin), None).await;
    assert_eq!(
        kinds(&page),
        [
            "auth.password_changed",
            "user.joined",
            "invite.created",
            "invite.revoked",
            "invite.created",
            "auth.login",
        ],
        "{page}"
    );
    assert_eq!(page["entries"][1]["summary"], "sam joined with an invite");
    assert_eq!(page["entries"][1]["actor"], "sam");
    let text = page.to_string();
    assert!(
        !text.contains(&token) && !text.contains("pass-"),
        "no invite token or password ever lands in the feed"
    );

    // Paging: 4 + 2, following next_before.
    let (_, p1, _) = client
        .req(Method::GET, &format!("{feed}?limit=4"), Some(&admin), None)
        .await;
    assert_eq!(kinds(&p1).len(), 4);
    let cursor = p1["next_before"].as_str().expect("more pages").to_string();
    let cursor = cursor.replace('+', "%2B");
    let (status, p2, _) = client
        .req(
            Method::GET,
            &format!("{feed}?limit=4&before={cursor}"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, 200, "{p2}");
    assert_eq!(kinds(&p2), ["invite.created", "auth.login"], "{p2}");
    assert!(p2["next_before"].is_null());

    // Category filter.
    let (_, invites, _) = client
        .req(
            Method::GET,
            &format!("{feed}?category=invite"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(
        kinds(&invites),
        ["invite.created", "invite.revoked", "invite.created"]
    );

    // A non-admin cannot tell the route exists (A7), though their
    // own sign-in is recorded.
    let (status, _, cookie) = client.login("sam", "sam-pass-12345").await;
    assert_eq!(status, 200);
    let sam = cookie.expect("sam session cookie");
    let (status, body, _) = client.req(Method::GET, feed, Some(&sam), None).await;
    assert_eq!(status, 404, "{body}");
    let (_, page, _) = client.req(Method::GET, feed, Some(&admin), None).await;
    assert_eq!(page["entries"][0]["summary"], "sam signed in");

    // Retention: the nightly job removes rows past 90 days and
    // reports how many; newer rows stay.
    sqlx::query(
        "INSERT INTO activity (kind, summary, created_at) VALUES
            ('scan.completed', 'ancient scan', now() - interval '91 days'),
            ('scan.completed', 'recent scan', now() - interval '89 days')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let job_id = parlour_jobs::enqueue(
        &pool,
        parlour_jobs::JobKind::PurgeActivity,
        None,
        json!({"trigger": "test"}),
    )
    .await
    .unwrap();
    parlour_server::catalog::spawn_background(
        pool.clone(),
        parlour_server::metadata::MetadataSettings::default(),
        parlour_catalog::EmbySettings::default(),
    );
    let mut finished = None;
    for _ in 0..60 {
        let job = parlour_jobs::get(&pool, job_id).await.unwrap().unwrap();
        if job.status == "succeeded" || job.status == "failed" {
            finished = Some(job);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    let job = finished.expect("purge job finished");
    assert_eq!(job.status, "succeeded");
    assert_eq!(job.result.unwrap()["deleted"], 1);
    let (_, scans, _) = client
        .req(
            Method::GET,
            &format!("{feed}?category=scan"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(scans["entries"].as_array().unwrap().len(), 1, "{scans}");
    assert_eq!(scans["entries"][0]["summary"], "recent scan");
}
