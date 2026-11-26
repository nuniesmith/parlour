//! End-to-end test of Phase 10's first slice (AB1–AB4): an
//! audiobooks library scanned into books (grouping, disc folders,
//! track order, tag metadata, chapters, folder covers), the native
//! `/api/v1/audiobooks` API, Range streaming of tracks, and
//! per-profile progress (get / put / delete / bulk sync), with
//! library access enforced and re-scans idempotent.
//!
//! Ignored by default; needs PARLOUR_TEST_DATABASE_URL (a fresh
//! database) and ffmpeg/ffprobe on PATH.
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test audiobook_flow -- --ignored

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;

use reqwest::header::{COOKIE, RANGE, SET_COOKIE};
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

fn ffmpeg(args: &[&str]) {
    let status = Command::new("ffmpeg")
        .args(args)
        .status()
        .expect("ffmpeg available");
    assert!(status.success(), "ffmpeg failed: {args:?}");
}

/// A tagged MP3 of `secs` seconds of tone.
fn make_mp3(dir: &Path, name: &str, secs: u32, tags: &[(&str, &str)]) {
    std::fs::create_dir_all(dir).unwrap();
    let src = format!("sine=frequency=440:duration={secs}");
    let out = dir.join(name);
    let mut args: Vec<String> = ["-y", "-v", "error", "-f", "lavfi", "-i", &src]
        .iter()
        .map(|s| s.to_string())
        .collect();
    for (k, v) in tags {
        args.push("-metadata".into());
        args.push(format!("{k}={v}"));
    }
    args.extend(["-c:a", "libmp3lame", "-b:a", "64k"].map(String::from));
    args.push(out.to_str().unwrap().to_string());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    ffmpeg(&refs);
}

/// A single-file .m4b with two embedded chapters.
fn make_m4b(dir: &Path, name: &str) {
    std::fs::create_dir_all(dir).unwrap();
    let meta = dir.join("m4b-meta.txt");
    std::fs::write(
        &meta,
        ";FFMETADATA1\ntitle=The Single\nartist=Solo Writer\n\
         [CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=1500\ntitle=Prologue\n\
         [CHAPTER]\nTIMEBASE=1/1000\nSTART=1500\nEND=4000\ntitle=The End\n",
    )
    .unwrap();
    ffmpeg(&[
        "-y",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=330:duration=4",
        "-i",
        meta.to_str().unwrap(),
        "-map",
        "0:a",
        "-map_metadata",
        "1",
        "-map_chapters",
        "1",
        "-c:a",
        "aac",
        "-f",
        "mp4",
        dir.join(name).to_str().unwrap(),
    ]);
    std::fs::remove_file(meta).unwrap();
}

fn make_cover(path: &Path) {
    ffmpeg(&[
        "-y",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=red:s=64x64",
        "-frames:v",
        "1",
        path.to_str().unwrap(),
    ]);
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

    /// GET returning (status, body bytes, content-range, content-type),
    /// with or without the session cookie.
    async fn raw(
        &self,
        path: &str,
        range: Option<&str>,
        with_cookie: bool,
    ) -> (u16, Vec<u8>, String, String) {
        let mut builder = self.client.get(format!("{}{}", self.base, path));
        if with_cookie {
            builder = builder.header(COOKIE, &self.cookie);
        }
        if let Some(r) = range {
            builder = builder.header(RANGE, r);
        }
        let resp = builder.send().await.expect("raw request");
        let status = resp.status().as_u16();
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string()
        };
        let content_range = header("content-range");
        let content_type = header("content-type");
        let bytes = resp.bytes().await.unwrap_or_default().to_vec();
        (status, bytes, content_range, content_type)
    }
}

async fn wait_for_job(app: &TestApp, job_id: &str) {
    for _ in 0..120 {
        let (_, job) = app.get(&format!("/api/v1/admin/jobs/{job_id}")).await;
        match job["status"].as_str() {
            Some("succeeded") => return,
            Some("failed") => panic!("job failed: {job}"),
            _ => sleep(Duration::from_millis(500)).await,
        }
    }
    panic!("job did not finish");
}

