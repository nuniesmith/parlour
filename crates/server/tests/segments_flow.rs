//! End-to-end test of SEG1 (Phase 9, D15): `media_segments`
//! rows come back on playback-info, resolved to one per kind,
//! start-ordered, with a manual row always winning over the
//! automatic sources; the schema rejects impossible spans and a
//! second row for the same (item, kind, source). And of SEG5's
//! server half (D22): playback-info carries the active profile's
//! skip mode (`segment_skip`), read fresh on every call and kept
//! per profile.
//!
//! Ignored by default; needs PARLOUR_TEST_DATABASE_URL (a fresh
//! database of its own) and ffmpeg/ffprobe on PATH.
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test segments_flow -- --ignored

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;

use reqwest::header::{COOKIE, SET_COOKIE};
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn ffmpeg(args: &[&str]) {
    let status = Command::new("ffmpeg")
        .args(args)
        .status()
        .expect("ffmpeg available");
    assert!(status.success(), "ffmpeg failed: {args:?}");
}

fn make_mp4(dir: &Path, name: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    ffmpeg(&[
        "-y",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=teal:s=320x240:d=4",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=4",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-shortest",
        path.to_str().unwrap(),
    ]);
}

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
            .header(COOKIE, &self.cookie);
        if let Some(body) = body {
            builder = builder.json(&body);
        }
        let resp = builder.send().await.expect("request");
        let status = resp.status().as_u16();
        let value = resp.json::<Value>().await.unwrap_or(Value::Null);
        (status, value)
    }
}

