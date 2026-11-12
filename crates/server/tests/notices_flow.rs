//! End-to-end server notices against a real Postgres (UI5):
//! a failed job, the metadata-review backlog and low disk space
//! each raise a notice; conditions that clear resolve theirs;
//! dismissal is per admin and does not survive a severity change.
//!
//! Ignored by default: set PARLOUR_TEST_DATABASE_URL to a
//! throwaway database and run `cargo test -- --ignored`.
//! The database is migrated fresh by the server code itself.

use std::net::SocketAddr;
use std::path::PathBuf;

use parlour_server::config::Config;
use parlour_server::http::AppState;
use parlour_server::notices::{self, DiskReading};
use serde_json::{json, Value};

const GIB: u64 = 1024 * 1024 * 1024;

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

fn cache_disk(free_bytes: Option<u64>) -> Vec<DiskReading> {
    vec![DiskReading {
        label: "cache".to_string(),
        path: PathBuf::from("/cache"),
        free_bytes,
    }]
}

fn summaries(list: &Value) -> Vec<String> {
    list.as_array()
        .expect("notice array")
        .iter()
        .map(|n| n["summary"].as_str().unwrap().to_string())
        .collect()
}

fn find<'a>(list: &'a Value, kind: &str) -> &'a Value {
    list.as_array()
        .unwrap()
        .iter()
        .find(|n| n["kind"] == kind)
        .unwrap_or_else(|| panic!("no {kind} notice in {list}"))
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL"]
async fn notices_flow() {
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
    let list = "/api/v1/admin/notices";

    let (status, _, _) = client.req(Method::GET, list, None, None).await;
    assert_eq!(status, 401);
    let (_, _, cookie) = client.login("jordan", "admin-pass-123").await;
    let jordan = cookie.expect("admin session cookie");

    // A healthy server has nothing to say.
    notices::reconcile_with(&pool, &cache_disk(Some(200 * GIB)))
        .await
        .unwrap();
    let (status, got, _) = client.req(Method::GET, list, Some(&jordan), None).await;
    assert_eq!(status, 200, "{got}");
    assert_eq!(got.as_array().unwrap().len(), 0, "{got}");

    // --- Three conditions appear.
    let (library_id,): (uuid::Uuid,) = sqlx::query_as(
        "INSERT INTO libraries (name, kind) VALUES ('Movies', 'movies') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let job_id = parlour_jobs::enqueue(
        &pool,
        parlour_jobs::JobKind::ScanLibrary,
        Some(library_id),
        json!({"trigger": "manual"}),
    )
    .await
    .unwrap();
    parlour_jobs::fail(&pool, job_id, "the folder is not readable")
        .await
        .unwrap();
    for n in 1..=2 {
        sqlx::query(
            "INSERT INTO media_items (library_id, kind, stable_key, title, sort_title, metadata_status)
             VALUES ($1, 'movie', $2, $2, $2, 'needs_review')",
        )
        .bind(library_id)
        .bind(format!("mystery-{n}"))
        .execute(&pool)
        .await
        .unwrap();
    }
    notices::reconcile_with(&pool, &cache_disk(Some(3 * GIB)))
        .await
        .unwrap();
    // Reconciling again must not duplicate anything.
    notices::reconcile_with(&pool, &cache_disk(Some(3 * GIB)))
        .await
        .unwrap();

    let (_, got, _) = client.req(Method::GET, list, Some(&jordan), None).await;
    assert_eq!(got.as_array().unwrap().len(), 3, "{got}");
    let job = find(&got, "job_failed");
    assert_eq!(job["severity"], "error");
    assert_eq!(job["summary"], "Scan of Movies failed");
    assert_eq!(job["detail"]["error"], "the folder is not readable");
    assert_eq!(job["detail"]["job_id"], job_id.to_string());
    let review = find(&got, "metadata_review");
    assert_eq!(review["severity"], "info");
    assert_eq!(review["summary"], "2 items need a metadata match");
    let disk = find(&got, "disk_low");
    assert_eq!(disk["severity"], "warning");
    assert_eq!(
        disk["summary"],
        "Low disk space for the cache folder: 3.0 GB free"
    );
    let disk_id = disk["id"].as_str().unwrap().to_string();
    let job_notice_id = job["id"].as_str().unwrap().to_string();

    // --- Dismissal is per admin.
    let (status, body, _) = client
        .req(Method::POST, "/api/v1/admin/invites", Some(&jordan), None)
        .await;
    assert_eq!(status, 201, "{body}");
    let token = body["token"].as_str().unwrap().to_string();
    let (status, body, _) = client
        .req(
            Method::POST,
            "/api/v1/invites/accept",
            None,
            Some(json!({"token": token, "username": "kayla", "password": "kayla-pass-12345"})),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    let (_, _, cookie) = client.login("kayla", "kayla-pass-12345").await;
    let kayla = cookie.expect("kayla session cookie");

    // Not an admin yet: the routes do not exist for her (A7).
    let (status, _, _) = client.req(Method::GET, list, Some(&kayla), None).await;
    assert_eq!(status, 404);
    let dismiss_disk = format!("/api/v1/admin/notices/{disk_id}/dismiss");
    let (status, _, _) = client
        .req(Method::POST, &dismiss_disk, Some(&kayla), None)
        .await;
    assert_eq!(status, 404);
    sqlx::query("UPDATE users SET is_admin = true WHERE username = 'kayla'")
        .execute(&pool)
        .await
        .unwrap();

    let (status, body, _) = client
        .req(Method::POST, &dismiss_disk, Some(&jordan), None)
        .await;
    assert_eq!(status, 204, "{body}");
    // Dismissing twice is fine.
    let (status, _, _) = client
        .req(Method::POST, &dismiss_disk, Some(&jordan), None)
        .await;
    assert_eq!(status, 204);
    let (_, got, _) = client.req(Method::GET, list, Some(&jordan), None).await;
    assert_eq!(got.as_array().unwrap().len(), 2, "hidden for jordan: {got}");
    let (_, got, _) = client.req(Method::GET, list, Some(&kayla), None).await;
    assert_eq!(
        got.as_array().unwrap().len(),
        3,
        "still there for kayla: {got}"
    );

    // Unknown or malformed ids are 404.
    for bad in ["00000000-0000-0000-0000-000000000000", "not-a-uuid"] {
        let (status, _, _) = client
            .req(
                Method::POST,
                &format!("/api/v1/admin/notices/{bad}/dismiss"),
                Some(&jordan),
                None,
            )
            .await;
        assert_eq!(status, 404, "{bad}");
    }

    // --- A reconcile with nothing changed keeps the dismissal...
    notices::reconcile_with(&pool, &cache_disk(Some(3 * GIB)))
        .await
        .unwrap();
    let (_, got, _) = client.req(Method::GET, list, Some(&jordan), None).await;
    assert_eq!(got.as_array().unwrap().len(), 2, "{got}");

    // ...but it getting worse brings the notice back.
    notices::reconcile_with(&pool, &cache_disk(Some(300 * 1024 * 1024)))
        .await
        .unwrap();
    let (_, got, _) = client.req(Method::GET, list, Some(&jordan), None).await;
    let disk = find(&got, "disk_low");
    assert_eq!(disk["severity"], "error");
    assert_eq!(
        disk["summary"],
        "Low disk space for the cache folder: 300 MB free"
    );
    assert_eq!(disk["id"], disk_id.as_str(), "same notice, escalated");

    // --- Conditions that clear resolve their notices.
    sqlx::query("UPDATE media_items SET metadata_status = 'matched'")
        .execute(&pool)
        .await
        .unwrap();
    notices::reconcile_with(&pool, &cache_disk(Some(200 * GIB)))
        .await
        .unwrap();
    let (_, got, _) = client.req(Method::GET, list, Some(&jordan), None).await;
    assert_eq!(summaries(&got), ["Scan of Movies failed"], "{got}");

    // A folder that cannot be measured is not reported as low.
    notices::reconcile_with(&pool, &cache_disk(None))
        .await
        .unwrap();
    let (_, got, _) = client.req(Method::GET, list, Some(&jordan), None).await;
    assert_eq!(got.as_array().unwrap().len(), 1, "{got}");

    // A failed-job notice stays until dismissed.
    let (status, _, _) = client
        .req(
            Method::POST,
            &format!("/api/v1/admin/notices/{job_notice_id}/dismiss"),
            Some(&jordan),
            None,
        )
        .await;
    assert_eq!(status, 204);
    notices::reconcile_with(&pool, &cache_disk(Some(200 * GIB)))
        .await
        .unwrap();
    let (_, got, _) = client.req(Method::GET, list, Some(&jordan), None).await;
    assert_eq!(got.as_array().unwrap().len(), 0, "not re-raised: {got}");

    // --- Restart-required: raised by hand (nothing raises it yet), cleared by resolve.
    notices::raise_restart_required(&pool, "test")
        .await
        .unwrap();
    let (_, got, _) = client.req(Method::GET, list, Some(&jordan), None).await;
    let restart = find(&got, "restart_required");
    assert_eq!(restart["severity"], "warning");
    notices::resolve(&pool, "restart_required").await.unwrap();
    let (_, got, _) = client.req(Method::GET, list, Some(&jordan), None).await;
    assert_eq!(got.as_array().unwrap().len(), 0, "{got}");
}
