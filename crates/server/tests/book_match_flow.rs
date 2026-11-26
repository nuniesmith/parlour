//! End-to-end test of AB6 (D30): an audiobooks library's books merged
//! from their local sources in the library's precedence order (folder
//! → first track's tags → NFO → desc.txt / reader.txt → OPF →
//! Audiobookshelf's metadata.json → Parlour's book.parlour.json),
//! embedded covers extracted into the artwork cache, an admin's manual
//! match against a **stub** Google Books and Open Library (no key, no
//! internet) with overwrite or fill-gaps, a library-wide quick match
//! that fills gaps, the per-library precedence list, the write_sidecars
//! opt-in for books, forgetting a match, and re-scans staying
//! idempotent throughout.
//!
//! Ignored by default; needs PARLOUR_TEST_DATABASE_URL (a fresh
//! database) and ffmpeg/ffprobe on PATH.
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test book_match_flow -- --ignored

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use reqwest::Method;
use serde_json::{json, Value};
use uuid::Uuid;

use parlour_server::config::Config;
use parlour_server::http::AppState;

fn ffmpeg(args: &[&str]) {
    let status = Command::new("ffmpeg")
        .args(args)
        .status()
        .expect("ffmpeg available");
    assert!(status.success(), "ffmpeg failed: {args:?}");
}

fn make_jpeg(path: &Path, colour: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    ffmpeg(&[
        "-y",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        &format!("color=c={colour}:s=64x64"),
        "-frames:v",
        "1",
        path.to_str().unwrap(),
    ]);
}

/// A tagged MP3 of `secs` seconds of tone, with `cover` embedded as
/// its attached picture when given.
fn make_mp3(dir: &Path, name: &str, secs: u32, tags: &[(&str, &str)], cover: Option<&Path>) {
    std::fs::create_dir_all(dir).unwrap();
    let src = format!("sine=frequency=440:duration={secs}");
    let mut args: Vec<String> = ["-y", "-v", "error", "-f", "lavfi", "-i", &src]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if let Some(c) = cover {
        args.extend(["-i".to_string(), c.to_str().unwrap().to_string()]);
        args.extend(["-map", "0:a", "-map", "1:v"].map(String::from));
    }
    for (k, v) in tags {
        args.push("-metadata".into());
        args.push(format!("{k}={v}"));
    }
    args.extend(["-c:a", "libmp3lame", "-b:a", "64k"].map(String::from));
    if cover.is_some() {
        args.extend(
            [
                "-c:v",
                "copy",
                "-id3v2_version",
                "3",
                "-disposition:v:0",
                "attached_pic",
            ]
            .map(String::from),
        );
    }
    args.push(dir.join(name).to_str().unwrap().to_string());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    ffmpeg(&refs);
}

// ------------------------------------------------------- stub providers

#[derive(Clone)]
struct Stub {
    base: String,
    jpeg: Arc<Vec<u8>>,
    /// Every request: "path?query | user-agent".
    seen: Arc<Mutex<Vec<String>>>,
    /// The `X-Goog-Api-Key` header of every request that carried one.
    key_headers: Arc<Mutex<Vec<String>>>,
}

fn record(stub: &Stub, path: &str, q: &HashMap<String, String>, headers: &HeaderMap) {
    let mut keys: Vec<String> = q.iter().map(|(k, v)| format!("{k}={v}")).collect();
    keys.sort();
    let ua = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    stub.seen
        .lock()
        .unwrap()
        .push(format!("{path}?{} | {ua}", keys.join("&")));
    if let Some(k) = headers.get("x-goog-api-key").and_then(|v| v.to_str().ok()) {
        stub.key_headers.lock().unwrap().push(k.to_string());
    }
}

