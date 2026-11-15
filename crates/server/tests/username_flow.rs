//! End-to-end change-username flow against a real Postgres (UI10):
//! self-service rename (password-gated) and admin rename.
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

    async fn change_username(
        &self,
        cookie: Option<&str>,
        current: &str,
        new: &str,
    ) -> (reqwest::StatusCode, Value) {
        let (status, body, _) = self
            .req(
                reqwest::Method::POST,
                "/api/v1/auth/change-username",
                cookie,
                Some(json!({"current_password": current, "new_username": new})),
            )
            .await;
        (status, body)
    }

    async fn me_name(&self, cookie: &str) -> (reqwest::StatusCode, Value) {
        let (status, me, _) = self
            .req(reqwest::Method::GET, "/api/v1/auth/me", Some(cookie), None)
            .await;
        (status, me["user"]["username"].clone())
    }
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL"]
async fn change_username_flow() {
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

    // Admin with two sessions; both must survive every rename.
    let (status, _, a) = client.login("jordan", "admin-pass-123").await;
    assert_eq!(status, 200);
    let admin_a = a.expect("admin session a");
    let (_, _, b) = client.login("jordan", "admin-pass-123").await;
    let admin_b = b.expect("admin session b");

    // A second, non-admin account via invite: "sam".
    let (status, invite, _) = client
        .req(
            Method::POST,
            "/api/v1/admin/invites",
            Some(&admin_a),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 201, "{invite}");
    let token = invite["token"].as_str().unwrap().to_string();
    let (status, body, _) = client
        .req(
            Method::POST,
            "/api/v1/invites/accept",
            None,
            Some(json!({"token": token, "username": "sam", "password": "sam-pass-123"})),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    let sam_id = body["user"]["id"].as_str().unwrap().to_string();
    let (_, _, s) = client.login("sam", "sam-pass-123").await;
    let sam = s.expect("sam session");

    // --- Self-service --------------------------------------------------

    let (status, _) = client
        .change_username(None, "admin-pass-123", "jordan2")
        .await;
    assert_eq!(status, 401, "no session, no change");

    // Wrong password is checked first: even a taken name says 401,
    // so the route can't be used to probe which names exist.
    let (status, body) = client
        .change_username(Some(&admin_a), "not-the-password", "SAM")
        .await;
    assert_eq!(status, 401, "{body}");

    let (status, body) = client
        .change_username(Some(&admin_a), "admin-pass-123", "no spaces")
        .await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = client
        .change_username(Some(&admin_a), "admin-pass-123", "ab")
        .await;
    assert_eq!(status, 400, "{body}");

    // Taken by another account in a different case → 409.
    let (status, body) = client
        .change_username(Some(&admin_a), "admin-pass-123", "SAM")
        .await;
    assert_eq!(status, 409, "{body}");

    // Exactly the current name → 400.
    let (status, body) = client
        .change_username(Some(&admin_a), "admin-pass-123", "jordan")
        .await;
    assert_eq!(status, 400, "{body}");

    // Changing only the case of your own name is allowed.
    let (status, body) = client
        .change_username(Some(&admin_a), "admin-pass-123", "Jordan")
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["username"], "Jordan");

    // A real rename (surrounding whitespace trimmed).
    let (status, body) = client
        .change_username(Some(&admin_a), "admin-pass-123", "  jordan.s  ")
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["username"], "jordan.s");
    assert_eq!(body["is_admin"], true);

    // Both sessions survive and see the new name.
    for cookie in [&admin_a, &admin_b] {
        let (status, name) = client.me_name(cookie).await;
        assert_eq!(status, 200, "sessions survive a rename");
        assert_eq!(name, "jordan.s");
    }

    // Old name no longer signs in; new one does, in any case,
    // with the unchanged password.
    let (status, _, _) = client.login("jordan", "admin-pass-123").await;
    assert_eq!(status, 401, "old username is gone");
    let (status, _, c) = client.login("JORDAN.S", "admin-pass-123").await;
    assert_eq!(status, 200);
    assert!(c.is_some());

    // The old name is free again for someone else.
    let (status, body) = client
        .change_username(Some(&sam), "sam-pass-123", "jordan")
        .await;
    assert_eq!(status, 200, "{body}");
    let (status, body) = client
        .change_username(Some(&sam), "sam-pass-123", "sam")
        .await;
    assert_eq!(status, 200, "{body}");

    // --- Admin rename --------------------------------------------------

    let user_path = format!("/api/v1/admin/users/{sam_id}");

    // Non-admins don't see the admin route at all.
    let (status, _, _) = client
        .req(
            Method::PATCH,
            &user_path,
            Some(&sam),
            Some(json!({"username": "x-sam"})),
        )
        .await;
    assert_eq!(status, 404, "admin route is invisible to non-admins");

    // Unknown / malformed ids → 404.
    let (status, _, _) = client
        .req(
            Method::PATCH,
            "/api/v1/admin/users/00000000-0000-0000-0000-000000000000",
            Some(&admin_a),
            Some(json!({"username": "ghost"})),
        )
        .await;
    assert_eq!(status, 404);
    let (status, _, _) = client
        .req(
            Method::PATCH,
            "/api/v1/admin/users/not-a-uuid",
            Some(&admin_a),
            Some(json!({"username": "ghost"})),
        )
        .await;
    assert_eq!(status, 404);

    // Collision with another account, any case → 409; bad name → 400.
    let (status, body, _) = client
        .req(
            Method::PATCH,
            &user_path,
            Some(&admin_a),
            Some(json!({"username": "Jordan.S"})),
        )
        .await;
    assert_eq!(status, 409, "{body}");
    let (status, body, _) = client
        .req(
            Method::PATCH,
            &user_path,
            Some(&admin_a),
            Some(json!({"username": "bad/name"})),
        )
        .await;
    assert_eq!(status, 400, "{body}");

    // The real admin rename — no password needed.
    let (status, body, _) = client
        .req(
            Method::PATCH,
            &user_path,
            Some(&admin_a),
            Some(json!({"username": "samuel"})),
        )
        .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["username"], "samuel");
    assert_eq!(body["id"], sam_id.as_str());
    assert_eq!(body["profile_count"], 1);

    // Sam's existing session survives and sees the new name.
    let (status, name) = client.me_name(&sam).await;
    assert_eq!(status, 200);
    assert_eq!(name, "samuel");

    // Profiles keep their own names.
    let (_, profiles, _) = client
        .req(Method::GET, "/api/v1/profiles", Some(&sam), None)
        .await;
    assert_eq!(profiles[0]["name"], "sam", "{profiles}");

    // The admin user list shows it.
    let (_, users, _) = client
        .req(Method::GET, "/api/v1/admin/users", Some(&admin_a), None)
        .await;
    let names: Vec<&str> = users
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["username"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["jordan.s", "samuel"]);

    // Every rename is in the activity feed; no password is.
    let (_, page, _) = client
        .req(
            Method::GET,
            "/api/v1/admin/activity?category=user",
            Some(&admin_a),
            None,
        )
        .await;
    let summaries: Vec<&str> = page["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "user.renamed")
        .map(|e| e["summary"].as_str().unwrap())
        .collect();
    assert_eq!(
        summaries,
        [
            "jordan.s renamed sam to samuel",
            "jordan changed their username to sam",
            "sam changed their username to jordan",
            "Jordan changed their username to jordan.s",
            "jordan changed their username to Jordan",
        ],
        "{page}"
    );
    assert!(
        !page.to_string().contains("pass-"),
        "no password in the feed"
    );
}
