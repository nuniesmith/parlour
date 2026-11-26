//! End-to-end test of AB11 (aligning AB1/AB2/AB4 with Audiobookshelf,
//! D28/D29/D33): `Part` folders are books of their own while `Disc`
//! folders still fold; the title folder's name is parsed (sequence,
//! year, narrator, ASIN, and the subtitle behind the library's
//! setting); the author is the nearest author folder in a deep layout;
//! the library's auto-finish rules (10 s by default, or a percentage);
//! finishing and un-finishing by hand; the hide-from-Continue-Listening
//! flag clearing when the position next moves; and the client's own
//! update time stored.
//!
//! Ignored by default; needs PARLOUR_TEST_DATABASE_URL (a fresh
//! database) and ffmpeg/ffprobe on PATH.
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test abs_alignment_flow -- --ignored

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;

use reqwest::header::{COOKIE, SET_COOKIE};
use reqwest::Method;
use serde_json::{json, Value};
use tokio::time::{sleep, Duration};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// An untagged MP3 of `secs` seconds, so the folder source speaks.
fn make_mp3(dir: &Path, secs: u32) {
    std::fs::create_dir_all(dir).unwrap();
    let src = format!("sine=frequency=440:duration={secs}");
    let out = dir.join("01.mp3");
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            &src,
            "-c:a",
            "libmp3lame",
            "-b:a",
            "32k",
            out.to_str().unwrap(),
        ])
        .status()
        .expect("ffmpeg available");
    assert!(status.success(), "ffmpeg failed for {}", out.display());
}

struct TestApp {
    base: String,
    client: reqwest::Client,
    cookie: String,
}

impl TestApp {
    async fn req(&self, method: Method, path: &str, body: Option<Value>) -> (u16, Value) {
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

    async fn get(&self, path: &str) -> (u16, Value) {
        self.req(Method::GET, path, None).await
    }

    async fn ok(&self, path: &str) -> Value {
        let (status, v) = self.get(path).await;
        assert_eq!(status, 200, "{path}: {v}");
        v
    }

    /// PUT progress, expecting 200.
    async fn put(&self, path: &str, body: Value) -> Value {
        let (status, v) = self.req(Method::PUT, path, Some(body.clone())).await;
        assert_eq!(status, 200, "PUT {path} {body}: {v}");
        v
    }
}

fn titles(books: &Value) -> Vec<String> {
    books
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["title"].as_str().unwrap().to_string())
        .collect()
}