fn book<'a>(books: &'a Value, title: &str) -> &'a Value {
    books
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["title"] == title)
        .unwrap_or_else(|| panic!("no book {title} in {books}"))
}

fn titles(books: &Value) -> Vec<String> {
    books
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["title"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL and ffmpeg"]
async fn audiobook_flow() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test database url");
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let pool = parlour_db::connect(&db_url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");

    // Library layout (AB11 kept it: `CD n` folders still fold; `Part`
    // folders are AB11's own flow, abs_alignment_flow):
    //   Jane Author/Book One/01 - Opening.mp3, 02 - Middle.mp3, cover.jpg
    //   Jane Author/Big Book/CD 1/Zed.mp3, CD 2/Alpha.mp3  (one book)
    //   Single.m4b                                          (root file)
    let root = PathBuf::from(format!("/tmp/parlour-audiobook-fixtures-{port}"));
    let _ = std::fs::remove_dir_all(&root);
    let book_one = root.join("Jane Author").join("Book One");
    let common = [
        ("album", "Book One"),
        ("album_artist", "Jane Author"),
        ("artist", "Someone Else"),
        ("composer", "Nick Narrator"),
        ("date", "2019"),
        ("genre", "Fantasy; Adventure"),
    ];
    let mut t1 = common.to_vec();
    t1.extend([("title", "Opening"), ("track", "1/2")]);
    let mut t2 = common.to_vec();
    t2.extend([("title", "Middle"), ("track", "2/2")]);
    make_mp3(&book_one, "01 - Opening.mp3", 3, &t1);
    make_mp3(&book_one, "02 - Middle.mp3", 2, &t2);
    make_cover(&book_one.join("cover.jpg"));
    let big_dir = root.join("Jane Author").join("Big Book");
    // Disc folders decide the order even though "Alpha" < "Zed".
    make_mp3(
        &big_dir.join("CD 1"),
        "Zed.mp3",
        2,
        &[("album", "Big Book")],
    );
    make_mp3(
        &big_dir.join("CD 2"),
        "Alpha.mp3",
        2,
        &[("album", "Big Book")],
    );
    make_m4b(&root, "Single.m4b");

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

    // ------------------------------------------------ library + scan
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
    let (status, _) = app
        .req(
            Method::POST,
            &format!("/api/v1/admin/libraries/{library_id}/access"),
            Some(json!({"profile_id": profile_id})),
        )
        .await;
    assert_eq!(status, 204);
    let (status, job) = app
        .req(
            Method::POST,
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
    let (_, job) = app
        .get(&format!(
            "/api/v1/admin/jobs/{}",
            job["id"].as_str().unwrap()
        ))
        .await;
    assert_eq!(job["errors"], json!([]), "{job}");

    // ------------------------------------------------------- list
    let (status, books) = app.get("/api/v1/audiobooks").await;
    assert_eq!(status, 200, "{books}");
    assert_eq!(titles(&books), ["Big Book", "Book One", "The Single"]);

    let one = book(&books, "Book One");
    assert_eq!(
        one["authors"],
        json!(["Jane Author"]),
        "album_artist beats artist"
    );
    assert_eq!(one["narrators"], json!(["Nick Narrator"]));
    assert_eq!(one["year"], 2019);
    assert_eq!(one["genres"], json!(["Fantasy", "Adventure"]));
    assert_eq!(one["track_count"], 2);
    assert_eq!(one["chapter_count"], 2);
    assert_eq!(one["has_cover"], true);
    let one_len = one["duration_ms"].as_i64().unwrap();
    assert!(
        (4900..=5300).contains(&one_len),
        "Book One is ~5 s: {one_len}"
    );
    assert_eq!(one["progress"]["position_ms"], 0);
    assert_eq!(one["progress"]["finished"], false);
    assert_eq!(one["progress"]["updated_at"], Value::Null);
    let one_id = one["id"].as_str().unwrap().to_string();

    let big = book(&books, "Big Book");
    assert_eq!(
        big["authors"],
        json!(["Jane Author"]),
        "the author folder (the nearest; AB11)"
    );
    assert_eq!(big["track_count"], 2);
    assert_eq!(big["has_cover"], false);
    let big_id = big["id"].as_str().unwrap().to_string();

    let single = book(&books, "The Single");
    assert_eq!(single["authors"], json!(["Solo Writer"]));
    assert_eq!(single["track_count"], 1);
    assert_eq!(single["chapter_count"], 2);
    let single_id = single["id"].as_str().unwrap().to_string();

    // Filters and sorts.
    let (_, found) = app.get("/api/v1/audiobooks?q=narrator").await;
    assert_eq!(titles(&found), ["Book One"], "q matches narrators");
    let (_, found) = app.get("/api/v1/audiobooks?q=single").await;
    assert_eq!(titles(&found), ["The Single"]);
    let (_, found) = app
        .get(&format!("/api/v1/audiobooks?library_id={library_id}"))
        .await;
    assert_eq!(found.as_array().unwrap().len(), 3);
    let (status, _) = app.get("/api/v1/audiobooks?sort=nope").await;
    assert_eq!(status, 400);

    // The generic items list shows the books as audiobook items.
    let (_, items) = app
        .get(&format!("/api/v1/libraries/{library_id}/items"))
        .await;
    assert!(items
        .as_array()
        .unwrap()
        .iter()
        .all(|i| i["kind"] == "audiobook"));

    // ----------------------------------------------------- detail
    let (status, detail) = app.get(&format!("/api/v1/audiobooks/{one_id}")).await;
    assert_eq!(status, 200, "{detail}");
    let tracks = detail["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 2);
    assert_eq!(tracks[0]["index"], 0);
    assert_eq!(tracks[0]["title"], "01 - Opening");
    assert_eq!(tracks[0]["start_ms"], 0);
    assert_eq!(tracks[0]["codec"], "mp3");
    assert_eq!(tracks[0]["content_type"], "audio/mpeg");
    assert_eq!(tracks[1]["start_ms"], tracks[0]["duration_ms"]);
    let chapters = detail["chapters"].as_array().unwrap();
    let chapter_titles: Vec<&str> = chapters
        .iter()
        .map(|c| c["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        chapter_titles,
        ["Opening", "Middle"],
        "one chapter per track, title tags"
    );
    assert_eq!(chapters[1]["start_ms"], tracks[1]["start_ms"]);
    assert_eq!(chapters[1]["end_ms"].as_i64().unwrap(), one_len);

    let (_, big_detail) = app.get(&format!("/api/v1/audiobooks/{big_id}")).await;
    let big_titles: Vec<&str> = big_detail["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        big_titles,
        ["Zed", "Alpha"],
        "disc folders order the tracks"
    );

    let (_, single_detail) = app.get(&format!("/api/v1/audiobooks/{single_id}")).await;
    let ch = single_detail["chapters"].as_array().unwrap();
    assert_eq!(ch[0]["title"], "Prologue");
    assert_eq!(ch[0]["start_ms"], 0);
    assert_eq!(ch[0]["end_ms"], 1500);
    assert_eq!(ch[1]["title"], "The End");
    assert_eq!(ch[1]["start_ms"], 1500);
    assert_eq!(single_detail["tracks"][0]["content_type"], "audio/mp4");

    let (status, _) = app
        .get("/api/v1/audiobooks/00000000-0000-0000-0000-000000000000")
        .await;
    assert_eq!(status, 404);
    let (status, _) = app.get("/api/v1/audiobooks/not-a-uuid").await;
    assert_eq!(status, 404);

    // -------------------------------------------------- streaming
    let stream_url = tracks[0]["stream_url"].as_str().unwrap().to_string();
    let size = tracks[0]["size_bytes"].as_i64().unwrap();
    // Token URL, no cookie: what a native player gets.
    let (status, bytes, content_range, content_type) =
        app.raw(&stream_url, Some("bytes=0-99"), false).await;
    assert_eq!(status, 206);
    assert_eq!(bytes.len(), 100);
    assert_eq!(content_range, format!("bytes 0-99/{size}"));
    assert_eq!(content_type, "audio/mpeg");
    let (status, bytes, _, _) = app.raw(&stream_url, None, false).await;
    assert_eq!(status, 200);
    assert_eq!(bytes.len() as i64, size);
    // Book-scoped route, session cookie.
    let track_path = format!("/api/v1/audiobooks/{one_id}/tracks/1/stream");
    let (status, bytes, content_range, _) = app.raw(&track_path, Some("bytes=10-"), true).await;
    assert_eq!(status, 206);
    let size1 = tracks[1]["size_bytes"].as_i64().unwrap();
    assert_eq!(bytes.len() as i64, size1 - 10);
    assert_eq!(content_range, format!("bytes 10-{}/{size1}", size1 - 1));
    let (status, _, content_range, _) = app
        .raw(&track_path, Some(&format!("bytes={size1}-")), true)
        .await;
    assert_eq!(status, 416);
    assert_eq!(content_range, format!("bytes */{size1}"));
    let (status, _, _, _) = app.raw(&track_path, None, false).await;
    assert_eq!(status, 401, "no cookie, no token");
    // A token for track 0 does not open track 1.
    let token = stream_url.split("token=").nth(1).unwrap();
    let (status, _, _, _) = app
        .raw(&format!("{track_path}?token={token}"), None, false)
        .await;
    assert_eq!(status, 401);
    let (status, _, _, _) = app
        .raw(
            &format!("/api/v1/audiobooks/{one_id}/tracks/9/stream"),
            None,
            true,
        )
        .await;
    assert_eq!(status, 404);

    // Folder cover through the existing artwork endpoint.
    let (status, bytes, _, content_type) = app
        .raw(
            &format!("/api/v1/items/{one_id}/artwork/poster"),
            None,
            true,
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(content_type, "image/jpeg");
    assert!(!bytes.is_empty());

    // --------------------------------------------------- progress
    // AB11: these books are ~5 s long, shorter than Audiobookshelf's
    // 10 s auto-finish default, so the library finishes a book with
    // less than 1 s left instead.
    let (status, settings) = app
        .req(
            Method::PUT,
            &format!("/api/v1/admin/libraries/{library_id}/audiobook-settings"),
            Some(
                json!({"parse_subtitles": false, "finish_remaining_seconds": 1,
                        "finish_percent": null}),
            ),
        )
        .await;
    assert_eq!(status, 200, "{settings}");
    let progress = format!("/api/v1/audiobooks/{one_id}/progress");
    let (status, p) = app.get(&progress).await;
    assert_eq!(status, 200);
    assert_eq!(p["position_ms"], 0);
    assert_eq!(p["finished"], false);
    assert_eq!(p["last_listened_at"], Value::Null);

    let (status, p) = app
        .req(Method::PUT, &progress, Some(json!({"position_ms": 2500})))
        .await;
    assert_eq!(status, 200, "{p}");
    assert_eq!(p["position_ms"], 2500);
    assert_eq!(p["duration_ms"], one_len);
    assert_eq!(p["finished"], false);
    assert!(p["last_listened_at"].is_string());

    // Past the end clamps onto the book, and (AB11) less than the
    // library's 1 s left finishes it.
    let (_, p) = app
        .req(
            Method::PUT,
            &progress,
            Some(json!({"position_ms": 999_999})),
        )
        .await;
    assert_eq!(p["position_ms"], one_len);
    assert_eq!(p["finished"], true, "auto-finished at the end");

    let (status, _) = app.req(Method::PUT, &progress, Some(json!({}))).await;
    assert_eq!(status, 400);
    let (status, _) = app
        .req(Method::PUT, &progress, Some(json!({"position_ms": -1})))
        .await;
    assert_eq!(status, 400);

    let finished_at = p["finished_at"].clone();
    assert!(finished_at.is_string());
    let listened_at = p["last_listened_at"].clone();
    let (_, p) = app
        .req(Method::PUT, &progress, Some(json!({"finished": true})))
        .await;
    assert_eq!(p["finished"], true);
    assert_eq!(
        p["position_ms"], one_len,
        "finishing leaves the position alone"
    );
    assert_eq!(
        p["finished_at"], finished_at,
        "finishing twice keeps the first time"
    );
    assert_eq!(
        p["last_listened_at"], listened_at,
        "marking is not listening"
    );
    // AB11, as Audiobookshelf: a finished book moved back before the
    // finishing point is unfinished again.
    let (_, p) = app
        .req(Method::PUT, &progress, Some(json!({"position_ms": 100})))
        .await;
    assert_eq!(p["finished"], false, "moved back, unfinished");
    assert_eq!(p["finished_at"], Value::Null);
    assert_eq!(p["position_ms"], 100);
    // Finishing by hand keeps the position; un-finishing by hand puts
    // it back to 0.
    let (_, p) = app
        .req(Method::PUT, &progress, Some(json!({"finished": true})))
        .await;
    assert_eq!(p["finished"], true);
    assert_eq!(p["position_ms"], 100);
    let (_, p) = app
        .req(Method::PUT, &progress, Some(json!({"finished": false})))
        .await;
    assert_eq!(p["finished"], false);
    assert_eq!(p["finished_at"], Value::Null);
    assert_eq!(p["position_ms"], 0, "un-finishing by hand starts over");
    let (_, p) = app
        .req(Method::PUT, &progress, Some(json!({"position_ms": 100})))
        .await;
    assert_eq!(p["position_ms"], 100);

    // Inline in the list, and `recent` puts it first.
    let (_, books) = app.get("/api/v1/audiobooks?sort=recent").await;
    assert_eq!(titles(&books)[0], "Book One");
    assert_eq!(books[0]["progress"]["position_ms"], 100);
    assert!(books[0]["progress"]["updated_at"].is_string());

    // Bulk sync: everything, then only what changed since.
    let (status, sync) = app.get("/api/v1/audiobooks/progress").await;
    assert_eq!(status, 200, "{sync}");
    assert_eq!(sync["progress"].as_array().unwrap().len(), 1);
    assert_eq!(sync["progress"][0]["item_id"], one_id.as_str());
    let since = sync["server_time"].as_str().unwrap().to_string();
    sleep(Duration::from_millis(20)).await;
    let (_, _) = app
        .req(
            Method::PUT,
            &format!("/api/v1/audiobooks/{single_id}/progress"),
            Some(json!({"position_ms": 1000})),
        )
        .await;
    let (status, sync) = app
        .get(&format!(
            "/api/v1/audiobooks/progress?since={}",
            since.replace('+', "%2B")
        ))
        .await;
    assert_eq!(status, 200, "{sync}");
    let changed: Vec<&str> = sync["progress"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["item_id"].as_str().unwrap())
        .collect();
    assert_eq!(changed, [single_id.as_str()]);

    let (status, _) = app.req(Method::DELETE, &progress, None).await;
    assert_eq!(status, 204);
    let (_, p) = app.get(&progress).await;
    assert_eq!(p["position_ms"], 0);
    assert_eq!(p["updated_at"], Value::Null);

    // ---------------------------------------------- profiles + access
    let (status, kid) = app
        .req(
            Method::POST,
            "/api/v1/profiles",
            Some(json!({"name": "Kid"})),
        )
        .await;
    assert_eq!(status, 201, "{kid}");
    let kid_id = kid["id"].as_str().unwrap().to_string();
    let (status, _) = app
        .req(
            Method::POST,
            &format!("/api/v1/profiles/{kid_id}/select"),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 200);
    let (_, books) = app.get("/api/v1/audiobooks").await;
    assert_eq!(books, json!([]), "no library access, no books");
    let single_progress = format!("/api/v1/audiobooks/{single_id}/progress");
    for (method, body) in [
        (Method::GET, None),
        (Method::PUT, Some(json!({"position_ms": 5}))),
        (Method::DELETE, None),
    ] {
        let (status, _) = app.req(method.clone(), &single_progress, body).await;
        assert_eq!(status, 404, "{method} without access");
    }
    let (status, _) = app.get(&format!("/api/v1/audiobooks/{single_id}")).await;
    assert_eq!(status, 404);
    let (status, _, _, _) = app
        .raw(
            &format!("/api/v1/audiobooks/{one_id}/tracks/0/stream"),
            None,
            true,
        )
        .await;
    assert_eq!(status, 401, "the session's profile can't reach the library");
    let (status, sync) = app.get("/api/v1/audiobooks/progress").await;
    assert_eq!(status, 200);
    assert_eq!(sync["progress"], json!([]));

    // With access, the kid's progress is its own.
    let (status, _) = app
        .req(
            Method::POST,
            &format!("/api/v1/admin/libraries/{library_id}/access"),
            Some(json!({"profile_id": kid_id})),
        )
        .await;
    assert_eq!(status, 204);
    let (_, p) = app.get(&single_progress).await;
    assert_eq!(
        p["position_ms"], 0,
        "the first profile's 1000 ms is not the kid's"
    );
    let (_, p) = app
        .req(
            Method::PUT,
            &single_progress,
            Some(json!({"position_ms": 300})),
        )
        .await;
    assert_eq!(p["position_ms"], 300);
    let (status, _) = app
        .req(
            Method::POST,
            &format!("/api/v1/profiles/{profile_id}/select"),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, 200);
    let (_, p) = app.get(&single_progress).await;
    assert_eq!(p["position_ms"], 1000);

    // ---------------------------------------------------- re-scans
    let library_uuid = uuid::Uuid::parse_str(&library_id).unwrap();
    let report = parlour_catalog::scan_library(&pool, library_uuid, None)
        .await
        .expect("rescan");
    assert_eq!(report.seen_files, 5);
    assert_eq!(report.unchanged_files, 5);
    assert_eq!(report.probed_files, 0, "unchanged books are not probed");
    assert_eq!(report.created_items, 0);
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    // A track goes away: its book is rebuilt (same id, so progress
    // stays), the other books are untouched.
    let (_, p) = app
        .req(Method::PUT, &progress, Some(json!({"position_ms": 1200})))
        .await;
    assert_eq!(p["position_ms"], 1200);
    std::fs::remove_file(book_one.join("02 - Middle.mp3")).unwrap();
    let report = parlour_catalog::scan_library(&pool, library_uuid, None)
        .await
        .expect("rescan");
    assert_eq!(report.probed_files, 1);
    assert_eq!(report.unchanged_files, 3);
    assert_eq!(report.created_items, 0);
    let (_, detail) = app.get(&format!("/api/v1/audiobooks/{one_id}")).await;
    assert_eq!(detail["tracks"].as_array().unwrap().len(), 1);
    assert_eq!(detail["chapters"].as_array().unwrap().len(), 1);
    assert_eq!(detail["track_count"], 1);
    assert_eq!(detail["progress"]["position_ms"], 1200);
    let (status, _, _, _) = app.raw(&track_path, None, true).await;
    assert_eq!(status, 404, "the removed track is gone");

    // The whole folder goes away: the book stays, unavailable.
    std::fs::remove_dir_all(&big_dir).unwrap();
    let report = parlour_catalog::scan_library(&pool, library_uuid, None)
        .await
        .expect("rescan");
    assert_eq!(report.missing_files, 2);
    let (_, detail) = app.get(&format!("/api/v1/audiobooks/{big_id}")).await;
    assert_eq!(detail["available"], false);
    assert_eq!(detail["tracks"], json!([]));

    let _ = std::fs::remove_dir_all(&root);
}
