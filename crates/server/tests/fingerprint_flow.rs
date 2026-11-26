//! End-to-end test of SEG3 (Phase 9; D15, D20): after a scan, the
//! `detect_segments` job fingerprints each season's episodes (ffmpeg
//! chromaprint), caches the prints by file hash, and stores the intro
//! and credits the season shares as `fingerprint` rows at
//! quality × agreement — only where the chapters left a kind open,
//! never for a season with fewer than 3 usable episodes, and never
//! touching manual rows. A changed episode re-matches its whole season
//! from the cache, and the old file's fingerprint is deleted.
//!
//! The fixtures are synthetic: tunes whose notes come from a hash of
//! the note's index (so nothing repeats by accident), and per-episode
//! audio of shaped pink noise under the episode's own tune.
//!
//! Ignored by default; needs PARLOUR_TEST_DATABASE_URL (a fresh
//! database of its own) and ffmpeg/ffprobe on PATH.
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test fingerprint_flow -- --ignored

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

/// A note (0–11 semitones) picked by hashing its index: a sequence
/// that never repeats, as real music does not loop exactly.
fn note(rate: f64, seed: u32) -> String {
    let h = format!("sin(floor(t*{rate})*12.9898+{seed}*78.233)*43758.5453");
    format!("floor(12*(({h})-floor({h})))")
}

/// One piece of an episode's audio.
enum Part {
    /// Music every episode of the show shares (intro or credits).
    Tune(u32, u32),
    /// The episode's own audio: speech-shaped pink noise under a
    /// quiet tune of its own.
    Own(u32, u32),
}

const FORMAT: &str = "aformat=sample_fmts=fltp:sample_rates=22050:channel_layouts=mono";

/// An episode whose sound is `parts` (seed, seconds) over a small
/// test picture, with optional chapters (start s, end s, title).
fn make_episode(path: &Path, parts: &[Part], chapters: &[(u32, u32, &str)]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let seconds: u32 = parts
        .iter()
        .map(|p| match *p {
            Part::Tune(_, secs) | Part::Own(_, secs) => secs,
        })
        .sum();
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-v".into(),
        "error".into(),
        "-f".into(),
        "lavfi".into(),
        "-i".into(),
        format!("testsrc=s=160x120:r=2:d={seconds}"),
    ];
    let mut graph = Vec::new();
    let mut labels = String::new();
    let mut input = 1; // 0 is the picture
    for (k, part) in parts.iter().enumerate() {
        match *part {
            Part::Tune(seed, secs) => {
                let tune = format!(
                    "0.25*sin(2*PI*t*220*pow(2,{}/12))+0.15*sin(2*PI*t*440*pow(2,{}/12))",
                    note(2.0, seed),
                    note(3.0, seed + 1)
                );
                args.extend([
                    "-f".into(),
                    "lavfi".into(),
                    "-i".into(),
                    format!("aevalsrc='{tune}':s=22050:d={secs}"),
                ]);
                graph.push(format!("[{input}]{FORMAT}[p{k}]"));
                input += 1;
            }
            Part::Own(seed, secs) => {
                let own = format!("0.08*sin(2*PI*t*196*pow(2,{}/12))", note(1.5, seed));
                args.extend([
                    "-f".into(),
                    "lavfi".into(),
                    "-i".into(),
                    format!("anoisesrc=d={secs}:c=pink:r=22050:a=0.3:seed={seed}"),
                    "-f".into(),
                    "lavfi".into(),
                    "-i".into(),
                    format!("aevalsrc='{own}':s=22050:d={secs}"),
                ]);
                graph.push(format!(
                    "[{input}]volume='0.6+0.4*sin(2*PI*4.3*t+{seed})':eval=frame[n{k}];\
                     [n{k}][{}]amix=inputs=2:normalize=0,{FORMAT}[p{k}]",
                    input + 1
                ));
                input += 2;
            }
        }
        labels.push_str(&format!("[p{k}]"));
    }
    graph.push(format!("{labels}concat=n={}:v=0:a=1[out]", parts.len()));
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
        args.extend([
            "-i".into(),
            meta.to_str().unwrap().into(),
            "-map_metadata".into(),
            input.to_string(),
            "-map_chapters".into(),
            input.to_string(),
        ]);
    }
    args.extend([
        "-filter_complex".into(),
        graph.join(";"),
        "-map".into(),
        "0:v".into(),
        "-map".into(),
        "[out]".into(),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "ultrafast".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "96k".into(),
        path.to_str().unwrap().into(),
    ]);
    let out = Command::new("ffmpeg")
        .args(&args)
        .output()
        .expect("ffmpeg available");
    assert!(
        out.status.success(),
        "ffmpeg failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_file(&meta);
}

