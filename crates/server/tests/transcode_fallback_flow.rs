//! End-to-end test of AB10: the AAC fallback for book tracks a client
//! profile can't play, using the audio profiles (CL5's, server side).
//!
//! - Without a profile the book detail is AB3's: every track is the
//!   original file, `plan` null. An unknown profile name is refused.
//! - The same ALAC book is a transcode for the browser and the Android
//!   profiles and plays as it is for Apple's; WMA converts everywhere; an
//!   MP3 book plays as it is (byte for byte); raw ADTS AAC is copied
//!   into MP4, not re-encoded.
//! - The detail starts the resume track's copy at once; another track's
//!   copy starts on its first request, which answers 503 + Retry-After.
//! - The copy is AAC in MP4 with its index first, served with Range
//!   (206 / suffix ranges), so ffmpeg reading it over HTTP seeks into
//!   the middle; it is kept, and made again when a re-scan sees the
//!   source change.
//! - A listening session on a converted book: Now Playing shows the
//!   plan (AAC transcode and why), the reports and the progress write
//!   work as AB8/AB11 built them; an admin stop revokes the copy's URL.
//! - A video file's sound is never converted this way (404).
//!
//! Ignored by default; needs PARLOUR_TEST_DATABASE_URL (a fresh
//! database) and ffmpeg/ffprobe on PATH.
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test transcode_fallback_flow -- --ignored

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use reqwest::header::{COOKIE, RANGE, SET_COOKIE};
use reqwest::Method;
use serde_json::{json, Value};

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

/// A stereo sine of `secs` seconds, encoded with `codec_args`.
fn make_audio(path: &Path, secs: u32, codec_args: &[&str]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let source = format!("sine=frequency=440:duration={secs}");
    let mut args = vec![
        "-y", "-v", "error", "-f", "lavfi", "-i", &source, "-ac", "2",
    ];
    args.extend_from_slice(codec_args);
    args.push(path.to_str().unwrap());
    ffmpeg(&args);
}

fn make_mp4(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    ffmpeg(&[
        "-y",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=teal:s=320x240:d=3",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:duration=3",
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

/// ffprobe's codec name and duration (s) for a file or URL.
fn probe(input: &str) -> (String, f64) {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=codec_name:format=duration",
            "-of",
            "default=nw=1",
            input,
        ])
        .output()
        .expect("ffprobe available");
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(&format!("{key}=")))
            .unwrap_or_else(|| panic!("no {key} in {text}"))
            .to_string()
    };
    (field("codec_name"), field("duration").parse().unwrap())
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

    async fn ok(&self, path: &str) -> Value {
        let (status, v) = self.req(Method::GET, path, None).await;
        assert_eq!(status, 200, "{path}: {v}");
        v
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        self.req(Method::POST, path, Some(body)).await
    }

    /// A media URL as a native player fetches it: the token in the URL,
    /// no cookie.
    async fn media(
        &self,
        method: Method,
        url: &str,
        range: Option<&str>,
    ) -> (u16, reqwest::header::HeaderMap, Vec<u8>) {
        let mut builder = self.client.request(method, format!("{}{}", self.base, url));
        if let Some(r) = range {
            builder = builder.header(RANGE, r);
        }
        let resp = builder.send().await.expect("media request");
        let status = resp.status().as_u16();
        let headers = resp.headers().clone();
        let bytes = resp.bytes().await.unwrap_or_default().to_vec();
        (status, headers, bytes)
    }

    /// Poll a fallback URL until the copy is there; the bytes.
    async fn wait_copy(&self, url: &str) -> Vec<u8> {
        for _ in 0..120 {
            let (status, headers, bytes) = self.media(Method::GET, url, None).await;
            match status {
                200 => return bytes,
                503 => {
                    assert_eq!(headers["retry-after"], "2");
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                other => panic!("{url}: {other} {}", String::from_utf8_lossy(&bytes)),
            }
        }
        panic!("the copy at {url} never became ready");
    }
}