fn volume(stub: &Stub, id: &str) -> Option<Value> {
    let cover = format!("{}/covers/{id}.jpg", stub.base);
    let info = match id {
        "gUntag1" => json!({
            "title": "Untagged Book", "subtitle": "Found Online",
            "authors": ["Ursula Online"], "publisher": "Stub Press",
            "publishedDate": "2005-02-03",
            "description": "<p>Online description.</p>",
            "categories": ["Mystery"],
            "industryIdentifiers": [{"type": "ISBN_13", "identifier": "9780000000019"}],
            "language": "en",
            "imageLinks": {"thumbnail": cover}
        }),
        "gLant1" => json!({
            "title": "The Lantern Keeper", "subtitle": "A Harbour Tale",
            "authors": ["Ada Fictional"], "publisher": "Example Press",
            "publishedDate": "2011", "description": "Provider description.",
            "imageLinks": {"thumbnail": cover}
        }),
        "gShared1" => json!({
            "title": "Shared Work", "authors": ["Someone Else"],
            "description": "Shared description.",
            "imageLinks": {"thumbnail": cover}
        }),
        _ => return None,
    };
    Some(json!({"kind": "books#volume", "id": id, "volumeInfo": info}))
}

async fn stub_volumes(
    State(stub): State<Stub>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Json<Value> {
    record(&stub, "/volumes", &q, &headers);
    let query = q.get("q").cloned().unwrap_or_default();
    let id = if query.contains("Untagged Book") {
        Some("gUntag1")
    } else if query.contains("The Lantern Keeper") {
        Some("gLant1")
    } else if query.contains("Shared Work") {
        Some("gShared1")
    } else {
        None
    };
    match id.and_then(|id| volume(&stub, id)) {
        Some(v) => Json(json!({"kind": "books#volumes", "totalItems": 1, "items": [v]})),
        None => Json(json!({"kind": "books#volumes", "totalItems": 0})),
    }
}

async fn stub_volume(
    State(stub): State<Stub>,
    headers: HeaderMap,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Value>, StatusCode> {
    record(&stub, &format!("/volumes/{id}"), &HashMap::new(), &headers);
    volume(&stub, &id).map(Json).ok_or(StatusCode::NOT_FOUND)
}

async fn stub_ol_search(
    State(stub): State<Stub>,
    headers: HeaderMap,
    Query(q): Query<HashMap<String, String>>,
) -> Json<Value> {
    record(&stub, "/search.json", &q, &headers);
    Json(json!({"numFound": 1, "docs": [{
        "key": "/works/OL7000001W", "title": "Second Book",
        "author_name": ["Bo Imaginary"], "first_publish_year": 1999,
        "publisher": ["OL Press"], "cover_i": 77, "language": ["eng"]
    }]}))
}

async fn stub_image(State(stub): State<Stub>) -> Vec<u8> {
    stub.jpeg.to_vec()
}

fn stub_app(stub: Stub) -> Router {
    Router::new()
        .route("/volumes", get(stub_volumes))
        .route("/volumes/{id}", get(stub_volume))
        .route("/search.json", get(stub_ol_search))
        .route("/covers/{f}", get(stub_image))
        .route("/b/id/{f}", get(stub_image))
        .with_state(stub)
}

// --------------------------------------------------------------- client

fn test_config(url: &str) -> Config {
    let mut vars = HashMap::new();
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
    cookie: String,
}

impl Client {
    async fn req(&self, method: Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut rb = self
            .http
            .request(method, format!("{}{path}", self.base))
            .header("Cookie", format!("parlour_session={}", self.cookie));
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let res = rb.send().await.expect("request");
        let status = res.status().as_u16();
        let text = res.text().await.unwrap_or_default();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn get(&self, path: &str) -> Value {
        let (s, v) = self.req(Method::GET, path, None).await;
        assert_eq!(s, 200, "{path}: {v}");
        v
    }

    async fn bytes(&self, path: &str) -> (u16, Vec<u8>) {
        let res = self
            .http
            .get(format!("{}{path}", self.base))
            .header("Cookie", format!("parlour_session={}", self.cookie))
            .send()
            .await
            .expect("request");
        let s = res.status().as_u16();
        (s, res.bytes().await.unwrap_or_default().to_vec())
    }

    async fn book(&self, id: &str) -> Value {
        self.get(&format!("/api/v1/audiobooks/{id}")).await
    }

    async fn apply(&self, id: &str, body: Value) -> (u16, Value) {
        self.req(
            Method::POST,
            &format!("/api/v1/admin/audiobooks/{id}/match"),
            Some(body),
        )
        .await
    }

    async fn run_job(&self, path: &str) -> Value {
        let (s, job) = self.req(Method::POST, path, None).await;
        assert_eq!(s, 202, "{path}: {job}");
        let id = job["id"].as_str().unwrap().to_string();
        for _ in 0..120 {
            let job = self.get(&format!("/api/v1/admin/jobs/{id}")).await;
            match job["status"].as_str() {
                Some("succeeded") => return job,
                Some("failed") => panic!("{path}: job failed: {job}"),
                _ => tokio::time::sleep(std::time::Duration::from_millis(250)).await,
            }
        }
        panic!("{path}: job did not finish");
    }
}

async fn login(http: &reqwest::Client, base: &str) -> (String, Value) {
    let res = http
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"username": "jordan", "password": "admin-pass-123"}))
        .send()
        .await
        .expect("login");
    assert_eq!(res.status(), 200);
    let cookie = res
        .headers()
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| {
            s.split(';')
                .next()
                .and_then(|kv| kv.split_once('='))
                .map(|(_, v)| v.to_string())
        })
        .expect("session cookie");
    (cookie, res.json().await.unwrap())
}