fn book<'a>(books: &'a Value, title: &str) -> &'a Value {
    books
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["title"] == title)
        .unwrap_or_else(|| panic!("no book {title} in {books}"))
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL and ffmpeg"]
async fn abs_alignment_flow() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test database url");
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let pool = parlour_db::connect(&db_url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");

    // Library layout (no tags, so every value below is the folder's):
    //   Fantasy/Jane Doe/The Long Road/1. First Steps/
    //   Fantasy/Jane Doe/The Long Road/Book 2 - 1999 - Second Steps {Ann Reader} [B000000002]/
    //   Jane Doe/Big Book/Part 1/, Part 2/     two books (AB11), not discs
    //   Jane Doe/Disc Book/Disc 1/, Disc 2/    one book, two discs
    //   Jane Doe/Title - The Subtitle/
    //   Jane Doe/1994 - Long Listen/           30 s, for progress
    let root = PathBuf::from(format!("/tmp/parlour-ab11-fixtures-{port}"));
    let _ = std::fs::remove_dir_all(&root);
    let road = root.join("Fantasy").join("Jane Doe").join("The Long Road");
    make_mp3(&road.join("1. First Steps"), 1);
    make_mp3(
        &road.join("Book 2 - 1999 - Second Steps {Ann Reader} [B000000002]"),
        1,
    );
    let jane = root.join("Jane Doe");
    make_mp3(&jane.join("Big Book").join("Part 1"), 1);
    make_mp3(&jane.join("Big Book").join("Part 2"), 1);
    make_mp3(&jane.join("Disc Book").join("Disc 1"), 1);
    make_mp3(&jane.join("Disc Book").join("Disc 2"), 1);
    make_mp3(&jane.join("Title - The Subtitle"), 1);
    make_mp3(&jane.join("1994 - Long Listen"), 30);
    let other = PathBuf::from(format!("/tmp/parlour-ab11-movies-{port}"));
    let _ = std::fs::remove_dir_all(&other);
    std::fs::create_dir_all(&other).unwrap();

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
    let state = parlour_server::http::AppState::new(pool.clone(), None, false);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    tokio::spawn(async move {
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
    let (_, profiles) = app.get("/api/v1/profiles").await;
    let profile_id = profiles[0]["id"].as_str().unwrap().to_string();
    let (status, _) = app
        .req(
            Method::POST,
            &format!("/api/v1/profiles/{profile_id}/select"),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 200);

    let (status, lib) = app
        .req(
            Method::POST,
            "/api/v1/admin/libraries",
            Some(json!({"name": "Audiobooks", "kind": "audiobooks",
                        "paths": [root.to_str().unwrap()]})),
        )
        .await;
    assert_eq!(status, 201, "{lib}");
    let library_id = lib["id"].as_str().unwrap().to_string();
    let library_uuid = uuid::Uuid::parse_str(&library_id).unwrap();
    let (status, _) = app
        .req(
            Method::POST,
            &format!("/api/v1/admin/libraries/{library_id}/access"),
            Some(json!({"profile_id": profile_id})),
        )
        .await;
    assert_eq!(status, 204);
    let scan = || parlour_catalog::scan_library(&pool, library_uuid, None);
    let report = scan().await.expect("scan");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(report.seen_files, 8);

    // ---------------------------------------------- folders (items 1, 3, 4)
    let books = app.ok("/api/v1/audiobooks").await;
    assert_eq!(
        titles(&books),
        [
            "Disc Book",
            "First Steps",
            "Long Listen",
            "Part 1",
            "Part 2",
            "Second Steps",
            "Title - The Subtitle",
        ]
    );
    // Item 1: `Disc n` folds; `Part n` is a folder of its own.
    assert_eq!(book(&books, "Disc Book")["track_count"], 2);
    for part in ["Part 1", "Part 2"] {
        let b = book(&books, part);
        assert_eq!(b["track_count"], 1, "{b}");
        assert_eq!(b["authors"], json!(["Jane Doe"]));
        assert_eq!(b["series"][0]["name"], "Big Book");
        assert_eq!(b["series"][0]["sequence"], Value::Null);
    }
    // Items 3 and 4: the title folder parsed, the nearest author.
    let second = book(&books, "Second Steps");
    assert_eq!(second["authors"], json!(["Jane Doe"]), "not \"Fantasy\"");
    assert_eq!(second["narrators"], json!(["Ann Reader"]));
    assert_eq!(second["year"], 1999);
    assert_eq!(second["asin"], "B000000002");
    assert_eq!(second["series"][0]["name"], "The Long Road");
    assert_eq!(second["series"][0]["sequence"], "2");
    let first = book(&books, "First Steps");
    assert_eq!(first["authors"], json!(["Jane Doe"]));
    assert_eq!(first["series"][0]["sequence"], "1");
    let long = book(&books, "Long Listen");
    assert_eq!(long["year"], 1994);
    assert_eq!(long["series"], json!([]));
    assert_eq!(
        book(&books, "Title - The Subtitle")["subtitle"],
        Value::Null,
        "subtitles are off by default"
    );
    // A folder series now has its sequence, so its page is in order.
    let road_id = second["series"][0]["id"].as_str().unwrap().to_string();
    let road_page = app.ok(&format!("/api/v1/series/{road_id}")).await;
    assert_eq!(titles(&road_page["books"]), ["First Steps", "Second Steps"]);
    // The library list's "Fantasy" credits nobody.
    let authors = app
        .ok(&format!("/api/v1/authors?library_id={library_id}"))
        .await;
    let names: Vec<&str> = authors
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Jane Doe"]);

    let report = scan().await.expect("rescan");
    assert_eq!(report.probed_files, 0, "a re-scan changes nothing");
    assert_eq!(report.created_items, 0);

    // ------------------------------------------------- library settings
    let settings_path = format!("/api/v1/admin/libraries/{library_id}/audiobook-settings");
    let s = app.ok(&settings_path).await;
    assert_eq!(s["parse_subtitles"], false);
    assert_eq!(s["finish_remaining_seconds"], 10);
    assert_eq!(s["finish_percent"], Value::Null);
    assert_eq!(
        s["default"],
        json!({"parse_subtitles": false, "finish_remaining_seconds": 10, "finish_percent": null})
    );
    for bad in [
        json!({"parse_subtitles": false, "finish_remaining_seconds": -1, "finish_percent": null}),
        json!({"parse_subtitles": false, "finish_remaining_seconds": 10, "finish_percent": 0}),
        json!({"parse_subtitles": false, "finish_remaining_seconds": 10, "finish_percent": 101}),
    ] {
        let (status, _) = app
            .req(Method::PUT, &settings_path, Some(bad.clone()))
            .await;
        assert_eq!(status, 400, "{bad}");
    }
    let (status, _) = app
        .req(
            Method::PUT,
            &settings_path,
            Some(json!({"parse_subtitles": true})),
        )
        .await;
    assert!((400..500).contains(&status), "all three are required");
    let (status, movies) = app
        .req(
            Method::POST,
            "/api/v1/admin/libraries",
            Some(json!({"name": "Movies", "kind": "movies",
                        "paths": [other.to_str().unwrap()]})),
        )
        .await;
    assert_eq!(status, 201, "{movies}");
    let (status, _) = app
        .get(&format!(
            "/api/v1/admin/libraries/{}/audiobook-settings",
            movies["id"].as_str().unwrap()
        ))
        .await;
    assert_eq!(status, 400, "a movies library has no audiobook settings");

    // Subtitles on: the next scan merges every book again, once.
    let s = app
        .put(
            &settings_path,
            json!({"parse_subtitles": true, "finish_remaining_seconds": 10,
                   "finish_percent": null}),
        )
        .await;
    assert_eq!(s["parse_subtitles"], true);
    let report = scan().await.expect("rescan");
    assert_eq!(report.probed_files, 8, "every book merged again");
    assert_eq!(report.created_items, 0);
    let books = app.ok("/api/v1/audiobooks").await;
    let sub = book(&books, "Title");
    assert_eq!(sub["subtitle"], "The Subtitle");
    let report = scan().await.expect("rescan");
    assert_eq!(report.probed_files, 0);

    // ---------------------------------------- auto-finish (item 8)
    let long_id = long["id"].as_str().unwrap().to_string();
    let d = long["duration_ms"].as_i64().unwrap();
    assert!((29_500..=30_500).contains(&d), "~30 s: {d}");
    let progress = format!("/api/v1/audiobooks/{long_id}/progress");
    // Exactly 10 s left is not less than 10 s.
    let p = app.put(&progress, json!({"position_ms": d - 10_000})).await;
    assert_eq!(p["finished"], false, "{p}");
    let p = app.put(&progress, json!({"position_ms": d - 9_999})).await;
    assert_eq!(p["finished"], true, "{p}");
    assert!(p["finished_at"].is_string());
    // Moved back: unfinished again, as ABS does.
    let p = app.put(&progress, json!({"position_ms": 1_000})).await;
    assert_eq!(p["finished"], false);
    assert_eq!(p["finished_at"], Value::Null);

    // A percentage replaces the time rule.
    app.put(
        &settings_path,
        json!({"parse_subtitles": true, "finish_remaining_seconds": 10, "finish_percent": 99}),
    )
    .await;
    let p = app.put(&progress, json!({"position_ms": d - 5_000})).await;
    assert_eq!(p["finished"], false, "5 s left, but under 99 %");
    app.put(
        &settings_path,
        json!({"parse_subtitles": true, "finish_remaining_seconds": 10, "finish_percent": 50}),
    )
    .await;
    let p = app.put(&progress, json!({"position_ms": d / 2})).await;
    assert_eq!(p["finished"], false, "half is not past half");
    let p = app.put(&progress, json!({"position_ms": d / 2 + 1})).await;
    assert_eq!(p["finished"], true);
    // The time rule off (0) and no percentage: positions never finish.
    app.put(
        &settings_path,
        json!({"parse_subtitles": true, "finish_remaining_seconds": 0, "finish_percent": null}),
    )
    .await;
    let p = app.put(&progress, json!({"finished": false})).await;
    assert_eq!(p["position_ms"], 0, "un-finishing by hand starts over");
    let p = app.put(&progress, json!({"position_ms": d})).await;
    assert_eq!(p["finished"], false, "rule off");
    // By hand: finishing keeps the position.
    let p = app.put(&progress, json!({"finished": true})).await;
    assert_eq!(
        (p["finished"].clone(), p["position_ms"].clone()),
        (json!(true), json!(d))
    );
    let p = app
        .put(&progress, json!({"finished": false, "position_ms": 7_000}))
        .await;
    assert_eq!(
        (p["finished"].clone(), p["position_ms"].clone()),
        (json!(false), json!(0)),
        "un-finishing ignores a position sent with it"
    );
    app.put(
        &settings_path,
        json!({"parse_subtitles": true, "finish_remaining_seconds": 10, "finish_percent": null}),
    )
    .await;

    // ------------------------------------------------ hide flag (item 9)
    app.put(&progress, json!({"position_ms": 4_000})).await;
    let p = app.put(&progress, json!({"hidden": true})).await;
    assert_eq!(p["hidden"], true);
    assert_eq!(p["position_ms"], 4_000, "hiding moves nothing");
    let books = app.ok("/api/v1/audiobooks").await;
    assert_eq!(book(&books, "Long Listen")["progress"]["hidden"], true);
    let sync = app.ok("/api/v1/audiobooks/progress").await;
    assert_eq!(sync["progress"][0]["hidden"], true);
    let p = app.put(&progress, json!({"position_ms": 4_000})).await;
    assert_eq!(p["hidden"], true, "the same position keeps it hidden");
    let p = app.put(&progress, json!({"finished": true})).await;
    assert_eq!(p["hidden"], true, "finishing alone doesn't clear it");
    let p = app.put(&progress, json!({"finished": false})).await;
    assert_eq!(
        (p["hidden"].clone(), p["position_ms"].clone()),
        (json!(false), json!(0)),
        "un-finishing moves the position, which clears it"
    );
    app.put(&progress, json!({"position_ms": 4_000, "hidden": true}))
        .await;
    let p = app.put(&progress, json!({"position_ms": 5_000})).await;
    assert_eq!(p["hidden"], false, "the next move clears it");
    app.put(&progress, json!({"hidden": true})).await;
    let p = app.put(&progress, json!({"hidden": false})).await;
    assert_eq!(p["hidden"], false, "shown again by hand");

    // ------------------------------------------ client timestamp (item 10)
    let since = app.ok("/api/v1/audiobooks/progress").await["server_time"]
        .as_str()
        .unwrap()
        .to_string();
    sleep(Duration::from_millis(20)).await;
    let p = app
        .put(
            &progress,
            json!({"position_ms": 6_000, "client_updated_at": "2026-11-20T08:00:00Z"}),
        )
        .await;
    assert_eq!(p["client_updated_at"], "2026-11-20T08:00:00Z");
    assert_ne!(p["updated_at"], p["client_updated_at"]);
    // The sync cursor is still the server's time, so an older client
    // clock never hides a write from the next pull.
    let sync = app
        .ok(&format!(
            "/api/v1/audiobooks/progress?since={}",
            since.replace('+', "%2B")
        ))
        .await;
    assert_eq!(sync["progress"].as_array().unwrap().len(), 1, "{sync}");
    assert_eq!(
        sync["progress"][0]["client_updated_at"],
        "2026-11-20T08:00:00Z"
    );
    // Without one, the server's time stands in.
    let p = app.put(&progress, json!({"position_ms": 6_500})).await;
    assert_eq!(p["client_updated_at"], p["updated_at"]);
    // Last arrival still wins: an older client time is stored as sent.
    let p = app
        .put(
            &progress,
            json!({"position_ms": 2_000, "client_updated_at": "2026-01-01T00:00:00Z"}),
        )
        .await;
    assert_eq!(p["position_ms"], 2_000);
    assert_eq!(p["client_updated_at"], "2026-01-01T00:00:00Z");
    let (status, _) = app
        .req(
            Method::PUT,
            &progress,
            Some(json!({"position_ms": 1, "client_updated_at": "yesterday"})),
        )
        .await;
    assert!((400..500).contains(&status), "a bad time is refused");
    let p = app.ok(&progress).await;
    assert_eq!(p["position_ms"], 2_000, "nothing stored");
    let (status, _) = app.req(Method::PUT, &progress, Some(json!({}))).await;
    assert_eq!(status, 400);

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&other);
}
