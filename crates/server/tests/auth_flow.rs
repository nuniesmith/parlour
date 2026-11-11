//! End-to-end auth flow against a real Postgres (A1–A7).
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
async fn full_auth_flow() {
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

    // Admin logs in (bootstrap account), anonymous /me is 401.
    let (status, _, _) = client.req(Method::GET, "/api/v1/auth/me", None, None).await;
    assert_eq!(status, 401);
    let (status, body, admin_cookie) = client.login("jordan", "admin-pass-123").await;
    assert_eq!(status, 200, "{body}");
    let admin = admin_cookie.expect("session cookie");
    assert_eq!(body["user"]["is_admin"], true);
    let admin_profile = body["profiles"][0]["id"].as_str().unwrap().to_string();

    // A5: profile with a PIN; selection enforces it.
    let (status, kayla, _) = client
        .req(
            Method::POST,
            "/api/v1/profiles",
            Some(&admin),
            Some(json!({"name": "Kayla", "pin": "1234", "avatar_color": "#3f8f5f"})),
        )
        .await;
    assert_eq!(status, 201, "{kayla}");
    let kayla_id = kayla["id"].as_str().unwrap().to_string();
    assert_eq!(kayla["has_pin"], true);
    assert_eq!(kayla["initial"], "K");
    let (status, _, _) = client
        .req(
            Method::POST,
            &format!("/api/v1/profiles/{kayla_id}/select"),
            Some(&admin),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 401, "PIN required");
    let (status, _, _) = client
        .req(
            Method::POST,
            &format!("/api/v1/profiles/{kayla_id}/select"),
            Some(&admin),
            Some(json!({"pin": "9999"})),
        )
        .await;
    assert_eq!(status, 401, "wrong PIN rejected");
    let (status, _, _) = client
        .req(
            Method::POST,
            &format!("/api/v1/profiles/{kayla_id}/select"),
            Some(&admin),
            Some(json!({"pin": "1234"})),
        )
        .await;
    assert_eq!(status, 200, "right PIN selects");

    // A6: two libraries; Kayla's profile gets one. The other is a
    // 404 for her profile even by direct URL.
    let (status, movies, _) = client
        .req(
            Method::POST,
            "/api/v1/admin/libraries",
            Some(&admin),
            Some(json!({"name": "Movies", "kind": "movies"})),
        )
        .await;
    assert_eq!(status, 201, "{movies}");
    let (status, private_lib, _) = client
        .req(
            Method::POST,
            "/api/v1/admin/libraries",
            Some(&admin),
            Some(json!({"name": "Private", "kind": "movies"})),
        )
        .await;
    assert_eq!(status, 201);
    let private_id = private_lib["id"].as_str().unwrap().to_string();
    let (status, _, _) = client
        .req(
            Method::POST,
            &format!(
                "/api/v1/admin/libraries/{}/access",
                movies["id"].as_str().unwrap()
            ),
            Some(&admin),
            Some(json!({"profile_id": kayla_id})),
        )
        .await;
    assert_eq!(status, 204);
    let (status, libs, _) = client
        .req(Method::GET, "/api/v1/libraries", Some(&admin), None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(libs.as_array().unwrap().len(), 1, "{libs}");
    assert_eq!(libs[0]["name"], "Movies");
    let (status, _, _) = client
        .req(
            Method::GET,
            &format!("/api/v1/libraries/{private_id}"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, 404, "inaccessible library is a 404, never a 403");

    // Back to the admin's own profile for the rest.
    let (status, _, _) = client
        .req(
            Method::POST,
            &format!("/api/v1/profiles/{admin_profile}/select"),
            Some(&admin),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 200);

    // A4: invite a friend; no open registration exists.
    let (status, _, _) = client
        .req(
            Method::POST,
            "/api/v1/auth/register",
            None,
            Some(json!({"username": "mallory", "password": "mallory-pass-1"})),
        )
        .await;
    assert_eq!(status, 404, "no open registration route");
    let (status, invite, _) = client
        .req(Method::POST, "/api/v1/admin/invites", Some(&admin), None)
        .await;
    assert_eq!(status, 201, "{invite}");
    let token = invite["token"].as_str().unwrap().to_string();
    let (status, check, _) = client
        .req(Method::GET, &format!("/api/v1/invites/{token}"), None, None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(check["valid"], true);
    let (status, accepted, _) = client
        .req(
            Method::POST,
            "/api/v1/invites/accept",
            None,
            Some(json!({"token": token, "username": "sam", "password": "sam-pass-1234"})),
        )
        .await;
    assert_eq!(status, 201, "{accepted}");
    // The token is single-use; status flips and re-accept fails.
    let (status, check, _) = client
        .req(Method::GET, &format!("/api/v1/invites/{token}"), None, None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(check["valid"], false);
    let (status, _, _) = client
        .req(
            Method::POST,
            "/api/v1/invites/accept",
            None,
            Some(json!({"token": token, "username": "sam2", "password": "sam2-pass-123"})),
        )
        .await;
    assert_eq!(status, 400);

    // The friend signs in; admin routes are 404 for them (A7),
    // and they see no libraries even with a profile selected.
    let (status, sam_body, sam_cookie) = client.login("sam", "sam-pass-1234").await;
    assert_eq!(status, 200, "{sam_body}");
    let sam = sam_cookie.unwrap();
    assert_eq!(sam_body["user"]["is_admin"], false);
    let (status, _, _) = client
        .req(Method::POST, "/api/v1/admin/invites", Some(&sam), None)
        .await;
    assert_eq!(status, 404, "admin surface hidden from non-admins");
    let sam_profile = sam_body["profiles"][0]["id"].as_str().unwrap().to_string();
    let (status, _, _) = client
        .req(
            Method::POST,
            &format!("/api/v1/profiles/{sam_profile}/select"),
            Some(&sam),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 200);
    let (status, libs, _) = client
        .req(Method::GET, "/api/v1/libraries", Some(&sam), None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        libs.as_array().unwrap().len(),
        0,
        "friend sees no libraries"
    );
    // Someone else's profile is a 404 for sam.
    let (status, _, _) = client
        .req(
            Method::POST,
            &format!("/api/v1/profiles/{kayla_id}/select"),
            Some(&sam),
            Some(json!({"pin": "1234"})),
        )
        .await;
    assert_eq!(status, 404);

    // A2: session list + revoke. A second admin login is a second
    // session; revoking it kills that cookie.
    let (status, _, admin2_cookie) = client.login("jordan", "admin-pass-123").await;
    assert_eq!(status, 200);
    let admin2 = admin2_cookie.unwrap();
    let (status, sessions, _) = client
        .req(Method::GET, "/api/v1/auth/sessions", Some(&admin), None)
        .await;
    assert_eq!(status, 200);
    assert!(sessions.as_array().unwrap().len() >= 2, "{sessions}");
    let other = sessions
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["current"].as_bool() == Some(false))
        .expect("a non-current session")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, _, _) = client
        .req(
            Method::DELETE,
            &format!("/api/v1/auth/sessions/{other}"),
            Some(&admin),
            None,
        )
        .await;
    assert_eq!(status, 204);
    let (status, _, _) = client
        .req(Method::GET, "/api/v1/auth/me", Some(&admin2), None)
        .await;
    assert_eq!(status, 401, "revoked session is dead");

    // A1: rate limiting — five bad passwords lock the account;
    // even the right password is refused while locked.
    let (status, invite2, _) = client
        .req(Method::POST, "/api/v1/admin/invites", Some(&admin), None)
        .await;
    assert_eq!(status, 201);
    let token2 = invite2["token"].as_str().unwrap().to_string();
    let (status, _, _) = client
        .req(
            Method::POST,
            "/api/v1/invites/accept",
            None,
            Some(json!({"token": token2, "username": "locky", "password": "locky-pass-12"})),
        )
        .await;
    assert_eq!(status, 201);
    for _ in 0..5 {
        let (status, _, _) = client.login("locky", "wrong-password").await;
        assert_eq!(status, 401);
    }
    let (status, _, _) = client.login("locky", "locky-pass-12").await;
    assert_eq!(status, 429, "locked after repeated failures");
}
