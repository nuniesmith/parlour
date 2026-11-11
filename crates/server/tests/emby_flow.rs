//! End-to-end test of Milestone 1.7: a stub Emby server feeds
//! the admin status endpoint and the import job; matching is by
//! provider id, dry runs write nothing, ticks convert to ms.
//!
//! Ignored by default; needs PARLOUR_TEST_DATABASE_URL and
//! ffmpeg/ffprobe on PATH.
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test emby_flow -- --ignored

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;

use axum::extract::Query;
use axum::routing::get;
use axum::{Json, Router};
use reqwest::header::SET_COOKIE;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn make_mp4(dir: &Path, name: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=c=gray:s=320x240:d=2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=220:duration=2",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
            path.to_str().unwrap(),
        ])
        .status()
        .expect("ffmpeg available");
    assert!(status.success());
}

// --- stub Emby -------------------------------------------------------

#[derive(Deserialize)]
struct StubItemsQuery {
    #[serde(rename = "StartIndex")]
    start: Option<usize>,
    #[serde(rename = "Limit")]
    limit: Option<usize>,
}

fn stub_items() -> Vec<Value> {
    vec![
        json!({"Id": "m1", "Name": "Alien", "Type": "Movie", "ProductionYear": 1979,
               "ProviderIds": {"Tmdb": "348"},
               "UserData": {"Played": true, "PlayCount": 2, "IsFavorite": true}}),
        json!({"Id": "m2", "Name": "In Progress", "Type": "Movie", "ProductionYear": 2020,
               "ProviderIds": {"Tmdb": "999002"},
               "UserData": {"PlaybackPositionTicks": 600_000_000_i64}}),
        json!({"Id": "m3", "Name": "Not In Parlour", "Type": "Movie", "ProductionYear": 2001,
               "ProviderIds": {"Tmdb": "424242"},
               "UserData": {"Played": true}}),
        json!({"Id": "m4", "Name": "No State", "Type": "Movie", "ProductionYear": 2010,
               "ProviderIds": {"Tmdb": "555"}, "UserData": {}}),
        json!({"Id": "s1", "Name": "Test Show", "Type": "Series",
               "ProviderIds": {"Tmdb": "777001"}, "UserData": {}}),
        json!({"Id": "e1", "Name": "Episode One", "Type": "Episode", "SeriesId": "s1",
               "ParentIndexNumber": 1, "IndexNumber": 1, "ProviderIds": {},
               "UserData": {"Played": true}}),
    ]
}

async fn stub_users() -> Json<Value> {
    Json(json!([{"Id": "u1", "Name": "Jordan"}]))
}

async fn stub_items_page(Query(q): Query<StubItemsQuery>) -> Json<Value> {
    let all = stub_items();
    let start = q.start.unwrap_or(0);
    let limit = q.limit.unwrap_or(all.len());
    let page: Vec<Value> = all.iter().skip(start).take(limit).cloned().collect();
    Json(json!({"Items": page, "TotalRecordCount": all.len()}))
}

async fn spawn_stub(port: u16) {
    let app = Router::new()
        .route("/emby/Users", get(stub_users))
        .route("/emby/Users/{uid}/Items", get(stub_items_page));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
}

// --- Parlour side ----------------------------------------------------

struct TestApp {
    base: String,
    client: reqwest::Client,
    cookie: String,
}

impl TestApp {
    async fn req(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut builder = self
            .client
            .request(method, format!("{}{}", self.base, path))
            .header(reqwest::header::COOKIE, &self.cookie);
        if let Some(body) = body {
            builder = builder.json(&body);
        }
        let resp = builder.send().await.expect("request");
        let status = resp.status().as_u16();
        let value = resp.json::<Value>().await.unwrap_or(Value::Null);
        (status, value)
    }
}