async fn wait_for_job(app: &TestApp, job_id: &str) {
    for _ in 0..60 {
        let (_, job) = app
            .req(
                reqwest::Method::GET,
                &format!("/api/v1/admin/jobs/{job_id}"),
                None,
            )
            .await;
        match job["status"].as_str() {
            Some("succeeded") => return,
            Some("failed") => panic!("job failed: {job}"),
            _ => sleep(Duration::from_millis(500)).await,
        }
    }
    panic!("job did not finish");
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL and ffmpeg"]
async fn segments_flow() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test database url");
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let pool = parlour_db::connect(&db_url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");

    let fixtures = PathBuf::from(format!("/tmp/parlour-segments-fixtures-{port}"));
    let _ = std::fs::remove_dir_all(&fixtures);
    let season = fixtures.join("Test Show").join("Season 1");
    make_mp4(&season, "Test Show S01E01.mp4");
    make_mp4(&season, "Test Show S01E02.mp4");

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

    let cache_dir = fixtures.join("cache");
    let mut state = parlour_server::http::AppState::new(pool.clone(), None, false);
    std::sync::Arc::get_mut(&mut state.playback)
        .expect("fresh playback state")
        .configure(cache_dir, None);
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

    let (_, profiles) = app
        .req(reqwest::Method::GET, "/api/v1/profiles", None)
        .await;
    let profile_id = profiles[0]["id"].as_str().unwrap().to_string();
    let (status, _) = app
        .req(
            reqwest::Method::POST,
            &format!("/api/v1/profiles/{profile_id}/select"),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 200);

    let (status, lib) = app
        .req(
            reqwest::Method::POST,
            "/api/v1/admin/libraries",
            Some(json!({"name": "Media", "kind": "movies", "paths": [fixtures.to_str().unwrap()]})),
        )
        .await;
    assert_eq!(status, 201, "{lib}");
    let library_id = lib["id"].as_str().unwrap().to_string();
    let (status, _) = app
        .req(
            reqwest::Method::POST,
            &format!("/api/v1/admin/libraries/{library_id}/access"),
            Some(json!({"profile_id": profile_id})),
        )
        .await;
    assert_eq!(status, 204);
    let (status, job) = app
        .req(
            reqwest::Method::POST,
            &format!("/api/v1/admin/libraries/{library_id}/scan"),
            None,
        )
        .await;
    assert_eq!(status, 202);
    parlour_server::catalog::spawn_background(
        pool.clone(),
        parlour_server::metadata::MetadataSettings::default(),
        parlour_catalog::EmbySettings::default(),
        None,
    );
    wait_for_job(&app, job["id"].as_str().unwrap()).await;
    // The scan queued segment detection for the two episodes. Let it
    // finish before rows are written by hand below: a detection run
    // replaces its own sources' rows (chapters, heuristic, and — for
    // this two-episode season, too small to match (D20) — every
    // fingerprint row), so it must not overlap them.
    let (_, jobs) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/admin/jobs?library_id={library_id}&limit=100"),
            None,
        )
        .await;
    let detection = jobs
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["kind"] == "detect_segments")
        .unwrap_or_else(|| panic!("the scan queues segment detection: {jobs}"))
        .clone();
    wait_for_job(&app, detection["id"].as_str().unwrap()).await;

    let (_, items) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/libraries/{library_id}/items"),
            None,
        )
        .await;
    let series_id = items
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["title"] == "Test Show")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, series_detail) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/items/{series_id}"),
            None,
        )
        .await;
    let season_id = series_detail["children"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, season_detail) = app
        .req(
            reqwest::Method::GET,
            &format!("/api/v1/items/{season_id}"),
            None,
        )
        .await;
    let episodes = season_detail["children"].as_array().unwrap();
    assert_eq!(episodes.len(), 2);
    let ep_id = |n: i64| -> uuid::Uuid {
        episodes.iter().find(|e| e["episode_number"] == n).unwrap()["id"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap()
    };
    let ep1 = ep_id(1);
    let ep2 = ep_id(2);

    let segments = |info: &Value| -> Vec<(String, i64, i64, String)> {
        info["segments"]
            .as_array()
            .unwrap_or_else(|| panic!("no segments array: {info}"))
            .iter()
            .map(|s| {
                (
                    s["kind"].as_str().unwrap().to_string(),
                    s["start_ms"].as_i64().unwrap(),
                    s["end_ms"].as_i64().unwrap(),
                    s["source"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };
    let info_path =
        |id: uuid::Uuid| format!("/api/v1/items/{id}/playback-info?client=web&network=lan");

    // No rows yet: an empty list, not a missing field.
    let (status, info) = app.req(reqwest::Method::GET, &info_path(ep1), None).await;
    assert_eq!(status, 200, "{info}");
    assert_eq!(segments(&info), vec![]);

    // Automatic rows only: one per kind, start-ordered; between two
    // auto sources the higher confidence wins.
    let insert = |item: uuid::Uuid,
                  kind: &'static str,
                  start: i64,
                  end: i64,
                  source: &'static str,
                  confidence: f32| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO media_segments (item_id, kind, start_ms, end_ms, source, confidence)
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(item)
            .bind(kind)
            .bind(start)
            .bind(end)
            .bind(source)
            .bind(confidence)
            .execute(&pool)
            .await
        }
    };
    insert(ep1, "credits", 3_000, 4_000, "fingerprint", 0.8)
        .await
        .unwrap();
    insert(ep1, "intro", 1_000, 2_000, "fingerprint", 0.9)
        .await
        .unwrap();
    insert(ep1, "intro", 0, 1_500, "chapters", 0.7)
        .await
        .unwrap();
    insert(ep1, "recap", 0, 500, "chapters", 1.0).await.unwrap();
    let (_, info) = app.req(reqwest::Method::GET, &info_path(ep1), None).await;
    assert_eq!(
        segments(&info),
        vec![
            ("recap".into(), 0, 500, "chapters".into()),
            ("intro".into(), 1_000, 2_000, "fingerprint".into()),
            ("credits".into(), 3_000, 4_000, "fingerprint".into()),
        ],
        "{info}"
    );
    assert!((info["segments"][1]["confidence"].as_f64().unwrap() - 0.9).abs() < 1e-6);

    // A manual row wins over every automatic source, even with a
    // lower stored confidence.
    insert(ep1, "intro", 1_200, 1_900, "manual", 0.5)
        .await
        .unwrap();
    let (_, info) = app.req(reqwest::Method::GET, &info_path(ep1), None).await;
    assert_eq!(
        segments(&info)[1],
        ("intro".into(), 1_200, 1_900, "manual".into()),
        "{info}"
    );
    // An automatic re-run upserts only its own row: the manual
    // edit still wins afterwards.
    sqlx::query(
        "INSERT INTO media_segments (item_id, kind, start_ms, end_ms, source, confidence)
         VALUES ($1, 'intro', 900, 2_100, 'fingerprint', 1.0)
         ON CONFLICT (item_id, kind, source) DO UPDATE
         SET start_ms = EXCLUDED.start_ms, end_ms = EXCLUDED.end_ms,
             confidence = EXCLUDED.confidence, updated_at = now()",
    )
    .bind(ep1)
    .execute(&pool)
    .await
    .unwrap();
    let (_, info) = app.req(reqwest::Method::GET, &info_path(ep1), None).await;
    assert_eq!(segments(&info)[1].3, "manual", "{info}");

    // Segments are per item: episode 2 is untouched.
    let (_, info) = app.req(reqwest::Method::GET, &info_path(ep2), None).await;
    assert_eq!(segments(&info), vec![]);

    // SEG5 (D22): playback-info also says what the active profile
    // wants done inside those segments. The default is the Skip
    // button (UI7), and a change applies to the very next call.
    let (_, info) = app.req(reqwest::Method::GET, &info_path(ep1), None).await;
    assert_eq!(info["segment_skip"], "button", "{info}");
    let set_skip = |profile: String, mode: &'static str| {
        let app = &app;
        async move {
            let (status, body) = app
                .req(
                    reqwest::Method::PATCH,
                    &format!("/api/v1/profiles/{profile}"),
                    Some(json!({"segment_skip": mode})),
                )
                .await;
            assert_eq!(status, 200, "{body}");
            assert_eq!(body["segment_skip"], mode, "{body}");
        }
    };
    for mode in ["auto", "off", "button", "auto"] {
        set_skip(profile_id.clone(), mode).await;
        let (status, info) = app.req(reqwest::Method::GET, &info_path(ep1), None).await;
        assert_eq!(status, 200, "{info}");
        assert_eq!(info["segment_skip"], mode, "{info}");
    }

    // Per profile: a second profile on the same account keeps its own
    // choice, sees the same segments (they belong to the item), and the
    // first profile's choice is untouched by it.
    let (status, kids) = app
        .req(
            reqwest::Method::POST,
            "/api/v1/profiles",
            Some(json!({"name": "Kids"})),
        )
        .await;
    assert_eq!(status, 201, "{kids}");
    assert_eq!(kids["segment_skip"], "button", "{kids}");
    let kids_id = kids["id"].as_str().unwrap().to_string();
    let (status, _) = app
        .req(
            reqwest::Method::POST,
            &format!("/api/v1/admin/libraries/{library_id}/access"),
            Some(json!({"profile_id": kids_id})),
        )
        .await;
    assert_eq!(status, 204);
    set_skip(kids_id.clone(), "off").await;
    let select = |profile: String| {
        let app = &app;
        async move {
            let (status, body) = app
                .req(
                    reqwest::Method::POST,
                    &format!("/api/v1/profiles/{profile}/select"),
                    Some(json!({})),
                )
                .await;
            assert_eq!(status, 200, "{body}");
        }
    };
    select(kids_id.clone()).await;
    let (status, kids_info) = app.req(reqwest::Method::GET, &info_path(ep1), None).await;
    assert_eq!(status, 200, "{kids_info}");
    assert_eq!(kids_info["segment_skip"], "off", "{kids_info}");
    assert_eq!(
        segments(&kids_info),
        segments(&info),
        "segments are the item's"
    );
    select(profile_id.clone()).await;
    let (_, info) = app.req(reqwest::Method::GET, &info_path(ep1), None).await;
    assert_eq!(info["segment_skip"], "auto", "{info}");

    // The schema refuses what a player could not use.
    assert!(
        insert(ep2, "intro", 2_000, 2_000, "manual", 1.0)
            .await
            .is_err(),
        "empty span"
    );
    assert!(
        insert(ep2, "intro", -1, 2_000, "manual", 1.0)
            .await
            .is_err(),
        "negative start"
    );
    assert!(
        insert(ep2, "outro", 0, 1_000, "manual", 1.0).await.is_err(),
        "unknown kind"
    );
    assert!(
        insert(ep2, "intro", 0, 1_000, "guess", 1.0).await.is_err(),
        "unknown source"
    );
    assert!(
        insert(ep2, "intro", 0, 1_000, "fingerprint", 1.5)
            .await
            .is_err(),
        "confidence > 1"
    );
    insert(ep2, "intro", 0, 1_000, "manual", 1.0).await.unwrap();
    assert!(
        insert(ep2, "intro", 10, 1_000, "manual", 1.0)
            .await
            .is_err(),
        "one row per (item, kind, source)"
    );

    // Deleting the item takes its segments with it.
    sqlx::query("DELETE FROM media_items WHERE id = $1")
        .bind(ep2)
        .execute(&pool)
        .await
        .unwrap();
    let (left,): (i64,) = sqlx::query_as("SELECT count(*) FROM media_segments WHERE item_id = $1")
        .bind(ep2)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0);

    server.abort();
    let _ = std::fs::remove_dir_all(&fixtures);
}
