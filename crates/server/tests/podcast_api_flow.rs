//! End-to-end test of POD5 (the listener API for podcasts; D31/D32 as
//! amended 2026-11-27, D41) against a real Postgres, the real router, a
//! local stub feed + enclosure host (never the real internet), real
//! ffmpeg/ffprobe and a throwaway folder as the podcast store:
//!
//! - **grants**: a profile without a grant on the podcasts library sees
//!   no show, and its episodes, streams, progress and reports are 404;
//! - **D41**: with a profile's explicit flag off (only an admin can turn
//!   it off) its explicit shows and their episodes, streams (even with a
//!   token minted while it was on), progress and reports are 404, while
//!   another profile is unaffected;
//! - **shows and episodes**: counts, downloaded state, lengths, the
//!   episodic (newest first) and serial (oldest first) orders, and this
//!   profile's progress inline;
//! - **stream**: a downloaded episode with Range (token or session); an
//!   undownloaded one is a 302 to its enclosure, `no-store`; AB10's AAC
//!   fallback for a file the `web` profile can't play; an admin stop
//!   revokes the episode's token;
//! - **progress**: clamp, auto-finish, un-finish when moved back,
//!   un-finish by hand to 0, the hide flag cleared by a move, the
//!   client's timestamp stored, the feed's length when there is no
//!   file, no clamp when no length is known, delete;
//! - **finish rule**: the podcasts library's twin of AB11's settings,
//!   read by the next write;
//! - **sessions**: start / progress / replace / stop on an episode, Now
//!   Playing, the profile's history and the Activity line;
//! - **OPML**: the listener export, admins only;
//! - **the token** in feed and enclosure addresses never appears in an
//!   answer body, an error, a job, a notice or the Activity feed — only
//!   in the redirect's `Location`.
//!
//! Ignored by default; needs PARLOUR_TEST_DATABASE_URL (a fresh
//! database) and ffmpeg/ffprobe on the PATH.
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test podcast_api_flow -- --ignored

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use parlour_server::config::Config;
use parlour_server::http::AppState;
use reqwest::Method;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

/// The private token every stub address carries.
const TOKEN: &str = "fake-feed-token";

// ------------------------------------------------------------ stub host