/// Every episode lasts 240 s: the head window (25 %) is the first
/// 60 s and the tail window (the last 600 s) is the whole file.
const LENGTH: u32 = 240;
const INTRO: u32 = 30;
const CREDITS: u32 = 20;
const PREVIEW: u32 = 10;
const INTRO_TUNE: u32 = 11;
const CREDITS_TUNE: u32 = 23;

/// A cold open of `cold` s, the shared intro, the episode, the shared
/// credits, then a preview of its own. `seed` keeps its own audio
/// unique.
fn standard_episode(path: &Path, seed: u32, cold: u32, chapters: &[(u32, u32, &str)]) {
    let mut parts = Vec::new();
    if cold > 0 {
        parts.push(Part::Own(seed, cold));
    }
    parts.push(Part::Tune(INTRO_TUNE, INTRO));
    parts.push(Part::Own(
        seed + 1,
        LENGTH - cold - INTRO - CREDITS - PREVIEW,
    ));
    parts.push(Part::Tune(CREDITS_TUNE, CREDITS));
    parts.push(Part::Own(seed + 2, PREVIEW));
    make_episode(path, &parts, chapters);
}

/// Where the shared credits sit in every episode, in ms.
const CREDITS_AT: (i64, i64) = (
    (LENGTH - PREVIEW - CREDITS) as i64 * 1000,
    (LENGTH - PREVIEW) as i64 * 1000,
);