fn find<'a>(books: &'a Value, title: &str) -> &'a Value {
    books
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["title"] == title)
        .unwrap_or_else(|| panic!("no book {title} in {books}"))
}

fn file_part(url: &str) -> &str {
    url.split("/api/v1/stream/")
        .nth(1)
        .unwrap()
        .split(['/', '?'])
        .next()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL and ffmpeg"]
async fn transcode_fallback_flow() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test database url");
    let port = free_port();
    let base = format!("http://127.0.0.1:{port}");
    let pool = parlour_db::connect(&db_url).await.expect("connect");
    parlour_db::migrate(&pool).await.expect("migrate");

    // Books (folder names speak; no tags matter):
    //   Ann Writer/Lossless Book/01.m4a, 02.m4a   ALAC, 30 s each
    //   Ann Writer/Plain Book/01.mp3              MP3
    //   Ann Writer/Windows Book/01.wma            WMA
    //   Ann Writer/Raw Book/01.aac                ADTS AAC
    // and one movie in a separate library.
    let root = PathBuf::from(format!("/tmp/parlour-ab10-fixtures-{port}"));
    let _ = std::fs::remove_dir_all(&root);
    let books_root = root.join("books");
    let ann = books_root.join("Ann Writer");
    let alac = ["-c:a", "alac"];
    make_audio(&ann.join("Lossless Book").join("01.m4a"), 30, &alac);
    make_audio(&ann.join("Lossless Book").join("02.m4a"), 30, &alac);
    let mp3 = ann.join("Plain Book").join("01.mp3");
    make_audio(&mp3, 30, &["-c:a", "libmp3lame", "-b:a", "64k"]);
    make_audio(
        &ann.join("Windows Book").join("01.wma"),
        30,
        &["-c:a", "wmav2"],
    );
    make_audio(&ann.join("Raw Book").join("01.aac"), 30, &["-c:a", "aac"]);
    let movies_root = root.join("movies");
    make_mp4(
        &movies_root
            .join("Some Movie (2020)")
            .join("Some Movie (2020).mp4"),
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
    let cache_dir = root.join("cache");
    let mut state = parlour_server::http::AppState::new(pool.clone(), None, false);
    std::sync::Arc::get_mut(&mut state.playback)
        .expect("fresh playback state")
        .configure(cache_dir.clone(), None);
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
    let profiles = app.ok("/api/v1/profiles").await;
    let jordan_id = profiles[0]["id"].as_str().unwrap().to_string();
    let (status, v) = app
        .post(&format!("/api/v1/profiles/{jordan_id}/select"), json!({}))
        .await;
    assert_eq!(status, 200, "{v}");

    let mut library_ids = Vec::new();
    for (name, kind, path) in [
        ("Audiobooks", "audiobooks", &books_root),
        ("Movies", "movies", &movies_root),
    ] {
        let (status, lib) = app
            .post(
                "/api/v1/admin/libraries",
                json!({"name": name, "kind": kind, "paths": [path.to_str().unwrap()]}),
            )
            .await;
        assert_eq!(status, 201, "{lib}");
        let id = lib["id"].as_str().unwrap().to_string();
        let (status, _) = app
            .post(
                &format!("/api/v1/admin/libraries/{id}/access"),
                json!({"profile_id": jordan_id}),
            )
            .await;
        assert_eq!(status, 204);
        let report =
            parlour_catalog::scan_library(&pool, uuid::Uuid::parse_str(&id).unwrap(), None)
                .await
                .expect("scan");
        assert!(report.errors.is_empty(), "{:?}", report.errors);
        library_ids.push(id);
    }

    let books = app.ok("/api/v1/audiobooks").await;
    assert_eq!(books.as_array().unwrap().len(), 4, "{books}");
    let lossless = find(&books, "Lossless Book")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let plain = find(&books, "Plain Book")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let windows = find(&books, "Windows Book")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let raw = find(&books, "Raw Book")["id"].as_str().unwrap().to_string();

    // ------------------------------------------- no profile: AB3 as it was
    let detail = app.ok(&format!("/api/v1/audiobooks/{lossless}")).await;
    let tracks = detail["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 2);
    for t in tracks {
        assert_eq!(t["codec"], "alac");
        assert_eq!(t["plan"], Value::Null, "{t}");
        let url = t["stream_url"].as_str().unwrap();
        assert!(
            url.starts_with("/api/v1/stream/") && !url.contains("aac.m4a"),
            "{url}"
        );
        assert_eq!(t["content_type"], "audio/mp4");
    }
    let (status, v) = app
        .req(
            Method::GET,
            &format!("/api/v1/audiobooks/{lossless}?client=roku"),
            None,
        )
        .await;
    assert_eq!(status, 400, "an unknown profile is refused: {v}");

    // Nothing is converted until a profile needs it.
    assert!(!cache_dir.join("audio-aac").exists());

    // ------------------------------------------- the profiles decide
    let plan_of = |detail: &Value, i: usize| detail["tracks"][i]["plan"].clone();
    let web = app
        .ok(&format!("/api/v1/audiobooks/{lossless}?client=web"))
        .await;
    for i in 0..2 {
        let plan = plan_of(&web, i);
        assert_eq!(plan["action"], "Transcode", "{plan}");
        assert_eq!(plan["reason"], "CodecUnsupported");
        assert_eq!(plan["mode"], "transcode");
        assert_eq!(plan["delivery"], "aac");
        assert_eq!(plan["client"], "web");
        let url = web["tracks"][i]["stream_url"].as_str().unwrap();
        assert!(url.contains("/aac.m4a?token="), "{url}");
        assert_eq!(web["tracks"][i]["content_type"], "audio/mp4");
        // The source facts stay the source's.
        assert_eq!(web["tracks"][i]["codec"], "alac");
    }
    let track0 = web["tracks"][0]["file_id"].as_str().unwrap().to_string();
    let track1 = web["tracks"][1]["file_id"].as_str().unwrap().to_string();
    let copy0_url = web["tracks"][0]["stream_url"].as_str().unwrap().to_string();
    let copy1_url = web["tracks"][1]["stream_url"].as_str().unwrap().to_string();
    assert_eq!(file_part(&copy0_url), track0);

    // The detail started the resume track's copy (track 0: never
    // played), and only that one.
    let dir0 = cache_dir.join("audio-aac").join(&track0);
    let dir1 = cache_dir.join("audio-aac").join(&track1);
    for _ in 0..40 {
        if dir0.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(dir0.exists(), "the resume track's copy was started");
    assert!(!dir1.exists(), "a later track waits for its first request");

    // Apple's AVPlayer plays ALAC as it is; Android's ExoPlayer doesn't.
    let ios = app
        .ok(&format!("/api/v1/audiobooks/{lossless}?client=ios"))
        .await;
    assert_eq!(plan_of(&ios, 0)["action"], "DirectPlay");
    assert_eq!(plan_of(&ios, 0)["client"], "ios");
    assert!(!ios["tracks"][0]["stream_url"]
        .as_str()
        .unwrap()
        .contains("aac.m4a"));
    let fire = app
        .ok(&format!("/api/v1/audiobooks/{lossless}?client=firetv"))
        .await;
    assert_eq!(plan_of(&fire, 0)["action"], "Transcode");
    assert_eq!(plan_of(&fire, 0)["client"], "androidtv");

    let wma = app
        .ok(&format!("/api/v1/audiobooks/{windows}?client=android"))
        .await;
    assert_eq!(wma["tracks"][0]["codec"], "wmav2");
    assert_eq!(plan_of(&wma, 0)["action"], "Transcode");
    assert_eq!(plan_of(&wma, 0)["reason"], "CodecUnsupported");

    let adts = app
        .ok(&format!("/api/v1/audiobooks/{raw}?client=web"))
        .await;
    assert_eq!(plan_of(&adts, 0)["action"], "Remux", "{adts}");
    assert_eq!(plan_of(&adts, 0)["reason"], "ContainerUnsupported");
    assert_eq!(plan_of(&adts, 0)["mode"], "copy");
    assert_eq!(plan_of(&adts, 0)["delivery"], "aac");

    // ------------------------------------------- a direct book, as it is
    let plain_web = app
        .ok(&format!("/api/v1/audiobooks/{plain}?client=web"))
        .await;
    assert_eq!(plan_of(&plain_web, 0)["action"], "DirectPlay");
    assert_eq!(plan_of(&plain_web, 0)["delivery"], "file");
    let plain_url = plain_web["tracks"][0]["stream_url"].as_str().unwrap();
    assert!(!plain_url.contains("aac.m4a"));
    let (status, headers, bytes) = app.media(Method::GET, plain_url, None).await;
    assert_eq!(status, 200);
    assert_eq!(headers["content-type"], "audio/mpeg");
    assert_eq!(
        bytes,
        std::fs::read(&mp3).unwrap(),
        "the MP3 is served byte for byte"
    );

    // ------------------------------------------- the copy
    // A track whose copy nobody asked for yet: the first request starts
    // it and is told to come back.
    let (status, headers, _) = app.media(Method::HEAD, &copy1_url, None).await;
    assert_eq!(status, 503, "the first request for a new copy waits");
    assert_eq!(headers["retry-after"], "2");
    assert!(dir1.exists());

    let full = app.wait_copy(&copy0_url).await;
    assert_eq!(&full[4..8], b"ftyp");
    let moov = full.windows(4).position(|w| w == b"moov").expect("moov");
    let mdat = full.windows(4).position(|w| w == b"mdat").expect("mdat");
    assert!(moov < mdat, "the index comes first (faststart)");
    let local = root.join("copy0.m4a");
    std::fs::write(&local, &full).unwrap();
    let (codec, duration) = probe(local.to_str().unwrap());
    assert_eq!(codec, "aac");
    assert!((duration - 30.0).abs() < 0.2, "copy lasts {duration} s");
    assert_eq!(
        std::fs::read_to_string(dir0.join(".ready"))
            .unwrap()
            .split(':')
            .next(),
        Some("aac-1"),
        "the copy is stamped with its recipe and source"
    );

    // Range, as `<audio>`, AVPlayer and ExoPlayer seek.
    let total = full.len();
    let (status, headers, part) = app
        .media(Method::GET, &copy0_url, Some("bytes=100-199"))
        .await;
    assert_eq!(status, 206);
    assert_eq!(headers["content-type"], "audio/mp4");
    assert_eq!(
        headers["content-range"],
        format!("bytes 100-199/{total}").as_str()
    );
    assert_eq!(part, full[100..200]);
    let (status, _, tail) = app
        .media(Method::GET, &copy0_url, Some("bytes=-1000"))
        .await;
    assert_eq!(status, 206);
    assert_eq!(tail, full[total - 1000..]);
    let (status, _, _) = app
        .media(Method::GET, &copy0_url, Some(&format!("bytes={total}-")))
        .await;
    assert_eq!(status, 416);

    // Seeking through the copy over HTTP: ffmpeg jumps to 20 s by byte
    // range and decodes from there.
    let url = format!("{base}{copy0_url}");
    let out = Command::new("ffmpeg")
        .args([
            "-v", "error", "-ss", "20", "-i", &url, "-t", "2", "-f", "s16le", "-ac", "1", "-ar",
            "8000", "-",
        ])
        .output()
        .expect("ffmpeg");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        out.stdout.len(),
        2 * 8000 * 2,
        "two seconds decoded from 20 s in"
    );
    let (codec, duration) = probe(&url);
    assert_eq!(codec, "aac");
    assert!((duration - 30.0).abs() < 0.2);

    // Kept: asking again serves the same file, nothing is re-made.
    let made = std::fs::metadata(dir0.join("audio.m4a"))
        .unwrap()
        .modified()
        .unwrap();
    let again = app.wait_copy(&copy0_url).await;
    assert_eq!(again, full);
    assert_eq!(
        std::fs::metadata(dir0.join("audio.m4a"))
            .unwrap()
            .modified()
            .unwrap(),
        made
    );
    assert!(!dir0.join("audio.part.m4a").exists());

    // The other track's copy finished too; WMA and the ADTS copy work.
    let copy1 = app.wait_copy(&copy1_url).await;
    assert!(!copy1.is_empty());
    let wma_copy = app
        .wait_copy(wma["tracks"][0]["stream_url"].as_str().unwrap())
        .await;
    let local = root.join("wma.m4a");
    std::fs::write(&local, &wma_copy).unwrap();
    assert_eq!(probe(local.to_str().unwrap()).0, "aac");
    let adts_copy = app
        .wait_copy(adts["tracks"][0]["stream_url"].as_str().unwrap())
        .await;
    let local = root.join("adts.m4a");
    std::fs::write(&local, &adts_copy).unwrap();
    let (codec, duration) = probe(local.to_str().unwrap());
    assert_eq!(codec, "aac");
    assert!((duration - 30.0).abs() < 0.2);

    // ------------------------------------------- a listening session on it
    let web = app
        .ok(&format!("/api/v1/audiobooks/{lossless}?client=web"))
        .await;
    let (status, started) = app
        .post(
            "/api/v1/playback/start",
            json!({"item_id": lossless, "position_ms": 0, "duration_ms": 60000,
                   "client_name": "Parlour Web", "device_name": "Chrome on Linux"}),
        )
        .await;
    assert_eq!(status, 200, "{started}");
    let session = started["session_id"].as_str().unwrap().to_string();
    let now_playing = app.ok("/api/v1/admin/sessions").await;
    let card = now_playing
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == session.as_str())
        .unwrap_or_else(|| panic!("no session in {now_playing}"));
    let plan = &card["plan"];
    assert_eq!(plan["action"], "Transcode", "{card}");
    assert_eq!(plan["reason"], "CodecUnsupported");
    assert_eq!(plan["delivery"], "aac");
    assert_eq!(plan["client"], "web");
    assert_eq!(plan["container"], "mov");
    assert_eq!(plan["video"], Value::Null);
    assert_eq!(plan["audio"]["codec"], "alac");
    assert_eq!(plan["audio"]["channels"], 2);
    assert_eq!(plan["audio"]["mode"], "transcode");

    // The reports and the progress write, as AB8/AB11 built them; a
    // position in the second track (book-global).
    let (status, beat) = app
        .post(
            "/api/v1/playback/progress",
            json!({"item_id": lossless, "session_id": session, "position_ms": 35000,
                   "duration_ms": 60000, "listened_ms": 5000}),
        )
        .await;
    assert_eq!(status, 200, "{beat}");
    assert_eq!(beat["session_id"], session.as_str());
    let (status, progress) = app
        .req(
            Method::PUT,
            &format!("/api/v1/audiobooks/{lossless}/progress"),
            Some(json!({"position_ms": 35000, "client_updated_at": "2026-11-26T12:00:00Z"})),
        )
        .await;
    assert_eq!(status, 200, "{progress}");
    assert_eq!(progress["position_ms"], 35000);
    assert_eq!(progress["finished"], false);
    let (user_rows,): (i64,) = sqlx::query_as("SELECT count(*) FROM user_data")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(user_rows, 0, "a listen still writes no user_data row");
    let (status, _) = app
        .post(
            "/api/v1/playback/stopped",
            json!({"item_id": lossless, "session_id": session, "position_ms": 35000,
                   "duration_ms": 60000, "listened_ms": 0}),
        )
        .await;
    assert_eq!(status, 204);
    let mine = app.ok("/api/v1/listening/sessions").await;
    let closed = &mine["sessions"][0];
    assert_eq!(closed["id"], session.as_str());
    assert_ne!(closed["ended_at"], Value::Null);
    assert_eq!(closed["listened_ms"], 5000);

    // The book resumes where the progress write left it (second track).
    let resumed = app
        .ok(&format!("/api/v1/audiobooks/{lossless}?client=web"))
        .await;
    assert_eq!(resumed["progress"]["position_ms"], 35000);

    // A direct book's session shows the plain plan.
    app.ok(&format!("/api/v1/audiobooks/{plain}?client=web"))
        .await;
    let (_, started) = app
        .post(
            "/api/v1/playback/start",
            json!({"item_id": plain, "position_ms": 0, "duration_ms": 30000}),
        )
        .await;
    let plain_session = started["session_id"].as_str().unwrap().to_string();
    let now_playing = app.ok("/api/v1/admin/sessions").await;
    let card = now_playing
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == plain_session.as_str())
        .unwrap();
    assert_eq!(card["plan"]["action"], "DirectPlay");
    assert_eq!(card["plan"]["delivery"], "file");
    assert_eq!(card["plan"]["audio"]["mode"], "direct_play");

    // An admin stop revokes the copy's URL like the file's.
    let (_, started) = app
        .post(
            "/api/v1/playback/start",
            json!({"item_id": lossless, "position_ms": 35000, "duration_ms": 60000}),
        )
        .await;
    let stopped_session = started["session_id"].as_str().unwrap().to_string();
    let live_url = web["tracks"][1]["stream_url"].as_str().unwrap().to_string();
    let (status, _, _) = app.media(Method::GET, &live_url, Some("bytes=0-99")).await;
    assert_eq!(status, 206);
    let (status, _) = app
        .post(
            &format!("/api/v1/admin/sessions/{stopped_session}/stop"),
            json!({}),
        )
        .await;
    assert_eq!(status, 204);
    let (status, _, _) = app.media(Method::GET, &live_url, Some("bytes=0-99")).await;
    assert_eq!(status, 401, "a stopped session's copy is refused");

    // ------------------------------------------- the source changes
    // A replaced file (same path, so the same file id) is re-scanned:
    // the old copy no longer matches its stamp and is made again.
    let old_stamp = std::fs::read_to_string(dir0.join(".ready")).unwrap();
    make_audio(&ann.join("Lossless Book").join("01.m4a"), 12, &alac);
    let report =
        parlour_catalog::scan_library(&pool, uuid::Uuid::parse_str(&library_ids[0]).unwrap(), None)
            .await
            .expect("rescan");
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let web = app
        .ok(&format!("/api/v1/audiobooks/{lossless}?client=web"))
        .await;
    assert_eq!(web["tracks"][0]["file_id"], track0.as_str(), "same file id");
    let fresh = app
        .wait_copy(web["tracks"][0]["stream_url"].as_str().unwrap())
        .await;
    let local = root.join("fresh.m4a");
    std::fs::write(&local, &fresh).unwrap();
    let (_, duration) = probe(local.to_str().unwrap());
    assert!(
        (duration - 12.0).abs() < 0.2,
        "the new copy is of the new file: {duration}"
    );
    assert_ne!(
        std::fs::read_to_string(dir0.join(".ready")).unwrap(),
        old_stamp
    );

    // ------------------------------------------- never for a video file
    let items = app
        .ok(&format!("/api/v1/libraries/{}/items", library_ids[1]))
        .await;
    let movie_id = items[0]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("no movie in {items}"))
        .to_string();
    let info = app
        .ok(&format!(
            "/api/v1/items/{movie_id}/playback-info?client=web"
        ))
        .await;
    let movie_file = info["file_id"].as_str().unwrap();
    let token = info["stream_url"]
        .as_str()
        .unwrap()
        .split("token=")
        .nth(1)
        .unwrap();
    let (status, _, _) = app
        .media(
            Method::GET,
            &format!("/api/v1/stream/{movie_file}/aac.m4a?token={token}"),
            None,
        )
        .await;
    assert_eq!(
        status, 404,
        "a video file's sound is not converted this way"
    );
    assert!(!cache_dir.join("audio-aac").join(movie_file).exists());

    let _ = std::fs::remove_dir_all(&root);
}