/// name → (content type, body)
type Files = HashMap<String, (&'static str, Vec<u8>)>;

#[derive(Clone, Default)]
struct Stub {
    files: Arc<Mutex<Files>>,
}

impl Stub {
    fn put(&self, name: &str, content_type: &'static str, body: impl Into<Vec<u8>>) {
        self.files
            .lock()
            .unwrap()
            .insert(name.into(), (content_type, body.into()));
    }
}

async fn stub_file(State(stub): State<Stub>, UrlPath(name): UrlPath<String>) -> Response {
    match stub.files.lock().unwrap().get(&name).cloned() {
        Some((ct, body)) => {
            let mut h = HeaderMap::new();
            h.insert("content-type", ct.parse().unwrap());
            (StatusCode::OK, h, body).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// One feed item: (guid, title, day in November 2026, enclosure URL,
/// enclosure type, `itunes:duration`).
type Item<'a> = (&'a str, &'a str, u32, String, &'a str, Option<&'a str>);

fn feed(title: &str, explicit: bool, serial: bool, items: &[Item]) -> String {
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\" \
         xmlns:itunes=\"http://www.itunes.com/dtds/podcast-1.0.dtd\">\n\
         <channel><title>{title}</title><description>A test show</description>\
         <itunes:explicit>{}</itunes:explicit><itunes:type>{}</itunes:type>\n",
        if explicit { "true" } else { "false" },
        if serial { "serial" } else { "episodic" },
    );
    for (guid, ep_title, day, url, ty, duration) in items {
        xml.push_str(&format!(
            "<item><guid>{guid}</guid><title>{ep_title}</title>\
             <pubDate>{day:02} Nov 2026 07:00:00 +0000</pubDate>{}\
             <enclosure url=\"{}\" type=\"{ty}\" length=\"1000\"/></item>\n",
            duration
                .map(|d| format!("<itunes:duration>{d}</itunes:duration>"))
                .unwrap_or_default(),
            url.replace('&', "&amp;")
        ));
    }
    xml.push_str("</channel></rss>\n");
    xml
}

fn tone(dir: &Path, name: &str, codec: &[&str]) -> Vec<u8> {
    let out = dir.join(name);
    let status = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2",
        ])
        .args(codec)
        .arg(&out)
        .status()
        .expect("ffmpeg runs");
    assert!(status.success(), "ffmpeg made {name}");
    std::fs::read(&out).unwrap()
}

// ------------------------------------------------------------ client

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

#[derive(Clone)]
struct Client {
    http: reqwest::Client,
    base: String,
    cookie: Option<String>,
}

impl Client {
    async fn raw(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        range: Option<&str>,
    ) -> reqwest::Response {
        let mut rb = self.http.request(method, format!("{}{path}", self.base));
        if let Some(c) = &self.cookie {
            rb = rb.header("Cookie", format!("parlour_session={c}"));
        }
        if let Some(r) = range {
            rb = rb.header("Range", r);
        }
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        rb.send().await.expect("request")
    }

    /// (status, JSON). Every answer outside the admin surface is
    /// checked for the token (admins see a feed's address on its POD4
    /// show page, by design: POD4 reading 6).
    async fn req(&self, method: Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let res = self.raw(method, path, body, None).await;
        let status = res.status().as_u16();
        let text = res.text().await.unwrap_or_default();
        if !path.starts_with("/api/v1/admin/") || status >= 400 {
            no_token(path, &text);
        }
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn get(&self, path: &str) -> Value {
        let (s, v) = self.req(Method::GET, path, None).await;
        assert_eq!(s, 200, "{path}: {v}");
        v
    }

    async fn put(&self, path: &str, body: Value) -> (u16, Value) {
        self.req(Method::PUT, path, Some(body)).await
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        self.req(Method::POST, path, Some(body)).await
    }
}

async fn login(http: &reqwest::Client, base: &str, user: &str) -> (String, Value) {
    let res = http
        .post(format!("{base}/api/v1/auth/login"))
        .json(&json!({"username": user, "password": "admin-pass-123"}))
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

async fn select(c: &Client, profile: &str) {
    let (s, v) = c
        .post(&format!("/api/v1/profiles/{profile}/select"), json!({}))
        .await;
    assert_eq!(s, 200, "{v}");
}

async fn run_queued(pool: &PgPool, http: &reqwest::Client) {
    let job = parlour_jobs::claim_next(pool, parlour_jobs::JobKind::RefreshPodcasts)
        .await
        .unwrap()
        .expect("a queued refresh_podcasts job");
    let id = job.id;
    parlour_server::podcasts::run_refresh_job_claimed(pool, http, job, None).await;
    let job = parlour_jobs::get(pool, id).await.unwrap().unwrap();
    assert_eq!(job.status, "succeeded", "{job:?}");
}

fn no_token(what: &str, text: &str) {
    assert!(!text.contains(TOKEN), "{what} shows the token: {text}");
}

fn episode<'a>(show: &'a Value, title: &str) -> &'a Value {
    show["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["title"] == title)
        .unwrap_or_else(|| panic!("no episode {title} in {show}"))
}

fn titles(show: &Value) -> Vec<String> {
    show["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["title"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL and ffmpeg"]
async fn podcast_api_flow() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test db url");
    let config = test_config(&db_url);
    let pool = parlour_db::connect(&config.database_url).await.unwrap();
    parlour_db::migrate(&pool).await.unwrap();
    parlour_server::auth::bootstrap_admin(&pool, &config)
        .await
        .unwrap();

    let root: PathBuf =
        std::env::temp_dir().join(format!("parlour-pod5-flow-{}", Uuid::new_v4().simple()));
    let store = root.join("podcasts");
    let books = root.join("books");
    let cache = root.join("cache");
    for d in [&store, &books, &cache] {
        std::fs::create_dir_all(d).unwrap();
    }

    // ------------------------------------------------------------ stub host
    let stub = Stub::default();
    let stub_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let feeds = format!("http://{}", stub_listener.local_addr().unwrap());
    let stub_app = Router::new()
        .route("/f/{name}", get(stub_file))
        .with_state(stub.clone());
    tokio::spawn(async move { axum::serve(stub_listener, stub_app).await.unwrap() });
    let at = |name: &str| format!("{feeds}/f/{name}?token={TOKEN}");

    let mp3 = tone(&root, "one.mp3", &["-c:a", "libmp3lame", "-b:a", "64k"]);
    let flac = tone(&root, "two.flac", &["-c:a", "flac"]);
    stub.put("one.mp3", "audio/mpeg", mp3.clone());
    stub.put("two.flac", "audio/flac", flac);
    stub.put("three.mp3", "audio/mpeg", mp3.clone());
    stub.put(
        "tones.xml",
        "application/rss+xml",
        feed(
            "Tone Radio",
            false,
            false,
            &[
                ("t1", "Tone one", 10, at("one.mp3"), "audio/mpeg", None),
                ("t2", "Tone two", 12, at("two.flac"), "audio/flac", None),
                (
                    "t3",
                    "Tone three",
                    14,
                    at("three.mp3"),
                    "audio/mpeg",
                    Some("00:01:40"),
                ),
                ("t4", "Tone four", 16, at("four.mp3"), "audio/mpeg", None),
            ],
        ),
    );
    stub.put(
        "spicy.xml",
        "application/rss+xml",
        feed(
            "Spicy Serial",
            true,
            true,
            &[
                (
                    "s2",
                    "Chapter two",
                    20,
                    at("three.mp3"),
                    "audio/mpeg",
                    Some("100"),
                ),
                (
                    "s1",
                    "Chapter one",
                    18,
                    at("three.mp3"),
                    "audio/mpeg",
                    Some("100"),
                ),
            ],
        ),
    );

    // ------------------------------------------------------------ server
    let mut state = AppState::new(pool.clone(), None, false);
    std::sync::Arc::get_mut(&mut state.playback)
        .unwrap()
        .configure(cache.clone(), None);
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
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let base = format!("http://{addr}");
    let (cookie, me) = login(&http, &base, "jordan").await;
    let admin = Client {
        http: http.clone(),
        base: base.clone(),
        cookie: Some(cookie),
    };
    let anon = Client {
        http: http.clone(),
        base: base.clone(),
        cookie: None,
    };
    let main_profile = me["profiles"][0]["id"].as_str().unwrap().to_string();
    select(&admin, &main_profile).await;
    let (s, kid) = admin.post("/api/v1/profiles", json!({"name": "Kid"})).await;
    assert_eq!(s, 201, "{kid}");
    let kid_profile = kid["id"].as_str().unwrap().to_string();
    let (kid_cookie, _) = login(&http, &base, "jordan").await;
    let kid = Client {
        http: http.clone(),
        base: base.clone(),
        cookie: Some(kid_cookie),
    };
    select(&kid, &kid_profile).await;

    let mut libs = Vec::new();
    for (name, kind, path) in [
        ("Podcasts", "podcasts", &store),
        ("Books", "audiobooks", &books),
    ] {
        let (s, lib) = admin
            .post(
                "/api/v1/admin/libraries",
                json!({"name": name, "kind": kind, "paths": [path.to_string_lossy()]}),
            )
            .await;
        assert_eq!(s, 201, "{lib}");
        libs.push(lib["id"].as_str().unwrap().to_string());
    }
    let (lib, book_lib) = (libs[0].clone(), libs[1].clone());
    let mut show_ids = Vec::new();
    for name in ["tones.xml", "spicy.xml"] {
        let (s, show) = admin
            .post(
                &format!("/api/v1/admin/libraries/{lib}/podcasts"),
                json!({"feed_url": at(name)}),
            )
            .await;
        assert_eq!(s, 201, "{show}");
        show_ids.push(show["id"].as_str().unwrap().to_string());
    }
    let (tones, spicy) = (show_ids[0].clone(), show_ids[1].clone());
    let feed_http = parlour_catalog::podcasts::http_client();
    let admin_show = admin.get(&format!("/api/v1/admin/podcasts/{tones}")).await;
    let id_of = |title: &str| {
        episode(&admin_show, title)["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let (ep1, ep2, ep3, ep4) = (
        id_of("Tone one"),
        id_of("Tone two"),
        id_of("Tone three"),
        id_of("Tone four"),
    );
    for ep in [&ep1, &ep2] {
        let (s, v) = admin
            .post(
                &format!("/api/v1/admin/podcast-episodes/{ep}/download"),
                json!({}),
            )
            .await;
        assert_eq!(s, 202, "{v}");
        run_queued(&pool, &feed_http).await;
    }
    let spicy_admin = admin.get(&format!("/api/v1/admin/podcasts/{spicy}")).await;
    let spicy_ep = episode(&spicy_admin, "Chapter one")["id"]
        .as_str()
        .unwrap()
        .to_string();

    // ------------------------------------------------- 1. no grant, nothing
    assert_eq!(admin.get("/api/v1/podcasts").await, json!([]));
    for path in [
        format!("/api/v1/podcasts/{tones}"),
        format!("/api/v1/podcast-episodes/{ep1}"),
        format!("/api/v1/podcast-episodes/{ep1}/progress"),
        format!("/api/v1/podcast-episodes/{ep1}/stream"),
    ] {
        let (s, v) = admin.req(Method::GET, &path, None).await;
        assert_eq!(s, 404, "{path}: {v}");
    }
    let (s, _) = admin
        .post(
            "/api/v1/playback/start",
            json!({"episode_id": ep1, "position_ms": 0}),
        )
        .await;
    assert_eq!(s, 404, "no grant, no session");
    for profile in [&main_profile, &kid_profile] {
        let (s, v) = admin
            .post(
                &format!("/api/v1/admin/libraries/{lib}/access"),
                json!({"profile_id": profile}),
            )
            .await;
        assert_eq!(s, 204, "{v}");
    }

    // ------------------------------------------- 2. shows and episode lists
    let shows = admin.get("/api/v1/podcasts").await;
    let names: Vec<&str> = shows
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["title"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Spicy Serial", "Tone Radio"], "by sort title");
    let tone_summary = &shows[1];
    assert_eq!(tone_summary["episode_count"], 4);
    assert_eq!(tone_summary["downloaded_count"], 2);
    assert_eq!(tone_summary["explicit"], false);
    assert_eq!(shows[0]["explicit"], true);
    assert_eq!(
        admin
            .get(&format!("/api/v1/podcasts?library_id={book_lib}"))
            .await,
        json!([])
    );
    assert_eq!(
        admin.get("/api/v1/podcasts?q=spicy").await[0]["title"],
        "Spicy Serial"
    );

    let show = admin.get(&format!("/api/v1/podcasts/{tones}")).await;
    assert_eq!(
        titles(&show),
        ["Tone four", "Tone three", "Tone two", "Tone one"],
        "episodic: newest first"
    );
    let serial = admin.get(&format!("/api/v1/podcasts/{spicy}")).await;
    assert_eq!(
        titles(&serial),
        ["Chapter one", "Chapter two"],
        "serial: oldest first"
    );
    let e1 = episode(&show, "Tone one");
    assert_eq!(e1["downloaded"], true);
    assert_eq!(e1["delivery"], "file");
    assert!(e1["plan"].is_null(), "no client named");
    assert!(e1["file_duration_ms"].as_i64().unwrap() > 1500);
    assert_eq!(e1["length_ms"], e1["file_duration_ms"]);
    let e3 = episode(&show, "Tone three");
    assert_eq!(e3["downloaded"], false);
    assert_eq!(e3["delivery"], "enclosure");
    assert_eq!(e3["duration_ms"], 100_000);
    assert_eq!(e3["length_ms"], 100_000, "the feed's length stands in");
    assert_eq!(e3["content_type"], "audio/mpeg");
    assert!(episode(&show, "Tone four")["length_ms"].is_null());
    assert!(
        !show.to_string().contains("enclosure_url") && !show.to_string().contains("feed_url"),
        "no publisher address in a listener's answer: {show}"
    );

    // ------------------------------------------------------- 3. streams
    let e1_url = e1["stream_url"].as_str().unwrap().to_string();
    assert!(e1_url.starts_with(&format!("/api/v1/podcast-episodes/{ep1}/stream?token=")));
    let res = anon.raw(Method::GET, &e1_url, None, None).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "audio/mpeg");
    assert_eq!(res.headers()["accept-ranges"], "bytes");
    let full = res.bytes().await.unwrap();
    assert_eq!(full.len(), mp3.len(), "the downloaded file, whole");
    let res = anon
        .raw(Method::GET, &e1_url, None, Some("bytes=0-99"))
        .await;
    assert_eq!(res.status(), 206);
    assert_eq!(
        res.headers()["content-range"],
        format!("bytes 0-99/{}", mp3.len()).as_str()
    );
    assert_eq!(res.bytes().await.unwrap().len(), 100);
    // A session works without a token; nobody else does.
    let res = admin
        .raw(
            Method::GET,
            &format!("/api/v1/podcast-episodes/{ep1}/stream"),
            None,
            Some("bytes=10-19"),
        )
        .await;
    assert_eq!(res.status(), 206);
    for path in [
        format!("/api/v1/podcast-episodes/{ep1}/stream"),
        // A real token with its signature broken (no literal token here).
        format!("{e1_url}x"),
        format!("/api/v1/podcast-episodes/{ep3}/stream"),
        format!("/api/v1/podcast-episodes/{}/stream", Uuid::new_v4()),
        "/api/v1/podcast-episodes/not-a-uuid/stream".to_string(),
    ] {
        let (s, v) = anon.req(Method::GET, &path, None).await;
        assert_eq!(s, 401, "{path}: {v}");
    }
    // A token for one episode doesn't open another.
    let e1_token = e1_url.split("token=").nth(1).unwrap().to_string();
    let (s, _) = anon
        .req(
            Method::GET,
            &format!("/api/v1/podcast-episodes/{ep2}/stream?token={e1_token}"),
            None,
        )
        .await;
    assert_eq!(s, 401);

    // Not downloaded: a redirect to the enclosure, and only there.
    let e3_url = e3["stream_url"].as_str().unwrap().to_string();
    let res = anon.raw(Method::GET, &e3_url, None, None).await;
    assert_eq!(res.status(), 302);
    assert_eq!(res.headers()["location"], at("three.mp3").as_str());
    assert_eq!(res.headers()["cache-control"], "no-store");
    assert_eq!(res.headers()["referrer-policy"], "no-referrer");
    assert!(res.text().await.unwrap().is_empty(), "no body");
    let followed = reqwest::get(at("three.mp3")).await.unwrap();
    assert_eq!(followed.bytes().await.unwrap().len(), mp3.len());
    let (s, v) = anon
        .req(
            Method::GET,
            &format!(
                "/api/v1/podcast-episodes/{ep3}/aac.m4a?{}",
                e3_url.split('?').nth(1).unwrap()
            ),
            None,
        )
        .await;
    assert_eq!(s, 404, "no AAC copy of an enclosure: {v}");

    // AB10's plan and the AAC fallback, widened to episodes.
    let d1 = admin
        .get(&format!("/api/v1/podcast-episodes/{ep1}?client=web"))
        .await;
    assert_eq!(d1["plan"]["action"], "DirectPlay");
    assert_eq!(d1["delivery"], "file");
    assert_eq!(d1["show_title"], "Tone Radio");
    let d2 = admin
        .get(&format!("/api/v1/podcast-episodes/{ep2}?client=web"))
        .await;
    assert_eq!(d2["codec"], "flac");
    assert_eq!(d2["plan"]["action"], "Transcode", "{d2}");
    assert_eq!(d2["delivery"], "aac");
    assert_eq!(d2["content_type"], "audio/mp4");
    let d2_url = d2["stream_url"].as_str().unwrap().to_string();
    assert!(d2_url.contains("/aac.m4a?token="), "{d2_url}");
    let android = admin
        .get(&format!("/api/v1/podcast-episodes/{ep2}?client=android"))
        .await;
    assert_eq!(android["plan"]["action"], "DirectPlay", "FLAC plays there");
    let d3 = admin
        .get(&format!("/api/v1/podcast-episodes/{ep3}?client=web"))
        .await;
    assert!(d3["plan"].is_null(), "an enclosure has no plan");
    assert_eq!(d3["delivery"], "enclosure");
    let (s, _) = admin
        .req(
            Method::GET,
            &format!("/api/v1/podcast-episodes/{ep2}?client=toaster"),
            None,
        )
        .await;
    assert_eq!(s, 400);
    let mut aac = None;
    for _ in 0..120 {
        let res = anon.raw(Method::GET, &d2_url, None, Some("bytes=0-")).await;
        match res.status().as_u16() {
            503 => tokio::time::sleep(std::time::Duration::from_millis(500)).await,
            206 => {
                assert_eq!(res.headers()["content-type"], "audio/mp4");
                aac = Some(res.bytes().await.unwrap());
                break;
            }
            other => panic!("AAC copy answered {other}"),
        }
    }
    let aac = aac.expect("the AAC copy was made");
    assert_eq!(&aac[4..8], b"ftyp", "an MP4 file");

    // ------------------------------------------------------ 4. progress
    let prog = |ep: &str| format!("/api/v1/podcast-episodes/{ep}/progress");
    let p = admin.get(&prog(&ep3)).await;
    assert_eq!(
        (&p["position_ms"], &p["finished"]),
        (&json!(0), &json!(false))
    );
    let (s, p) = admin
        .put(
            &prog(&ep3),
            json!({"position_ms": 30_000, "client_updated_at": "2026-11-20T10:00:00Z"}),
        )
        .await;
    assert_eq!(s, 200, "{p}");
    assert_eq!(p["position_ms"], 30_000);
    assert_eq!(
        p["duration_ms"], 100_000,
        "the feed's length, not downloaded"
    );
    assert!(p["last_listened_at"].is_string());
    assert_eq!(p["client_updated_at"], "2026-11-20T10:00:00Z");
    let (_, p) = admin
        .put(&prog(&ep3), json!({"position_ms": 500_000}))
        .await;
    assert_eq!(
        (&p["position_ms"], &p["finished"]),
        (&json!(100_000), &json!(true)),
        "clamped to the length, and finished: {p}"
    );
    assert!(p["finished_at"].is_string());
    let (_, p) = admin.put(&prog(&ep3), json!({"position_ms": 20_000})).await;
    assert_eq!(
        (&p["position_ms"], &p["finished"]),
        (&json!(20_000), &json!(false)),
        "moved back: unfinished"
    );
    let (_, p) = admin.put(&prog(&ep3), json!({"position_ms": 91_000})).await;
    assert_eq!(p["finished"], true, "under 10 s left finishes (ABS's rule)");
    let (_, p) = admin.put(&prog(&ep3), json!({"finished": false})).await;
    assert_eq!(
        (&p["position_ms"], &p["finished"]),
        (&json!(0), &json!(false)),
        "un-finished by hand: back to 0"
    );
    let (_, p) = admin.put(&prog(&ep3), json!({"finished": true})).await;
    assert_eq!(
        p["finished"], true,
        "a mark-finished on an undownloaded episode"
    );
    let (_, p) = admin.put(&prog(&ep3), json!({"hidden": true})).await;
    assert_eq!(p["hidden"], true);
    let (_, p) = admin.put(&prog(&ep3), json!({"position_ms": 5_000})).await;
    assert_eq!(p["hidden"], false, "a move clears the hide flag");
    assert_eq!(p["finished"], false);
    for (body, what) in [
        (json!({}), "nothing to write"),
        (json!({"position_ms": -1}), "negative"),
    ] {
        let (s, _) = admin.put(&prog(&ep3), body).await;
        assert_eq!(s, 400, "{what}");
    }
    // No length known: no clamp, no auto-finish.
    let (_, p) = admin
        .put(&prog(&ep4), json!({"position_ms": 900_000_000}))
        .await;
    assert_eq!(
        (&p["position_ms"], &p["finished"]),
        (&json!(900_000_000), &json!(false))
    );
    // A downloaded episode's length is its file's.
    let (_, p) = admin.put(&prog(&ep1), json!({"position_ms": 60_000})).await;
    assert_eq!(p["position_ms"], e1["file_duration_ms"]);
    assert_eq!(p["finished"], true);
    // Inline, per profile; the Kid's progress is its own.
    let show = admin.get(&format!("/api/v1/podcasts/{tones}")).await;
    assert_eq!(
        episode(&show, "Tone three")["progress"]["position_ms"],
        5_000
    );
    let summary = admin.get("/api/v1/podcasts").await;
    assert_eq!(summary[1]["finished_count"], 1);
    assert_eq!(summary[1]["in_progress_count"], 2);
    let kid_show = kid.get(&format!("/api/v1/podcasts/{tones}")).await;
    assert_eq!(
        episode(&kid_show, "Tone three")["progress"]["position_ms"],
        0
    );
    let (_, p) = kid.put(&prog(&ep3), json!({"position_ms": 42_000})).await;
    assert_eq!(p["position_ms"], 42_000);
    assert_eq!(admin.get(&prog(&ep3)).await["position_ms"], 5_000);
    // Episode rows stay out of the book sync.
    let sync = admin.get("/api/v1/audiobooks/progress").await;
    assert_eq!(sync["progress"], json!([]));
    let (s, _) = admin.req(Method::DELETE, &prog(&ep4), None).await;
    assert_eq!(s, 204);
    assert_eq!(admin.get(&prog(&ep4)).await["position_ms"], 0);

    // ------------------------------------------- 5. the library's finish rule
    let settings = format!("/api/v1/admin/libraries/{lib}/podcast-settings");
    let v = admin.get(&settings).await;
    assert_eq!(v["finish_remaining_seconds"], 10);
    assert!(v["finish_percent"].is_null());
    assert_eq!(v["default"]["finish_remaining_seconds"], 10);
    for body in [
        json!({"finish_remaining_seconds": -1, "finish_percent": null}),
        json!({"finish_remaining_seconds": 10, "finish_percent": 0}),
        json!({"finish_percent": null}),
        json!({"finish_remaining_seconds": 10, "finish_percent": null, "parse_subtitles": true}),
    ] {
        let (s, _) = admin.put(&settings, body.clone()).await;
        assert!(s == 400 || s == 422, "{body}: {s}");
    }
    assert_eq!(admin.get(&settings).await["finish_remaining_seconds"], 10);
    let (s, _) = admin
        .req(
            Method::GET,
            &format!("/api/v1/admin/libraries/{book_lib}/podcast-settings"),
            None,
        )
        .await;
    assert_eq!(s, 400, "an audiobooks library has no podcast settings");
    let (s, _) = admin
        .req(
            Method::GET,
            &format!("/api/v1/admin/libraries/{lib}/audiobook-settings"),
            None,
        )
        .await;
    assert_eq!(s, 400, "and the other way round");
    let (s, v) = admin
        .put(
            &settings,
            json!({"finish_remaining_seconds": 0, "finish_percent": null}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let (_, p) = admin.put(&prog(&ep3), json!({"position_ms": 99_900})).await;
    assert_eq!(p["finished"], false, "the time rule is off");
    admin
        .put(
            &settings,
            json!({"finish_remaining_seconds": 10, "finish_percent": 50}),
        )
        .await;
    let (_, p) = admin.put(&prog(&ep3), json!({"position_ms": 40_000})).await;
    assert_eq!(p["finished"], false);
    let (_, p) = admin.put(&prog(&ep3), json!({"position_ms": 50_001})).await;
    assert_eq!(p["finished"], true, "past 50 %");
    admin
        .put(
            &settings,
            json!({"finish_remaining_seconds": 10, "finish_percent": null}),
        )
        .await;

    // ------------------------------------------------------- 6. sessions
    let (s, v) = admin
        .post(
            "/api/v1/playback/start",
            json!({"episode_id": ep3, "item_id": Uuid::new_v4(), "position_ms": 0}),
        )
        .await;
    assert_eq!(s, 400, "item_id and episode_id: {v}");
    let (s, v) = admin
        .post("/api/v1/playback/start", json!({"position_ms": 0}))
        .await;
    assert_eq!(s, 400, "neither: {v}");
    let (s, started) = admin
        .post(
            "/api/v1/playback/start",
            json!({"episode_id": ep3, "position_ms": 1_000, "duration_ms": 100_000,
                   "client_name": "Flow"}),
        )
        .await;
    assert_eq!(s, 200, "{started}");
    let first = started["session_id"].as_str().unwrap().to_string();
    let (n,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM listening_sessions
         WHERE episode_id = $1 AND item_id IS NULL AND ended_at IS NULL",
    )
    .bind(Uuid::parse_str(&ep3).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
    let now_playing = admin.get("/api/v1/admin/sessions").await;
    let np = now_playing
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == first.as_str())
        .expect("on Now Playing");
    assert_eq!(np["item_kind"], "podcast_episode");
    assert_eq!(np["item_title"], "Tone three");
    assert_eq!(np["series_title"], "Tone Radio");
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let (s, v) = admin
        .post(
            "/api/v1/playback/progress",
            json!({"episode_id": ep3, "session_id": first, "position_ms": 2_000,
                   "listened_ms": 1_000}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    // A new start replaces the open one, then a stop closes it.
    let (_, again) = admin
        .post(
            "/api/v1/playback/start",
            json!({"episode_id": ep3, "position_ms": 2_000}),
        )
        .await;
    let second = again["session_id"].as_str().unwrap().to_string();
    let (s, _) = admin
        .post(
            "/api/v1/playback/stopped",
            json!({"episode_id": ep3, "session_id": second, "position_ms": 3_000}),
        )
        .await;
    assert_eq!(s, 204);
    let reasons: Vec<(String, i64)> = sqlx::query_as(
        "SELECT end_reason, listened_ms FROM listening_sessions
         WHERE episode_id = $1 ORDER BY started_at",
    )
    .bind(Uuid::parse_str(&ep3).unwrap())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        reasons,
        [
            ("replaced".to_string(), 1_000),
            ("stopped".to_string(), 1_000)
        ],
        "the stop sent no measure: the position's 1 s forward counts (AB8)"
    );
    let history = admin.get("/api/v1/listening/sessions").await;
    let mine = &history["sessions"][0];
    assert_eq!(mine["episode_id"], ep3.as_str());
    assert!(mine["item_id"].is_null());
    assert_eq!(mine["show_title"], "Tone Radio");
    assert_eq!(mine["title"], "Tone three");
    let stats = admin.get("/api/v1/listening/stats").await;
    assert_eq!(stats["total_ms"], 2_000, "an episode's listening counts");
    let lines: Vec<(String, Option<Uuid>, Value)> = sqlx::query_as(
        "SELECT summary, item_id, detail FROM activity
         WHERE kind = 'playback.listened' ORDER BY created_at",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].0.contains("Tone Radio · Tone three"), "{lines:?}");
    assert!(lines[0].1.is_none(), "an episode isn't a catalog item");
    assert_eq!(lines[0].2["episode_id"], ep3.as_str());
    assert_eq!(lines[0].2["podcast_id"], tones.as_str());

    // An admin stop revokes the episode's stream token.
    let d1 = admin.get(&format!("/api/v1/podcast-episodes/{ep1}")).await;
    let d1_url = d1["stream_url"].as_str().unwrap().to_string();
    let (_, started) = admin
        .post(
            "/api/v1/playback/start",
            json!({"episode_id": ep1, "position_ms": 0}),
        )
        .await;
    let sid = started["session_id"].as_str().unwrap().to_string();
    assert_eq!(
        anon.raw(Method::GET, &d1_url, None, None).await.status(),
        200
    );
    let (s, _) = admin
        .post(&format!("/api/v1/admin/sessions/{sid}/stop"), json!({}))
        .await;
    assert_eq!(s, 204);
    let (s, v) = anon.req(Method::GET, &d1_url, None).await;
    assert_eq!(s, 401, "a stopped stream stays stopped: {v}");
    let (s, _) = admin
        .post(
            "/api/v1/playback/progress",
            json!({"episode_id": ep1, "session_id": sid, "position_ms": 500}),
        )
        .await;
    assert_eq!(s, 409);
    let (reason,): (String,) =
        sqlx::query_as("SELECT end_reason FROM listening_sessions WHERE id = $1")
            .bind(Uuid::parse_str(&sid).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(reason, "admin");

    // ------------------------------------------------ 7. D41: explicit off
    let spicy_detail = kid
        .get(&format!("/api/v1/podcast-episodes/{spicy_ep}"))
        .await;
    let kid_token_url = spicy_detail["stream_url"].as_str().unwrap().to_string();
    let (_, p) = kid
        .put(&prog(&spicy_ep), json!({"position_ms": 10_000}))
        .await;
    assert_eq!(p["position_ms"], 10_000);
    // Only an admin sets the flag; a profile's own edit can't.
    let invite_token = {
        let (s, inv) = admin.post("/api/v1/admin/invites", json!({})).await;
        assert_eq!(s, 201, "{inv}");
        inv["token"].as_str().unwrap().to_string()
    };
    let (s, v) = anon
        .post(
            "/api/v1/invites/accept",
            json!({"token": invite_token, "username": "sam", "password": "admin-pass-123"}),
        )
        .await;
    assert_eq!(s, 201, "{v}");
    let (sam_cookie, sam_me) = login(&http, &base, "sam").await;
    let sam = Client {
        http: http.clone(),
        base: base.clone(),
        cookie: Some(sam_cookie),
    };
    let sam_profile = sam_me["profiles"][0]["id"].as_str().unwrap().to_string();
    select(&sam, &sam_profile).await;
    for (method, path, body) in [
        (Method::GET, "/api/v1/admin/profiles".to_string(), None),
        (
            Method::PUT,
            format!("/api/v1/admin/profiles/{sam_profile}/explicit"),
            Some(json!({"allow_explicit": false})),
        ),
        (
            Method::GET,
            format!("/api/v1/admin/libraries/{lib}/podcast-settings"),
            None,
        ),
        (Method::GET, "/api/v1/podcasts/opml".to_string(), None),
    ] {
        let (s, v) = sam.req(method, &path, body).await;
        assert_eq!(s, 404, "{path}: {v}");
    }
    let (s, _) = sam
        .req(
            Method::PATCH,
            &format!("/api/v1/profiles/{sam_profile}"),
            Some(json!({"allow_explicit": false})),
        )
        .await;
    assert!(s == 200 || s == 422, "{s}");
    let all = admin.get("/api/v1/admin/profiles").await;
    let flag = |id: &str| {
        all.as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .unwrap()["allow_explicit"]
            .clone()
    };
    assert_eq!(flag(&sam_profile), true, "on by default, and unchanged");
    assert_eq!(flag(&kid_profile), true);
    let (s, v) = admin
        .put(
            &format!("/api/v1/admin/profiles/{kid_profile}/explicit"),
            json!({"allow_explicit": false}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["allow_explicit"], false);
    assert_eq!(v["name"], "Kid");
    let (s, _) = admin
        .put(
            &format!("/api/v1/admin/profiles/{kid_profile}/explicit"),
            json!({"allow": false}),
        )
        .await;
    assert_eq!(s, 422, "an unknown field is refused");

    let kid_shows = kid.get("/api/v1/podcasts").await;
    assert_eq!(kid_shows.as_array().unwrap().len(), 1, "{kid_shows}");
    assert_eq!(kid_shows[0]["title"], "Tone Radio");
    for path in [
        format!("/api/v1/podcasts/{spicy}"),
        format!("/api/v1/podcast-episodes/{spicy_ep}"),
        prog(&spicy_ep),
        format!("/api/v1/podcast-episodes/{spicy_ep}/stream"),
    ] {
        let (s, v) = kid.req(Method::GET, &path, None).await;
        assert_eq!(s, 404, "{path}: {v}");
    }
    let (s, _) = kid
        .put(&prog(&spicy_ep), json!({"position_ms": 20_000}))
        .await;
    assert_eq!(s, 404);
    let (s, _) = kid.req(Method::DELETE, &prog(&spicy_ep), None).await;
    assert_eq!(s, 404);
    let (s, _) = kid
        .post(
            "/api/v1/playback/start",
            json!({"episode_id": spicy_ep, "position_ms": 0}),
        )
        .await;
    assert_eq!(s, 404);
    let (s, _) = anon.req(Method::GET, &kid_token_url, None).await;
    assert_eq!(s, 404, "a token minted while it was on no longer plays");
    // The progress row is kept, only hidden from the profile.
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM book_progress WHERE episode_id = $1")
        .bind(Uuid::parse_str(&spicy_ep).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1);
    // Another profile is unaffected; turned back on, it all returns.
    assert_eq!(
        admin
            .get("/api/v1/podcasts")
            .await
            .as_array()
            .unwrap()
            .len(),
        2
    );
    admin
        .put(
            &format!("/api/v1/admin/profiles/{kid_profile}/explicit"),
            json!({"allow_explicit": true}),
        )
        .await;
    assert_eq!(kid.get(&prog(&spicy_ep)).await["position_ms"], 10_000);
    let changed: (i64,) =
        sqlx::query_as("SELECT count(*) FROM activity WHERE kind = 'profile.explicit_changed'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(changed.0, 2);

    // ------------------------------------------------------------ 8. OPML
    let res = admin
        .raw(Method::GET, "/api/v1/podcasts/opml", None, None)
        .await;
    assert_eq!(res.status(), 200);
    assert!(res.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/x-opml"));
    let opml = res.text().await.unwrap();
    let feeds = parlour_catalog::opml::parse_opml(opml.as_bytes()).unwrap();
    assert_eq!(feeds.len(), 2, "{opml}");
    // The export is the admin's own profile's view: D41 applies to it.
    admin
        .put(
            &format!("/api/v1/admin/profiles/{main_profile}/explicit"),
            json!({"allow_explicit": false}),
        )
        .await;
    let opml = admin
        .raw(
            Method::GET,
            &format!("/api/v1/podcasts/opml?library_id={lib}"),
            None,
            None,
        )
        .await
        .text()
        .await
        .unwrap();
    let feeds = parlour_catalog::opml::parse_opml(opml.as_bytes()).unwrap();
    assert_eq!(feeds.len(), 1, "{opml}");
    assert!(opml.contains("Tone Radio") && !opml.contains("Spicy Serial"));

    // --------------------------------------------- 9. the token stays out
    for (table, sql) in [
        (
            "jobs",
            "SELECT count(*) FROM jobs j WHERE to_jsonb(j)::text LIKE '%fake-feed-token%'",
        ),
        (
            "notices",
            "SELECT count(*) FROM notices n WHERE to_jsonb(n)::text LIKE '%fake-feed-token%'",
        ),
        (
            "activity",
            "SELECT count(*) FROM activity a WHERE to_jsonb(a)::text LIKE '%fake-feed-token%'",
        ),
        (
            "listening_sessions",
            "SELECT count(*) FROM listening_sessions s \
             WHERE to_jsonb(s)::text LIKE '%fake-feed-token%'",
        ),
    ] {
        let (n,): (i64,) = sqlx::query_as(sql).fetch_one(&pool).await.unwrap();
        assert_eq!(n, 0, "the token is in {table}");
    }

    let _ = std::fs::remove_dir_all(&root);
}