/// Fingerprint edges land within 2 s of the true ones (D20's
/// measurements; they tend to come out early, never by more).
fn near(actual: i64, expected_ms: i64) -> bool {
    (actual - expected_ms).abs() <= 2_000
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
    for _ in 0..600 {
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

/// Scan, then wait for the detection job that scan queued.
async fn scan_and_detect(app: &TestApp, library_id: &str) -> Value {
    let before = detect_jobs(app, library_id).await.len();
    scan(app, library_id).await;
    let jobs = detect_jobs(app, library_id).await;
    assert_eq!(
        jobs.len(),
        before + 1,
        "the scan queues detection: {jobs:?}"
    );
    let job = wait_for_job(app, jobs[0]["id"].as_str().unwrap()).await;
    assert_eq!(job["errors"], json!([]), "{job}");
    job
}

/// A stored row: (start_ms, end_ms, confidence).
type Row = (i64, i64, f32);

/// An item's stored row of one kind and source, if any.
async fn row(pool: &sqlx::PgPool, item: Uuid, kind: &str, source: &str) -> Option<Row> {
    sqlx::query_as(
        "SELECT start_ms, end_ms, confidence FROM media_segments
         WHERE item_id = $1 AND kind = $2 AND source = $3",
    )
    .bind(item)
    .bind(kind)
    .bind(source)
    .fetch_optional(pool)
    .await
    .unwrap()
}

/// How many rows of a source the item has.
async fn count(pool: &sqlx::PgPool, item: Uuid, source: &str) -> i64 {
    let (n,): (i64,) =
        sqlx::query_as("SELECT count(*) FROM media_segments WHERE item_id = $1 AND source = $2")
            .bind(item)
            .bind(source)
            .fetch_one(pool)
            .await
            .unwrap();
    n
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

async fn cached_prints(pool: &sqlx::PgPool) -> i64 {
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM audio_fingerprints")
        .fetch_one(pool)
        .await
        .unwrap();
    n
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL and ffmpeg"]
async fn fingerprint_flow() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test database url");
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let pool = parlour_db::connect(&db_url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");

    let fixtures = PathBuf::from(format!("/tmp/parlour-fingerprint-fixtures-{port}"));
    let _ = std::fs::remove_dir_all(&fixtures);
    let media = fixtures.join("media");
    let ep = |season: u32, n: u32| {
        media
            .join("Test Show")
            .join(format!("Season {season}"))
            .join(format!("Test Show S{season:02}E{n:02}.mkv"))
    };
    // Season 1: five episodes share the intro (after cold opens of
    // different lengths) and the credits. E05's chapters name its
    // intro, so it gets no fingerprint intro — but its audio still
    // helps the others agree.
    let colds = [0, 5, 12, 20, 8];
    for (i, &cold) in colds.iter().enumerate() {
        let n = i as u32 + 1;
        let chapters: &[(u32, u32, &str)] = if n == 5 {
            &[
                (0, 8, "Prologue"),
                (8, 38, "Opening"),
                (38, LENGTH, "Episode"),
            ]
        } else {
            &[]
        };
        standard_episode(&ep(1, n), 1000 + 10 * n, cold, chapters);
    }
    // Season 2: the same intro and credits, but only two episodes —
    // too few to agree on anything (D20).
    standard_episode(&ep(2, 1), 2010, 3, &[]);
    standard_episode(&ep(2, 2), 2020, 15, &[]);

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
    parlour_server::catalog::spawn_background(
        pool.clone(),
        parlour_server::metadata::MetadataSettings::default(),
        parlour_catalog::EmbySettings::default(),
        None,
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

    // ---- First scan: season 1 is matched, season 2 is too small.
    let job = scan_and_detect(&app, &library_id).await;
    assert_eq!(
        job["result"],
        json!({
            "episodes": 7,
            "chapter_intros": 1,
            "chapter_credits": 0,
            "heuristic_intros": 0,
            "heuristic_credits": 0,
            "nothing_found": 2,
            "failed": 0,
            "changed_during_detection": 0,
            "seasons_matched": 1,
            "seasons_skipped": 1,
            "fingerprinted": 5,
            "fingerprints_reused": 0,
            "fingerprint_failed": 0,
            "fingerprint_intros": 4,
            "fingerprint_credits": 5,
            "fingerprints_removed": 0
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
    let mut s1 = Vec::new();
    for n in 1..=5 {
        s1.push(item_of(ep(1, n)).await);
    }
    let s2 = [item_of(ep(2, 1)).await, item_of(ep(2, 2)).await];

    // E01–E04: the intro where each episode put it and the credits,
    // shared with all four other episodes (agreement 15/16).
    for (i, &item) in s1.iter().enumerate() {
        let cold = colds[i] as i64 * 1000;
        let intro = row(&pool, item, "intro", "fingerprint").await;
        if i < 4 {
            let (start, end, confidence) = intro.unwrap_or_else(|| panic!("E0{}: no intro", i + 1));
            assert!(
                near(start, cold) && near(end, cold + INTRO as i64 * 1000),
                "E0{}: intro {start}–{end}, true {cold}–{}",
                i + 1,
                cold + INTRO as i64 * 1000
            );
            assert!(
                confidence > 0.875 && confidence <= 0.9375,
                "E0{}: {confidence}",
                i + 1
            );
        } else {
            assert_eq!(intro, None, "E05's chapters answered the intro");
        }
        let (start, end, confidence) = row(&pool, item, "credits", "fingerprint")
            .await
            .unwrap_or_else(|| panic!("E0{}: no credits", i + 1));
        assert!(
            near(start, CREDITS_AT.0) && near(end, CREDITS_AT.1),
            "E0{}: credits {start}–{end}",
            i + 1
        );
        assert!(confidence > 0.875, "E0{}: {confidence}", i + 1);
    }
    // E05 keeps its chapter intro, which is what the player gets.
    assert_eq!(
        row(&pool, s1[4], "intro", "chapters").await,
        Some((8_000, 38_000, 1.0))
    );
    let e5 = served(&app, s1[4]).await;
    assert_eq!(
        e5[0],
        ("intro".into(), "chapters".into(), 8_000, 38_000),
        "{e5:?}"
    );
    assert_eq!(
        (e5[1].0.as_str(), e5[1].1.as_str()),
        ("credits", "fingerprint")
    );
    // The player gets E01's fingerprint segments as stored.
    let (start, end, _) = row(&pool, s1[0], "intro", "fingerprint").await.unwrap();
    let e1 = served(&app, s1[0]).await;
    assert_eq!(e1.len(), 2, "{e1:?}");
    assert_eq!(e1[0], ("intro".into(), "fingerprint".into(), start, end));
    assert_eq!(e1[1].1, "fingerprint");

    // Season 2: nothing, though its episodes share the same music.
    for &item in &s2 {
        assert_eq!(count(&pool, item, "fingerprint").await, 0);
        assert_eq!(served(&app, item).await, vec![]);
    }
    // Five fingerprints cached, each linked from its file's row.
    assert_eq!(cached_prints(&pool).await, 5);
    let (linked,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM media_files f
         JOIN audio_fingerprints a ON a.file_hash = f.file_hash",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(linked, 5);

    // ---- Unchanged rescan: nothing to look at, no new job.
    let before = detect_jobs(&app, &library_id).await.len();
    scan(&app, &library_id).await;
    assert_eq!(detect_jobs(&app, &library_id).await.len(), before);

    // ---- An admin's intro on E02 (SEG4's future job), then E04 is
    // replaced by a cut without the show's music.
    sqlx::query(
        "INSERT INTO media_segments (item_id, kind, start_ms, end_ms, source, confidence)
         VALUES ($1, 'intro', 4_000, 36_000, 'manual', 1.0)",
    )
    .bind(s1[1])
    .execute(&pool)
    .await
    .unwrap();
    make_episode(&ep(1, 4), &[Part::Own(1990, LENGTH)], &[]);
    let job = scan_and_detect(&app, &library_id).await;
    let result = &job["result"];
    assert_eq!(result["episodes"], 1, "{job}");
    assert_eq!(result["nothing_found"], 1, "E04 shares nothing now: {job}");
    assert_eq!(result["seasons_matched"], 1, "{job}");
    // The season was re-matched as a whole: only E04 needed ffmpeg.
    assert_eq!(result["fingerprinted"], 1, "{job}");
    assert_eq!(result["fingerprints_reused"], 4, "{job}");
    assert_eq!(result["fingerprint_intros"], 3, "{job}");
    assert_eq!(result["fingerprint_credits"], 4, "{job}");
    // E04's old fingerprint is referenced by nothing now: deleted.
    assert_eq!(result["fingerprints_removed"], 1, "{job}");
    assert_eq!(cached_prints(&pool).await, 5);

    // E04 lost its fingerprint rows; the others now share with three
    // episodes (agreement 7/8).
    assert_eq!(count(&pool, s1[3], "fingerprint").await, 0);
    for &item in &[s1[0], s1[1], s1[2]] {
        let (_, _, confidence) = row(&pool, item, "intro", "fingerprint").await.unwrap();
        assert!(confidence > 0.75 && confidence <= 0.875, "{confidence}");
    }
    // The manual intro was left alone, and it is what the player gets.
    assert_eq!(
        row(&pool, s1[1], "intro", "manual").await,
        Some((4_000, 36_000, 1.0))
    );
    assert_eq!(
        served(&app, s1[1]).await[0],
        ("intro".into(), "manual".into(), 4_000, 36_000)
    );

    // ---- Season 2 gets its third episode: now it can agree.
    standard_episode(&ep(2, 3), 2030, 9, &[]);
    let job = scan_and_detect(&app, &library_id).await;
    let result = &job["result"];
    assert_eq!(result["episodes"], 1, "{job}");
    assert_eq!(result["seasons_matched"], 1, "{job}");
    assert_eq!(result["seasons_skipped"], 0, "{job}");
    assert_eq!(
        result["fingerprinted"], 3,
        "never fingerprinted before: {job}"
    );
    assert_eq!(result["fingerprint_intros"], 3, "{job}");
    assert_eq!(result["fingerprint_credits"], 3, "{job}");
    let s2_e3 = item_of(ep(2, 3)).await;
    for (item, cold) in [(s2[0], 3), (s2[1], 15), (s2_e3, 9)] {
        let (start, end, confidence) = row(&pool, item, "intro", "fingerprint").await.unwrap();
        assert!(
            near(start, cold * 1000) && near(end, (cold + INTRO as i64) * 1000),
            "{start}–{end}, true from {cold} s"
        );
        // Two other episodes share it: agreement 3/4.
        assert!((0.6..=0.75).contains(&confidence), "{confidence}");
    }
    assert_eq!(cached_prints(&pool).await, 8);
    // Season 1 was not touched: its rows are as the last match left them.
    assert_eq!(count(&pool, s1[0], "fingerprint").await, 2);

    server.abort();
    let _ = std::fs::remove_dir_all(&fixtures);
}
