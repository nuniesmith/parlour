//! End-to-end recent-searches flow against a real Postgres (UI9):
//! `/api/v1/search/recent` records committed searches per profile,
//! dedupes case-insensitively, keeps the newest ten, and never
//! shows or deletes another profile's entries.
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

fn queries(list: &Value) -> Vec<&str> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|r| r["query"].as_str().unwrap())
        .collect()
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL"]
async fn search_flow() {
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

    let (status, body, cookie) = client.login("jordan", "admin-pass-123").await;
    assert_eq!(status, 200, "{body}");
    let cookie = cookie.expect("session cookie");
    let c = Some(cookie.as_str());
    let profile = body["profiles"][0]["id"].as_str().unwrap().to_string();
    const RECENT: &str = "/api/v1/search/recent";

    // Recent searches belong to a profile: none selected is a 400.
    let (status, _, _) = client.req(Method::GET, RECENT, c, None).await;
    assert_eq!(status, 400);
    let (status, _, _) = client
        .req(Method::POST, RECENT, c, Some(json!({"query": "dune"})))
        .await;
    assert_eq!(status, 400);

    let select = |id: String| format!("/api/v1/profiles/{id}/select");
    let (status, _, _) = client
        .req(Method::POST, &select(profile.clone()), c, Some(json!({})))
        .await;
    assert_eq!(status, 200);

    let (status, list, _) = client.req(Method::GET, RECENT, c, None).await;
    assert_eq!(status, 200, "{list}");
    assert_eq!(list, json!([]));

    // Recording returns the updated list, newest first, trimmed.
    let (status, list, _) = client
        .req(Method::POST, RECENT, c, Some(json!({"query": "  Dune "})))
        .await;
    assert_eq!(status, 200, "{list}");
    assert_eq!(queries(&list), ["Dune"]);
    let (_, list, _) = client
        .req(Method::POST, RECENT, c, Some(json!({"query": "Arrival"})))
        .await;
    assert_eq!(queries(&list), ["Arrival", "Dune"]);

    // The same search again, in any case, moves to the top with
    // its latest spelling instead of adding a second row.
    let (_, list, _) = client
        .req(Method::POST, RECENT, c, Some(json!({"query": "dune"})))
        .await;
    assert_eq!(queries(&list), ["dune", "Arrival"]);

    // Too short or too long is refused, and nothing is stored.
    for bad in [json!({"query": " a "}), json!({"query": "x".repeat(201)})] {
        let (status, _, _) = client.req(Method::POST, RECENT, c, Some(bad)).await;
        assert_eq!(status, 400);
    }

    // Only the newest ten are kept.
    for n in 0..10 {
        client
            .req(
                Method::POST,
                RECENT,
                c,
                Some(json!({"query": format!("query {n:02}")})),
            )
            .await;
    }
    let (_, list, _) = client.req(Method::GET, RECENT, c, None).await;
    let got = queries(&list);
    assert_eq!(got.len(), 10, "{list}");
    assert_eq!(got[0], "query 09");
    assert_eq!(got[9], "query 00");
    assert!(!got.contains(&"dune") && !got.contains(&"Arrival"));
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM recent_searches")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, 10, "older rows are deleted, not just hidden");

    // Forget one.
    let first_id = list[0]["id"].as_str().unwrap().to_string();
    let one = format!("{RECENT}/{first_id}");
    let (status, _, _) = client.req(Method::DELETE, &one, c, None).await;
    assert_eq!(status, 204);
    let (status, _, _) = client.req(Method::DELETE, &one, c, None).await;
    assert_eq!(status, 404);
    let (status, _, _) = client
        .req(Method::DELETE, &format!("{RECENT}/not-a-uuid"), c, None)
        .await;
    assert_eq!(status, 404);
    let (_, list, _) = client.req(Method::GET, RECENT, c, None).await;
    assert_eq!(queries(&list).len(), 9);
    let second_id = list[0]["id"].as_str().unwrap().to_string();

    // A second profile on the same account has its own list and
    // cannot see or delete the first profile's entries.
    let (status, kids, _) = client
        .req(
            Method::POST,
            "/api/v1/profiles",
            c,
            Some(json!({"name": "Kids"})),
        )
        .await;
    assert_eq!(status, 201, "{kids}");
    let kids_id = kids["id"].as_str().unwrap().to_string();
    let (status, _, _) = client
        .req(Method::POST, &select(kids_id), c, Some(json!({})))
        .await;
    assert_eq!(status, 200);
    let (_, list, _) = client.req(Method::GET, RECENT, c, None).await;
    assert_eq!(list, json!([]));
    let (status, _, _) = client
        .req(Method::DELETE, &format!("{RECENT}/{second_id}"), c, None)
        .await;
    assert_eq!(status, 404);
    let (_, list, _) = client
        .req(Method::POST, RECENT, c, Some(json!({"query": "Bluey"})))
        .await;
    assert_eq!(queries(&list), ["Bluey"]);
    // Clearing only clears the active profile's list.
    let (status, _, _) = client.req(Method::DELETE, RECENT, c, None).await;
    assert_eq!(status, 204);
    let (_, list, _) = client.req(Method::GET, RECENT, c, None).await;
    assert_eq!(list, json!([]));

    let (status, _, _) = client
        .req(Method::POST, &select(profile.clone()), c, Some(json!({})))
        .await;
    assert_eq!(status, 200);
    let (_, list, _) = client.req(Method::GET, RECENT, c, None).await;
    assert_eq!(queries(&list).len(), 9, "{list}");

    // Deleting a profile deletes its searches.
    let (status, _, _) = client
        .req(
            Method::DELETE,
            &format!("/api/v1/profiles/{profile}"),
            c,
            None,
        )
        .await;
    assert_eq!(status, 204);
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM recent_searches")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, 0);

    // Signed out is 401.
    let (status, _, _) = client.req(Method::GET, RECENT, None, None).await;
    assert_eq!(status, 401);
}
