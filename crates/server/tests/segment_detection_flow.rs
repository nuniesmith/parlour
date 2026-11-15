//! End-to-end test of SEG2 (Phase 9; D15, D17, D18): a library scan
//! queues `detect_segments` for episodes whose files are new or
//! changed; the job stores chapter-named intros/credits (source
//! `chapters`, 1.0) and black-frame + silence guesses for what the
//! chapters leave open (source `heuristic`, 0.5); playback-info
//! serves only rows at or above the 0.6 floor, a manual row
//! whatever its confidence; an unchanged rescan queues nothing; a
//! changed file is looked at again and loses its stale rows, while
//! a manual row survives.
//!
//! Ignored by default; needs PARLOUR_TEST_DATABASE_URL (a fresh
//! database of its own) and ffmpeg/ffprobe on PATH.
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test segment_detection_flow -- --ignored

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;

use reqwest::header::{COOKIE, SET_COOKIE};
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};
use uuid::Uuid;

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A tiny real episode: a test pattern with a tone, `breaks` (in
/// seconds) cut to black *and* silence, and optional chapters.
fn make_media(path: &Path, seconds: u32, breaks: &[(u32, u32)], chapters: &[(u32, u32, &str)]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut video = format!("testsrc=d={seconds}:s=160x120:r=10");
    let mut audio = format!("sine=f=440:d={seconds}");
    if !breaks.is_empty() {
        let when = breaks
            .iter()
            .map(|(a, b)| format!("between(t,{a},{b})"))
            .collect::<Vec<_>>()
            .join("+");
        video.push_str(&format!(
            ",drawbox=x=0:y=0:w=iw:h=ih:color=black:t=fill:enable='{when}'"
        ));
        audio.push_str(&format!(",volume=volume=0:enable='{when}'"));
    }
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-v", "error", "-f", "lavfi", "-i", &video])
        .args(["-f", "lavfi", "-i", &audio]);
    let meta = path.with_extension("chapters.txt");
    if !chapters.is_empty() {
        let mut text = String::from(";FFMETADATA1\n");
        for (start, end, title) in chapters {
            text.push_str(&format!(
                "[CHAPTER]\nTIMEBASE=1/1000\nSTART={}\nEND={}\ntitle={title}\n",
                start * 1000,
                end * 1000
            ));
        }
        std::fs::write(&meta, text).unwrap();
        cmd.arg("-i")
            .arg(&meta)
            .args(["-map_metadata", "2", "-map_chapters", "2"]);
    }
    cmd.args([
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-shortest",
    ])
    .arg(path);
    let out = cmd.output().expect("ffmpeg available");
    assert!(
        out.status.success(),
        "ffmpeg failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_file(&meta);
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

    async fn get(&self, path: &str) -> Value {
        let (status, value) = self.req(reqwest::Method::GET, path, None).await;
        assert_eq!(status, 200, "GET {path}: {value}");
        value
    }
}

async fn wait_for_job(app: &TestApp, job_id: &str) -> Value {
    for _ in 0..240 {
        let job = app.get(&format!("/api/v1/admin/jobs/{job_id}")).await;
        match job["status"].as_str() {
            Some("succeeded") => return job,
            Some("failed") => panic!("job failed: {job}"),
            _ => sleep(Duration::from_millis(500)).await,
        }
    }
    panic!("job {job_id} did not finish");
}

/// Scan the library and wait for the scan to finish.
async fn scan(app: &TestApp, library_id: &str) {
    let (status, job) = app
        .req(
            reqwest::Method::POST,
            &format!("/api/v1/admin/libraries/{library_id}/scan"),
            None,
        )
        .await;
    assert_eq!(status, 202, "{job}");
    wait_for_job(app, job["id"].as_str().unwrap()).await;
}

/// The library's detect_segments jobs, newest first.
async fn detect_jobs(app: &TestApp, library_id: &str) -> Vec<Value> {
    let jobs = app
        .get(&format!(
            "/api/v1/admin/jobs?library_id={library_id}&limit=100"
        ))
        .await;
    jobs.as_array()
        .unwrap()
        .iter()
        .filter(|j| j["kind"] == "detect_segments")
        .cloned()
        .collect()
}

/// Stored rows for an item: (kind, source, start_ms, end_ms, confidence).
async fn rows(pool: &sqlx::PgPool, item: Uuid) -> Vec<(String, String, i64, i64, f32)> {
    sqlx::query_as(
        "SELECT kind, source, start_ms, end_ms, confidence FROM media_segments
         WHERE item_id = $1 ORDER BY kind DESC, source",
    )
    .bind(item)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// What playback-info hands the player: (kind, source, start_ms, end_ms).
async fn served(app: &TestApp, item: Uuid) -> Vec<(String, String, i64, i64)> {
    let info = app
        .get(&format!(
            "/api/v1/items/{item}/playback-info?client=web&network=lan"
        ))
        .await;
    info["segments"]
        .as_array()
        .unwrap_or_else(|| panic!("no segments array: {info}"))
        .iter()
        .map(|s| {
            (
                s["kind"].as_str().unwrap().to_string(),
                s["source"].as_str().unwrap().to_string(),
                s["start_ms"].as_i64().unwrap(),
                s["end_ms"].as_i64().unwrap(),
            )
        })
        .collect()
}

/// Detected edges land within half a second of the fixture's
/// break edges (frame / audio-packet granularity).
fn near(actual: i64, expected_ms: i64) -> bool {
    (actual - expected_ms).abs() <= 500
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL and ffmpeg"]
async fn segment_detection_flow() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test database url");
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let pool = parlour_db::connect(&db_url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");

    // Fixture library.
    let fixtures = PathBuf::from(format!("/tmp/parlour-segdetect-fixtures-{port}"));
    let _ = std::fs::remove_dir_all(&fixtures);
    let media = fixtures.join("media");
    let season = media.join("Test Show").join("Season 1");
    let ep = |n: u32| season.join(format!("Test Show S01E{n:02}.mkv"));
    // E01: chapters name both kinds. Its black + silence breaks
    // would make a heuristic intro, but chapters answered first.
    make_media(
        &ep(1),
        60,
        &[(10, 11), (40, 41)],
        &[
            (0, 5, "Prologue"),
            (5, 20, "Intro"),
            (20, 45, "Part A"),
            (45, 60, "End Credits"),
        ],
    );
    // E02: no chapters; breaks at 10 s and 40 s (intro between them)
    // and at 640 s (credits from there to the end).
    make_media(&ep(2), 700, &[(10, 11), (40, 41), (640, 641)], &[]);
    // E03: a chapter names the intro only; the credits come from the
    // tail heuristic. The head break at 30 s would have made a
    // heuristic intro of 0–30 s — not looked for, chapters answered.
    make_media(
        &ep(3),
        700,
        &[(30, 31), (650, 651)],
        &[(0, 25, "Intro"), (25, 700, "Episode")],
    );
    // E04: nothing to find.
    make_media(&ep(4), 30, &[], &[]);
    // A movie with an intro chapter: SEG2 is for series episodes.
    let movie = media
        .join("Movie Night (2020)")
        .join("Movie Night (2020).mkv");
    make_media(&movie, 30, &[], &[(0, 10, "Intro")]);

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

    let mut state = parlour_server::http::AppState::new(pool.clone(), None, false);
    std::sync::Arc::get_mut(&mut state.playback)
        .expect("fresh playback state")
        .configure(fixtures.join("cache"), None);
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
    // The background workers, including segment detection's own.
    parlour_server::catalog::spawn_background(
        pool.clone(),
        parlour_server::metadata::MetadataSettings::default(),
        parlour_catalog::EmbySettings::default(),
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
    let profiles = app.get("/api/v1/profiles").await;
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
            Some(json!({"name": "Shows", "kind": "shows", "paths": [media.to_str().unwrap()]})),
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

    // ---- First scan: one detection job, for the four episodes.
    assert!(detect_jobs(&app, &library_id).await.is_empty());
    scan(&app, &library_id).await;
    let jobs = detect_jobs(&app, &library_id).await;
    assert_eq!(jobs.len(), 1, "the scan queues detection: {jobs:?}");
    assert_eq!(jobs[0]["payload"]["trigger"], "scan");
    let job = wait_for_job(&app, jobs[0]["id"].as_str().unwrap()).await;
    assert_eq!(job["progress_total"], 4, "{job}");
    assert_eq!(job["errors"], json!([]), "{job}");
    assert_eq!(
        job["result"],
        json!({
            "episodes": 4,
            "chapter_intros": 2,
            "chapter_credits": 1,
            "heuristic_intros": 1,
            "heuristic_credits": 2,
            "nothing_found": 1,
            "failed": 0,
            "changed_during_detection": 0
        }),
        "{job}"
    );

    let item_of = |path: PathBuf| {
        let pool = pool.clone();
        async move {
            let (id,): (Uuid,) = sqlx::query_as("SELECT item_id FROM media_files WHERE path = $1")
                .bind(path.to_str().unwrap())
                .fetch_one(&pool)
                .await
                .unwrap();
            id
        }
    };
    let (e1, e2, e3, e4) = (
        item_of(ep(1)).await,
        item_of(ep(2)).await,
        item_of(ep(3)).await,
        item_of(ep(4)).await,
    );
    let movie_item = item_of(movie.clone()).await;

    // E01: both from the chapters, at 1.0, served as they are.
    assert_eq!(
        rows(&pool, e1).await,
        vec![
            ("intro".into(), "chapters".into(), 5_000, 20_000, 1.0),
            ("credits".into(), "chapters".into(), 45_000, 60_000, 1.0),
        ]
    );
    assert_eq!(
        served(&app, e1).await,
        vec![
            ("intro".into(), "chapters".into(), 5_000, 20_000),
            ("credits".into(), "chapters".into(), 45_000, 60_000),
        ]
    );

    // E02: two heuristic guesses at 0.5 — stored, never served.
    let stored = rows(&pool, e2).await;
    assert_eq!(stored.len(), 2, "{stored:?}");
    let (kind, source, start, end, confidence) = &stored[0];
    assert_eq!((kind.as_str(), source.as_str()), ("intro", "heuristic"));
    assert!(near(*start, 11_100) && near(*end, 40_000), "{stored:?}");
    assert_eq!(*confidence, 0.5);
    let (kind, source, start, end, confidence) = &stored[1];
    assert_eq!((kind.as_str(), source.as_str()), ("credits", "heuristic"));
    assert!(near(*start, 641_100) && near(*end, 700_000), "{stored:?}");
    assert_eq!(*confidence, 0.5);
    assert_eq!(served(&app, e2).await, vec![]);

    // E03: the chapter intro is served; the heuristic credits are
    // not; no heuristic intro was looked for.
    let stored = rows(&pool, e3).await;
    assert_eq!(stored.len(), 2, "{stored:?}");
    assert_eq!(
        stored[0],
        ("intro".into(), "chapters".into(), 0, 25_000, 1.0)
    );
    assert_eq!(
        (stored[1].0.as_str(), stored[1].1.as_str()),
        ("credits", "heuristic")
    );
    assert!(
        near(stored[1].2, 651_100) && near(stored[1].3, 700_000),
        "{stored:?}"
    );
    assert_eq!(
        served(&app, e3).await,
        vec![("intro".into(), "chapters".into(), 0, 25_000)]
    );

    // E04 and the movie: nothing.
    assert!(rows(&pool, e4).await.is_empty());
    assert!(rows(&pool, movie_item).await.is_empty());

    // Every episode file is marked as looked at; the movie never is.
    let pending: Vec<(String,)> =
        sqlx::query_as("SELECT relative_path FROM media_files WHERE segments_detected_at IS NULL")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        pending,
        vec![("Movie Night (2020)/Movie Night (2020).mkv".to_string(),)]
    );

    // ---- Unchanged rescan: nothing new to look at, no new job.
    scan(&app, &library_id).await;
    assert_eq!(detect_jobs(&app, &library_id).await.len(), 1);

    // ---- An admin's edit (SEG4's future job), then two files change:
    // E01 loses its chapters, E02 loses its breaks.
    sqlx::query(
        "INSERT INTO media_segments (item_id, kind, start_ms, end_ms, source, confidence)
         VALUES ($1, 'intro', 12_000, 39_000, 'manual', 0.5)",
    )
    .bind(e2)
    .execute(&pool)
    .await
    .unwrap();
    make_media(&ep(1), 45, &[], &[]);
    make_media(&ep(2), 50, &[], &[]);
    scan(&app, &library_id).await;
    let jobs = detect_jobs(&app, &library_id).await;
    assert_eq!(jobs.len(), 2, "the changed files queue detection: {jobs:?}");
    let job = wait_for_job(&app, jobs[0]["id"].as_str().unwrap()).await;
    assert_eq!(job["result"]["episodes"], 2, "only what changed: {job}");
    assert_eq!(job["result"]["nothing_found"], 2, "{job}");

    // The detector removed its own stale rows...
    assert!(rows(&pool, e1).await.is_empty());
    assert_eq!(served(&app, e1).await, vec![]);
    // ...and left the manual row alone, which is served even at a
    // confidence below the floor.
    assert_eq!(
        rows(&pool, e2).await,
        vec![("intro".into(), "manual".into(), 12_000, 39_000, 0.5)]
    );
    assert_eq!(
        served(&app, e2).await,
        vec![("intro".into(), "manual".into(), 12_000, 39_000)]
    );
    // E03 was not looked at again.
    assert_eq!(rows(&pool, e3).await.len(), 2);

    server.abort();
    let _ = std::fs::remove_dir_all(&fixtures);
}