async fn wait_for_job(app: &TestApp, job_id: &str) -> Value {
    for _ in 0..60 {
        let (_, job) = app
            .req(
                reqwest::Method::GET,
                &format!("/api/v1/admin/jobs/{job_id}"),
                None,
            )
            .await;
        match job["status"].as_str() {
            Some("succeeded") => return job,
            Some("failed") => panic!("job failed: {job}"),
            _ => sleep(Duration::from_millis(500)).await,
        }
    }
    panic!("job did not finish");
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL and ffmpeg"]
async fn emby_flow() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test database url");
    let port = free_port();
    let stub_port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    spawn_stub(stub_port).await;

    let pool = parlour_db::connect(&db_url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");

    let fixtures = PathBuf::from(format!("/tmp/parlour-emby-fixtures-{port}"));
    let _ = std::fs::remove_dir_all(&fixtures);
    make_mp4(&fixtures.join("Alien (1979)"), "Alien (1979).mp4");
    make_mp4(
        &fixtures.join("In Progress (2020)"),
        "In Progress (2020).mp4",
    );
    make_mp4(
        &fixtures.join("Test Show").join("Season 1"),
        "Test Show S01E01.mp4",
    );

    let mut vars = std::collections::HashMap::new();
    vars.insert("PARLOUR_DATABASE_URL".to_string(), db_url.clone());
    vars.insert("PARLOUR_ADMIN_USER".to_string(), "jordan".to_string());
    vars.insert(
        "PARLOUR_ADMIN_PASSWORD".to_string(),
        "admin-pass-123".to_string(),
    );
    let cfg = parlour_server::config::Config::from_vars(&vars).expect("config");
    parlour_server::auth::bootstrap_admin(&pool, &cfg)
        .await
        .expect("bootstrap");

    let emby = parlour_catalog::EmbySettings {
        base_url: Some(format!("http://127.0.0.1:{stub_port}")),
        api_key: Some("stub-key".to_string()),
    };
    let mut state = parlour_server::http::AppState::new(pool.clone(), None, false);
    state.emby = emby.clone();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            parlour_server::http::router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    parlour_server::catalog::spawn_background(
        pool.clone(),
        parlour_server::metadata::MetadataSettings::default(),
        emby,
    );

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"username": "jordan", "password": "admin-pass-123"}))
        .send()
        .await
        .expect("login");
    assert_eq!(resp.status().as_u16(), 200);
    let cookie = resp
        .headers()
        .get(SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let app = TestApp {
        base: base.clone(),
        client,
        cookie,
    };

    // Scan a library and stamp provider ids as TMDB matching would.
    let (_, profiles) = app
        .req(reqwest::Method::GET, "/api/v1/profiles", None)
        .await;
    let profile_id = profiles[0]["id"].as_str().unwrap().to_string();
    app.req(
        reqwest::Method::POST,
        &format!("/api/v1/profiles/{profile_id}/select"),
        Some(json!({})),
    )
    .await;
    let (status, lib) = app
        .req(
            reqwest::Method::POST,
            "/api/v1/admin/libraries",
            Some(json!({"name": "Media", "kind": "movies", "paths": [fixtures.to_str().unwrap()]})),
        )
        .await;
    assert_eq!(status, 201, "{lib}");
    let library_id = lib["id"].as_str().unwrap().to_string();
    app.req(
        reqwest::Method::POST,
        &format!("/api/v1/admin/libraries/{library_id}/access"),
        Some(json!({"profile_id": profile_id})),
    )
    .await;
    let (_, job) = app
        .req(
            reqwest::Method::POST,
            &format!("/api/v1/admin/libraries/{library_id}/scan"),
            None,
        )
        .await;
    wait_for_job(&app, job["id"].as_str().unwrap()).await;

    let (_, items) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/libraries/{library_id}/items"),
            None,
        )
        .await;
    assert_eq!(items.as_array().unwrap().len(), 3, "{items}");
    for (title, tmdb) in [
        ("Alien", 348),
        ("In Progress", 999002),
        ("Test Show", 777001),
    ] {
        sqlx::query("UPDATE media_items SET tmdb_id = $1 WHERE title = $2")
            .bind(tmdb)
            .bind(title)
            .execute(&pool)
            .await
            .unwrap();
    }

    // Status sees the stub.
    let (status, emby_status) = app
        .req(reqwest::Method::GET, "/api/v1/admin/emby/status", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(emby_status["configured"], true);
    assert_eq!(emby_status["reachable"], true);
    assert_eq!(emby_status["users"][0]["name"], "Jordan");

    // Mapping screen profiles.
    let (_, profs) = app
        .req(reqwest::Method::GET, "/api/v1/admin/emby/profiles", None)
        .await;
    assert_eq!(profs.as_array().unwrap().len(), 1);
    assert_eq!(profs[0]["username"], "jordan");

    // Dry run: counts, and provably no writes.
    let (status, job) = app
        .req(
            reqwest::Method::POST,
            "/api/v1/admin/emby/import",
            Some(json!({"emby_user_id": "u1", "profile_id": profile_id, "dry_run": true})),
        )
        .await;
    assert_eq!(status, 202, "{job}");
    let job = wait_for_job(&app, job["id"].as_str().unwrap()).await;
    let report = &job["result"];
    assert_eq!(report["dry_run"], true, "{report}");
    assert_eq!(report["matched"], 3, "{report}");
    assert_eq!(report["applied"], 0, "{report}");
    assert_eq!(report["unmatched"], 1, "{report}");
    assert_eq!(report["skipped_no_state"], 1, "{report}");
    assert_eq!(report["unmatched_items"][0], "Not In Parlour (2001)");
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM user_data")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "dry run must not write");

    // Real run: state lands, ticks converted.
    let (status, job) = app
        .req(
            reqwest::Method::POST,
            "/api/v1/admin/emby/import",
            Some(json!({"emby_user_id": "u1", "profile_id": profile_id, "dry_run": false})),
        )
        .await;
    assert_eq!(status, 202);
    let job = wait_for_job(&app, job["id"].as_str().unwrap()).await;
    let report = &job["result"];
    assert_eq!(report["matched"], 3, "{report}");
    assert!(report["applied"].as_i64().unwrap() >= 3, "{report}");

    let alien = items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["title"] == "Alien")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, detail) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/items/{alien}"),
            None,
        )
        .await;
    assert_eq!(detail["user_state"]["played"], true);
    assert_eq!(detail["user_state"]["is_favorite"], true);
    assert_eq!(detail["user_state"]["play_count"], 2);

    let inprog = items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["title"] == "In Progress")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, detail) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/items/{inprog}"),
            None,
        )
        .await;
    assert_eq!(detail["user_state"]["position_ms"], 60_000);
    assert_eq!(detail["user_state"]["played"], false);

    // The episode's state landed too (via series id + S/E numbers).
    let (played_eps,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM user_data u JOIN media_items i ON i.id = u.item_id
         WHERE i.kind = 'episode' AND u.played",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(played_eps, 1);

    // Re-running is idempotent: same counts, no duplicates.
    let (_, job) = app
        .req(
            reqwest::Method::POST,
            "/api/v1/admin/emby/import",
            Some(json!({"emby_user_id": "u1", "profile_id": profile_id, "dry_run": false})),
        )
        .await;
    let job = wait_for_job(&app, job["id"].as_str().unwrap()).await;
    assert_eq!(job["result"]["matched"], 3);
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM user_data")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 3);

    let _ = std::fs::remove_dir_all(&fixtures);
    server.abort();
}