fn by_title<'a>(books: &'a Value, title: &str) -> &'a Value {
    books
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["title"] == title)
        .unwrap_or_else(|| panic!("no book {title} in {books}"))
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect()
}

async fn poster(pool: &parlour_db::PgPool, item: &str) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT variant, path FROM images WHERE item_id = $1::uuid AND kind = 'poster'
         ORDER BY variant",
    )
    .bind(item)
    .fetch_all(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL and ffmpeg"]
async fn book_match_flow_end_to_end() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test db url");
    let config = test_config(&db_url);
    let pool = parlour_db::connect(&config.database_url).await.unwrap();
    parlour_db::migrate(&pool).await.unwrap();
    parlour_server::auth::bootstrap_admin(&pool, &config)
        .await
        .unwrap();

    let root: PathBuf = std::env::temp_dir().join(format!(
        "parlour-book-match-flow-{}",
        Uuid::new_v4().simple()
    ));
    let media = root.join("books");
    let artwork = root.join("artwork");

    // Stub providers.
    let stub_jpeg = root.join("stub-cover.jpg");
    make_jpeg(&stub_jpeg, "blue");
    let stub_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stub_base = format!("http://{}", stub_listener.local_addr().unwrap());
    let stub = Stub {
        base: stub_base.clone(),
        jpeg: Arc::new(std::fs::read(&stub_jpeg).unwrap()),
        seen: Arc::new(Mutex::new(Vec::new())),
        key_headers: Arc::new(Mutex::new(Vec::new())),
    };
    let app_stub = stub_app(stub.clone());
    tokio::spawn(async move {
        axum::serve(stub_listener, app_stub).await.unwrap();
    });

    // ------------------------------------------------------------ media
    // A: two tracks, an embedded cover on the first, desc.txt; the
    // second track's tags must not count (first track only).
    let embedded = root.join("embedded.jpg");
    make_jpeg(&embedded, "red");
    let a_dir = media.join("Ada Fictional/The Lantern Keeper");
    make_mp3(
        &a_dir,
        "01.mp3",
        2,
        &[
            ("album", "The Lantern Keeper"),
            ("album_artist", "Ada Fictional"),
            ("composer", "Nell Reader"),
            ("genre", "Fiction"),
            ("date", "2010"),
            ("track", "1"),
        ],
        Some(&embedded),
    );
    make_mp3(
        &a_dir,
        "02.mp3",
        2,
        &[
            ("album", "The Lantern Keeper"),
            ("track", "2"),
            ("comment", "Second track comment"),
            ("publisher", "Late Press"),
        ],
        None,
    );
    std::fs::write(a_dir.join("desc.txt"), "Description from desc.txt.\n").unwrap();
    // B: every local source, and a folder cover someone else saved.
    let b_dir = media.join("Bo Imaginary/Second Book");
    make_mp3(
        &b_dir,
        "01.mp3",
        2,
        &[("album", "Second Book (tag)"), ("artist", "Tag Artist")],
        None,
    );
    std::fs::write(
        b_dir.join("book.nfo"),
        "Title: NFO Title: NFO Subtitle\nAuthor: Bo Imaginary\nNarrator: Nina Nfo\nPublisher: NFO Press\n",
    )
    .unwrap();
    std::fs::write(
        b_dir.join("book.opf"),
        r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf"><dc:title>OPF Title</dc:title><dc:publisher>OPF Press</dc:publisher><dc:language>en</dc:language><dc:identifier opf:scheme="ISBN">9780000000026</dc:identifier></metadata></package>"#,
    )
    .unwrap();
    std::fs::write(
        b_dir.join("metadata.json"),
        r#"{"title": "JSON Title", "genres": ["Json Genre"]}"#,
    )
    .unwrap();
    make_jpeg(&b_dir.join("cover.jpg"), "green");
    let b_cover_before = std::fs::read(b_dir.join("cover.jpg")).unwrap();
    // C: nothing but the folder layout (AB2's behaviour must hold).
    let c_dir = media.join("Plain/Untagged Book");
    make_mp3(&c_dir, "01.mp3", 2, &[], None);
    // D: an author folder ABS splits: "Last, First and Last, First".
    let d_dir = media.join("Doe, Jane and Roe, John/Shared Work");
    make_mp3(&d_dir, "01.mp3", 2, &[], None);

    // ------------------------------------------------------------ server
    let mut state = AppState::new(pool.clone(), None, false);
    state.metadata = parlour_server::metadata::MetadataSettings {
        google_books_base: Some(stub_base.clone()),
        open_library_base: Some(stub_base.clone()),
        open_library_covers_base: Some(stub_base.clone()),
        quick_match_pace: std::time::Duration::ZERO,
        artwork_dir: artwork.clone(),
        ..Default::default()
    };
    let app = parlour_server::http::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let http = reqwest::Client::new();
    let base = format!("http://{addr}");
    let (cookie, me) = login(&http, &base).await;
    let client = Client { http, base, cookie };
    let profile = me["profiles"][0]["id"].as_str().unwrap().to_string();
    let (s, _) = client
        .req(
            Method::POST,
            &format!("/api/v1/profiles/{profile}/select"),
            Some(json!({})),
        )
        .await;
    assert_eq!(s, 200);
    let (s, lib) = client
        .req(
            Method::POST,
            "/api/v1/admin/libraries",
            Some(json!({"name": "Books", "kind": "audiobooks",
                        "paths": [media.to_string_lossy()]})),
        )
        .await;
    assert_eq!(s, 201, "{lib}");
    let library = lib["id"].as_str().unwrap().to_string();
    let library_id = Uuid::parse_str(&library).unwrap();
    let (s, _) = client
        .req(
            Method::POST,
            &format!("/api/v1/admin/libraries/{library}/access"),
            Some(json!({"profile_id": profile})),
        )
        .await;
    assert_eq!(s, 204);
    let options = parlour_catalog::ScanOptions {
        artwork_dir: Some(artwork.clone()),
    };
    let scan = || parlour_catalog::scan_library_with(&pool, library_id, None, &options);

    // ------------------------------------------------ 1. merged sources
    let report = scan().await.unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!((report.created_items, report.probed_files), (4, 5));
    let books = client
        .get(&format!("/api/v1/audiobooks?library_id={library}"))
        .await;
    let a = by_title(&books, "The Lantern Keeper").clone();
    assert_eq!(strs(&a["authors"]), ["Ada Fictional"]);
    assert_eq!(strs(&a["narrators"]), ["Nell Reader"]);
    assert_eq!(a["year"], 2010);
    assert_eq!(
        a["overview"], "Description from desc.txt.",
        "txt after tags"
    );
    assert_eq!(
        a["publisher"],
        Value::Null,
        "the second track's tags don't count"
    );
    assert_eq!(a["has_cover"], true, "embedded cover");
    let a_id = a["id"].as_str().unwrap().to_string();
    let a_poster = poster(&pool, &a_id).await;
    assert_eq!(a_poster.len(), 1);
    assert_eq!(a_poster[0].0, "embedded");
    assert!(
        a_poster[0].1.starts_with(artwork.to_str().unwrap()),
        "in the cache"
    );
    let (s, bytes) = client
        .bytes(&format!("/api/v1/items/{a_id}/artwork/poster"))
        .await;
    assert_eq!(s, 200);
    assert!(bytes.starts_with(&[0xFF, 0xD8, 0xFF]), "a JPEG");

    let b = by_title(&books, "JSON Title").clone();
    assert_eq!(b["subtitle"], "NFO Subtitle");
    assert_eq!(strs(&b["authors"]), ["Bo Imaginary"], "NFO over the tag");
    assert_eq!(strs(&b["narrators"]), ["Nina Nfo"]);
    assert_eq!(b["publisher"], "OPF Press", "OPF after NFO");
    assert_eq!(b["isbn"], "9780000000026");
    assert_eq!(b["language"], "en");
    assert_eq!(strs(&b["genres"]), ["Json Genre"]);
    let b_id = b["id"].as_str().unwrap().to_string();
    assert_eq!(
        poster(&pool, &b_id).await[0].0,
        "file",
        "the folder cover wins"
    );

    let c = by_title(&books, "Untagged Book").clone();
    assert_eq!(strs(&c["authors"]), ["Plain"], "AB2's folder author");
    assert_eq!(c["has_cover"], false);
    assert_eq!(c["match"], Value::Null);
    let c_id = c["id"].as_str().unwrap().to_string();
    let d = by_title(&books, "Shared Work").clone();
    assert_eq!(strs(&d["authors"]), ["Jane Doe", "John Roe"]);
    let d_id = d["id"].as_str().unwrap().to_string();

    // A re-scan with nothing changed probes nothing and changes nothing.
    let report = scan().await.unwrap();
    assert_eq!((report.probed_files, report.created_items), (0, 0));
    assert_eq!(report.unchanged_files, 5);
    assert_eq!(client.book(&a_id).await["overview"], a["overview"]);
    assert_eq!(poster(&pool, &a_id).await, a_poster, "embedded cover kept");

    // -------------------------------------------------------- 2. search
    let found = client
        .get(&format!("/api/v1/admin/audiobooks/{c_id}/match/search"))
        .await;
    assert_eq!(found["provider"], "google");
    assert_eq!(found["title"], "Untagged Book");
    assert_eq!(found["author"], "Plain");
    assert_eq!(found["results"][0]["provider_id"], "gUntag1");
    assert_eq!(found["results"][0]["confidence"], 1.0);
    {
        let seen = stub.seen.lock().unwrap();
        let last = seen.last().unwrap();
        assert!(
            last.starts_with("/volumes?maxResults=10&printType=books&q=intitle:Untagged Book inauthor:Plain | Parlour/"),
            "{last}"
        );
        assert!(
            !seen.iter().any(|r| r.contains("key=")),
            "no key configured, none sent"
        );
        assert!(
            stub.key_headers.lock().unwrap().is_empty(),
            "no key configured, no key header"
        );
    }
    // With a key configured, it travels only in the X-Goog-Api-Key
    // header: never in a URL, so never in an error message either. The
    // key is made up at run time; no key-shaped literal is committed.
    {
        let made_up = format!("test-{}", Uuid::new_v4().simple());
        let keyed = parlour_metadata::books::GoogleBooksClient::new(
            Some(made_up.clone()),
            Some(&stub_base),
        );
        let hits = keyed.search("Untagged Book", Some("Plain")).await.unwrap();
        assert_eq!(hits[0].provider_id, "gUntag1");
        assert_eq!(*stub.key_headers.lock().unwrap(), vec![made_up.clone()]);
        assert!(
            !stub
                .seen
                .lock()
                .unwrap()
                .iter()
                .any(|r| r.contains(&made_up)),
            "the key is never in a URL"
        );
        // A failed request's message, which lands on jobs and in API
        // errors, doesn't carry the key either.
        let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let dead_base = format!("http://{}", dead.local_addr().unwrap());
        drop(dead);
        let err = parlour_metadata::books::GoogleBooksClient::new(
            Some(made_up.clone()),
            Some(&dead_base),
        )
        .search("Untagged Book", None)
        .await
        .unwrap_err()
        .to_string();
        assert!(!err.contains(&made_up), "{err}");
        stub.key_headers.lock().unwrap().clear();
    }
    let found = client
        .get(&format!(
            "/api/v1/admin/audiobooks/{b_id}/match/search?provider=openlibrary&title=Second%20Book"
        ))
        .await;
    assert_eq!(found["results"][0]["provider_id"], "OL7000001W");
    let (s, _) = client
        .req(
            Method::GET,
            &format!("/api/v1/admin/audiobooks/{c_id}/match/search?provider=audnexus"),
            None,
        )
        .await;
    assert_eq!(s, 400, "Audnexus isn't a provider");
    let (s, _) = client
        .req(
            Method::POST,
            &format!("/api/v1/admin/items/{c_id}/metadata/fetch"),
            None,
        )
        .await;
    assert_eq!(s, 400, "books aren't matched through TMDB");

    // ------------------------------------------------- 3. fill gaps
    let (s, _) = client
        .apply(
            &a_id,
            json!({"provider": "google", "provider_id": "gLant1"}),
        )
        .await;
    assert!(
        (400..500).contains(&s),
        "the mode is the admin's choice: {s}"
    );
    let (s, _) = client
        .apply(
            &a_id,
            json!({"provider": "google", "provider_id": "gLant1", "mode": "merge"}),
        )
        .await;
    assert_eq!(s, 400);
    let (s, r) = client
        .apply(
            &a_id,
            json!({"provider": "google", "provider_id": "gLant1", "mode": "fill_gaps"}),
        )
        .await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["changed"], true);
    assert_eq!(r["cover"], "kept", "it has a cover; fill gaps keeps it");
    assert_eq!(r["sidecar_written"], false, "the library doesn't write");
    let a2 = client.book(&a_id).await;
    assert_eq!(a2["overview"], "Description from desc.txt.", "kept");
    assert_eq!(a2["year"], 2010, "kept");
    assert_eq!(a2["subtitle"], "A Harbour Tale", "filled");
    assert_eq!(a2["publisher"], "Example Press", "filled");
    assert_eq!(a2["google_books_id"], "gLant1");
    assert_eq!(a2["match"]["mode"], "fill_gaps");
    assert_eq!(poster(&pool, &a_id).await[0].0, "embedded");

    // ------------------------------------------------- 4. overwrite
    let (s, r) = client
        .apply(
            &c_id,
            json!({"provider": "google", "provider_id": "gUntag1", "mode": "overwrite"}),
        )
        .await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["cover"], "applied");
    let c2 = client.book(&c_id).await;
    assert_eq!(strs(&c2["authors"]), ["Ursula Online"]);
    assert_eq!(c2["overview"], "Online description.");
    assert_eq!(c2["subtitle"], "Found Online");
    assert_eq!(c2["year"], 2005);
    assert_eq!(strs(&c2["genres"]), ["Mystery"]);
    assert_eq!(c2["isbn"], "9780000000019");
    assert_eq!(c2["match"]["provider"], "google");
    assert_eq!(c2["match"]["mode"], "overwrite");
    let c_poster = poster(&pool, &c_id).await;
    assert_eq!(c_poster.len(), 1);
    assert_eq!(c_poster[0].0, "match");
    assert_eq!(std::fs::read(&c_poster[0].1).unwrap(), *stub.jpeg);
    assert!(
        !c_dir.join("book.parlour.json").exists() && !c_dir.join("cover.jpg").exists(),
        "nothing beside the media without the opt-in"
    );

    // A re-scan keeps the matches (the server's copy fills the
    // sidecar slot) and probes nothing.
    let report = scan().await.unwrap();
    assert_eq!(report.probed_files, 0);
    let c3 = client.book(&c_id).await;
    assert_eq!(strs(&c3["authors"]), ["Ursula Online"]);
    assert_eq!(poster(&pool, &c_id).await, c_poster);

    // -------------------------------------------------- 5. precedence
    let p = client
        .get(&format!(
            "/api/v1/admin/libraries/{library}/metadata-precedence"
        ))
        .await;
    assert_eq!(
        strs(&p["sources"]),
        ["folder", "tags", "nfo", "txt", "opf", "abs_json", "sidecar"]
    );
    assert_eq!(p["default"], p["sources"]);
    let precedence_path = format!("/api/v1/admin/libraries/{library}/metadata-precedence");
    let put = |sources: Value| {
        client.req(
            Method::PUT,
            &precedence_path,
            Some(json!({ "sources": sources })),
        )
    };
    assert_eq!(put(json!(["tags", "tags"])).await.0, 400);
    assert_eq!(put(json!(["audnexus"])).await.0, 400);
    assert_eq!(put(json!([])).await.0, 400);
    let (s, films) = client
        .req(
            Method::POST,
            "/api/v1/admin/libraries",
            Some(json!({"name": "Films", "kind": "movies", "paths": []})),
        )
        .await;
    assert_eq!(s, 201);
    let (s, _) = client
        .req(
            Method::PUT,
            &format!(
                "/api/v1/admin/libraries/{}/metadata-precedence",
                films["id"].as_str().unwrap()
            ),
            Some(json!({"sources": ["tags"]})),
        )
        .await;
    assert_eq!(s, 400, "video libraries have no precedence");
    let nfo_last = json!(["folder", "tags", "txt", "opf", "abs_json", "sidecar", "nfo"]);
    let (s, p) = put(nfo_last.clone()).await;
    assert_eq!(s, 200, "{p}");
    assert_eq!(p["sources"], nfo_last);
    // A new order merges every book again at the next scan.
    let report = scan().await.unwrap();
    assert_eq!(report.probed_files, 5);
    let b2 = client.book(&b_id).await;
    assert_eq!(b2["title"], "NFO Title", "the NFO is last now");
    assert_eq!(b2["publisher"], "NFO Press");
    assert_eq!(
        strs(&client.book(&c_id).await["authors"]),
        ["Ursula Online"],
        "a match survives a re-merge"
    );
    let (s, _) = put(json!([
        "folder", "tags", "nfo", "txt", "opf", "abs_json", "sidecar"
    ]))
    .await;
    assert_eq!(s, 200);
    scan().await.unwrap();
    assert_eq!(client.book(&b_id).await["title"], "JSON Title");

    // ---------------------------------------- 6. writing beside books
    let (s, flag) = client
        .req(
            Method::PUT,
            &format!("/api/v1/admin/libraries/{library}/write-sidecars"),
            Some(json!({"enabled": true})),
        )
        .await;
    assert_eq!(s, 200, "audiobooks libraries take the opt-in now: {flag}");
    let status = client.get("/api/v1/admin/sidecars").await;
    let row = status
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["id"] == library.as_str())
        .unwrap()
        .clone();
    assert_eq!(row["supported"], true);
    assert_eq!(row["items"], 4);
    assert_eq!(row["sidecars"], 2, "two books carry a match");

    let (s, r) = client
        .apply(
            &b_id,
            json!({"provider": "openlibrary", "provider_id": "OL7000001W",
                   "mode": "overwrite", "cover": true}),
        )
        .await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(r["sidecar_written"], true);
    assert_eq!(r["cover"], "applied");
    assert_eq!(r["cover_written"], false);
    assert!(
        r["left_alone"][0]
            .as_str()
            .unwrap()
            .contains("a file Parlour didn't write"),
        "{r}"
    );
    assert_eq!(
        std::fs::read(b_dir.join("cover.jpg")).unwrap(),
        b_cover_before,
        "someone else's cover is never replaced"
    );
    let sidecar: Value =
        serde_json::from_slice(&std::fs::read(b_dir.join("book.parlour.json")).unwrap()).unwrap();
    assert_eq!(sidecar["kind"], "audiobook");
    assert_eq!(sidecar["title"], "Second Book");
    assert_eq!(sidecar["open_library_id"], "OL7000001W");
    assert_eq!(sidecar["publisher"], "OL Press");
    // C's folder is its own: the sidecar and the cover both go there.
    let (s, r) = client
        .apply(
            &c_id,
            json!({"provider": "google", "provider_id": "gUntag1", "mode": "overwrite"}),
        )
        .await;
    assert_eq!(s, 200, "{r}");
    assert_eq!(
        (r["sidecar_written"].clone(), r["cover_written"].clone()),
        (json!(true), json!(true))
    );
    assert_eq!(std::fs::read(c_dir.join("cover.jpg")).unwrap(), *stub.jpeg);
    assert!(std::fs::read_dir(&c_dir).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".tmp")));

    // The scanner sees the new files, merges again, and the books keep
    // their matches (the files are the ones the match wrote).
    let report = scan().await.unwrap();
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(client.book(&b_id).await["title"], "Second Book");
    assert_eq!(poster(&pool, &c_id).await[0].0, "match");
    let report = scan().await.unwrap();
    assert_eq!(report.probed_files, 0, "and then it settles");

    // A hand edit of the file beats the match until it changes again.
    let mut edited = sidecar.clone();
    edited["title"] = json!("Hand Edited");
    std::fs::write(
        b_dir.join("book.parlour.json"),
        serde_json::to_vec_pretty(&edited).unwrap(),
    )
    .unwrap();
    scan().await.unwrap();
    assert_eq!(client.book(&b_id).await["title"], "Hand Edited");
    // A broken sidecar is reported, and the book keeps the rest.
    std::fs::write(
        d_dir.join("book.parlour.json"),
        b"{\"format\": \"parlour-sidecar\", \"format_version\": 9}",
    )
    .unwrap();
    let report = scan().await.unwrap();
    assert!(
        report.errors.iter().any(|e| e.contains("format version 9")),
        "{:?}",
        report.errors
    );
    assert_eq!(client.book(&d_id).await["title"], "Shared Work");
    std::fs::remove_file(d_dir.join("book.parlour.json")).unwrap();
    scan().await.unwrap();

    // ---------------------------------------------------- 7. quick match
    let job = client
        .run_job(&format!("/api/v1/admin/libraries/{library}/metadata"))
        .await;
    let result = &job["result"];
    assert_eq!(result["quick_match"], true, "{job}");
    assert_eq!(result["mode"], "fill_gaps");
    assert_eq!(result["total"], 4);
    assert_eq!(result["matched"], 1, "{result}");
    assert_eq!(result["unchanged"], 2, "{result}");
    assert_eq!(
        result["no_match"], 1,
        "\"Hand Edited\" finds nothing: {result}"
    );
    assert_eq!(result["failed"], 0);
    assert_eq!(result["covers_applied"], 1);
    let d2 = client.book(&d_id).await;
    assert_eq!(strs(&d2["authors"]), ["Jane Doe", "John Roe"], "gaps only");
    assert_eq!(d2["overview"], "Shared description.");
    assert_eq!(d2["has_cover"], true);
    assert_eq!(d2["match"]["mode"], "fill_gaps");
    assert!(
        stub.seen
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.starts_with("/volumes/gShared1")),
        "the full record is fetched for its cover"
    );

    // ------------------------------------------------------- 8. forget
    let (s, r) = client
        .req(
            Method::DELETE,
            &format!("/api/v1/admin/audiobooks/{c_id}/match"),
            None,
        )
        .await;
    assert_eq!((s, r["forgotten"].clone()), (200, json!(true)));
    assert_eq!(client.book(&c_id).await["match"], Value::Null);
    // The files written beside it stay, and now speak for it.
    scan().await.unwrap();
    let c4 = client.book(&c_id).await;
    assert_eq!(strs(&c4["authors"]), ["Ursula Online"]);
    assert_eq!(poster(&pool, &c_id).await[0].0, "file");
    // Removed by hand, the book is its folder again — AB2's reading.
    std::fs::remove_file(c_dir.join("book.parlour.json")).unwrap();
    std::fs::remove_file(c_dir.join("cover.jpg")).unwrap();
    scan().await.unwrap();
    let c5 = client.book(&c_id).await;
    assert_eq!(c5["title"], "Untagged Book");
    assert_eq!(strs(&c5["authors"]), ["Plain"]);
    assert_eq!(c5["overview"], Value::Null);
    assert_eq!(c5["has_cover"], false);
    let report = scan().await.unwrap();
    assert_eq!(report.probed_files, 0, "idempotent to the end");

    let _ = std::fs::remove_dir_all(&root);
}
