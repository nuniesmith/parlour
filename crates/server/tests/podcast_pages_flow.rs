//! End-to-end test of POD6's server side (the web podcast pages and
//! player; D31/D32 as amended, D41) against a real Postgres, the real
//! router, a local stub feed + artwork + chapter host (never the real
//! internet), real ffmpeg/ffprobe, a throwaway podcast store and a
//! throwaway artwork cache:
//!
//! - **artwork**: a show's artwork is fetched into the cache after an
//!   admin adds it and after each check, served from Parlour's own route
//!   (grant and D41 checked, `nosniff`), versioned by a key that says
//!   nothing of the address; a new address replaces the copy, a
//!   non-image is refused (the older copy kept, the job naming the show,
//!   never the address), and a feed that drops its artwork drops the
//!   copy;
//! - **chapters**: a download's own chapter marks, else the feed's
//!   Podcasting 2.0 chapter list fetched server-side (image and link
//!   dropped), an unreadable list reported without its address, none;
//! - **stats**: listening per show, following D41's flag;
//! - **Now Playing and history** carry the show and its artwork;
//! - **Home's library tile** counts the shows the profile can reach;
//! - **the token** in feed, artwork, chapter and enclosure addresses
//!   never appears in a listener's answer, a job or the Activity feed.
//!
//! Ignored by default; needs PARLOUR_TEST_DATABASE_URL (a fresh
//! database) and ffmpeg/ffprobe on the PATH.
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test podcast_pages_flow -- --ignored

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
/// chapter-list URL).
type Item<'a> = (&'a str, &'a str, u32, String, Option<String>);

fn feed(title: &str, explicit: bool, image: Option<&str>, items: &[Item]) -> String {
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\" \
         xmlns:itunes=\"http://www.itunes.com/dtds/podcast-1.0.dtd\" \
         xmlns:podcast=\"https://podcastindex.org/namespace/1.0\">\n\
         <channel><title>{title}</title>\
         <description>&lt;p&gt;A &lt;b&gt;test&lt;/b&gt; show&lt;/p&gt;</description>\
         <itunes:explicit>{}</itunes:explicit>{}\n",
        if explicit { "true" } else { "false" },
        image
            .map(|u| format!("<itunes:image href=\"{}\"/>", u.replace('&', "&amp;")))
            .unwrap_or_default(),
    );
    for (guid, ep_title, day, url, chapters) in items {
        xml.push_str(&format!(
            "<item><guid>{guid}</guid><title>{ep_title}</title>\
             <pubDate>{day:02} Nov 2026 07:00:00 +0000</pubDate>\
             <itunes:duration>3</itunes:duration>{}\
             <enclosure url=\"{}\" type=\"audio/mpeg\" length=\"1000\"/></item>\n",
            chapters
                .as_ref()
                .map(|c| format!(
                    "<podcast:chapters url=\"{}\" type=\"application/json+chapters\"/>",
                    c.replace('&', "&amp;")
                ))
                .unwrap_or_default(),
            url.replace('&', "&amp;")
        ));
    }
    xml.push_str("</channel></rss>\n");
    xml
}

fn ffmpeg(dir: &Path, name: &str, args: &[&str]) -> Vec<u8> {
    let out = dir.join(name);
    let status = std::process::Command::new("ffmpeg")
        .current_dir(dir)
        .args(["-v", "error", "-y"])
        .args(args)
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
    async fn raw(&self, method: Method, path: &str, body: Option<Value>) -> reqwest::Response {
        let mut rb = self.http.request(method, format!("{}{path}", self.base));
        if let Some(c) = &self.cookie {
            rb = rb.header("Cookie", format!("parlour_session={c}"));
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
        let res = self.raw(method, path, body).await;
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

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        self.req(Method::POST, path, Some(body)).await
    }

    async fn put(&self, path: &str, body: Value) -> (u16, Value) {
        self.req(Method::PUT, path, Some(body)).await
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

async fn select(c: &Client, profile: &str) {
    let (s, v) = c
        .post(&format!("/api/v1/profiles/{profile}/select"), json!({}))
        .await;
    assert_eq!(s, 200, "{v}");
}

/// Claim the queued `refresh_podcasts` job and run it as `main` does,
/// with the artwork cache; the finished job.
async fn run_queued(pool: &PgPool, http: &reqwest::Client, artwork: &Path) -> parlour_jobs::Job {
    let job = parlour_jobs::claim_next(pool, parlour_jobs::JobKind::RefreshPodcasts)
        .await
        .unwrap()
        .expect("a queued refresh_podcasts job");
    let id = job.id;
    parlour_server::podcasts::run_refresh_job_claimed(pool, http, job, Some(artwork)).await;
    let job = parlour_jobs::get(pool, id).await.unwrap().unwrap();
    assert_eq!(job.status, "succeeded", "{job:?}");
    no_token("the job", &format!("{:?} {:?}", job.errors, job.result));
    job
}

fn no_token(what: &str, text: &str) {
    assert!(!text.contains(TOKEN), "{what} shows the token: {text}");
}

fn episode_id(show: &Value, title: &str) -> String {
    show["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["title"] == title)
        .unwrap_or_else(|| panic!("no episode {title} in {show}"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn show_in<'a>(list: &'a Value, title: &str) -> Option<&'a Value> {
    list.as_array()
        .unwrap()
        .iter()
        .find(|s| s["title"] == title)
}

/// The cache files of one show.
fn cached_files(artwork: &Path, show: &str) -> Vec<String> {
    let dir = artwork.join("podcasts").join(show);
    let mut out: Vec<String> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL and ffmpeg"]
async fn podcast_pages_flow() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test db url");
    let config = test_config(&db_url);
    let pool = parlour_db::connect(&config.database_url).await.unwrap();
    parlour_db::migrate(&pool).await.unwrap();
    parlour_server::auth::bootstrap_admin(&pool, &config)
        .await
        .unwrap();

    let root: PathBuf =
        std::env::temp_dir().join(format!("parlour-pod6-flow-{}", Uuid::new_v4().simple()));
    let store = root.join("podcasts");
    let artwork = root.join("artwork");
    let cache = root.join("cache");
    let lab = root.join("lab");
    for d in [&store, &artwork, &cache, &lab] {
        std::fs::create_dir_all(d).unwrap();
    }

    // ------------------------------------------------------------ fixtures
    let png = ffmpeg(
        &lab,
        "art.png",
        &["-f", "lavfi", "-i", "color=c=red:s=16x16", "-frames:v", "1"],
    );
    let jpg = ffmpeg(
        &lab,
        "art.jpg",
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=16x16",
            "-frames:v",
            "1",
        ],
    );
    std::fs::write(
        lab.join("meta.txt"),
        ";FFMETADATA1\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=1500\ntitle=Opening\n\
         [CHAPTER]\nTIMEBASE=1/1000\nSTART=1500\nEND=3000\ntitle=The rest\n",
    )
    .unwrap();
    let marked = ffmpeg(
        &lab,
        "marked.mp3",
        &[
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=3",
            "-i",
            "meta.txt",
            "-map",
            "0:a",
            "-map_chapters",
            "1",
            "-c:a",
            "libmp3lame",
            "-b:a",
            "64k",
        ],
    );
    let plain = ffmpeg(
        &lab,
        "plain.mp3",
        &[
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=330:duration=3",
            "-c:a",
            "libmp3lame",
            "-b:a",
            "64k",
        ],
    );

    let stub = Stub::default();
    let stub_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host = format!("http://{}", stub_listener.local_addr().unwrap());
    let stub_app = Router::new()
        .route("/f/{name}", get(stub_file))
        .with_state(stub.clone());
    tokio::spawn(async move { axum::serve(stub_listener, stub_app).await.unwrap() });
    let at = |name: &str| format!("{host}/f/{name}?token={TOKEN}");

    stub.put("art.png", "image/png", png.clone());
    stub.put("art.jpg", "image/jpeg", jpg.clone());
    stub.put(
        "art.svg",
        "image/svg+xml",
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>",
    );
    stub.put("night.jpg", "image/jpeg", jpg.clone());
    stub.put("marked.mp3", "audio/mpeg", marked);
    stub.put("plain.mp3", "audio/mpeg", plain);
    stub.put(
        "ch.json",
        "application/json+chapters",
        format!(
            "{{\"version\":\"1.2.0\",\"chapters\":[\
             {{\"startTime\":0,\"title\":\"Hello\",\"img\":\"{}\"}},\
             {{\"startTime\":1.25,\"title\":\"Sponsor\",\"url\":\"{}\"}},\
             {{\"startTime\":2,\"title\":\"Silent marker\",\"toc\":false}}]}}",
            at("chapter.jpg"),
            at("sponsor")
        ),
    );
    let lamp_items = |image: Option<&str>| {
        feed(
            "Lamp Radio",
            false,
            image,
            &[
                ("l1", "Lamp one", 10, at("marked.mp3"), None),
                ("l2", "Lamp two", 12, at("plain.mp3"), Some(at("ch.json"))),
                (
                    "l3",
                    "Lamp three",
                    14,
                    at("plain.mp3"),
                    Some(at("missing.json")),
                ),
                ("l4", "Lamp four", 16, at("plain.mp3"), None),
            ],
        )
    };
    stub.put(
        "lamp.xml",
        "application/rss+xml",
        lamp_items(Some(&at("art.png"))),
    );
    stub.put(
        "night.xml",
        "application/rss+xml",
        feed(
            "Night Serial",
            true,
            Some(&at("night.jpg")),
            &[("n1", "Night one", 18, at("plain.mp3"), None)],
        ),
    );

    // ------------------------------------------------------------ server
    let mut state = AppState::new(pool.clone(), None, false);
    state.metadata.artwork_dir = artwork.clone();
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
    let client = |cookie: Option<String>| Client {
        http: http.clone(),
        base: base.clone(),
        cookie,
    };
    let (cookie, me) = login(&http, &base).await;
    let admin = client(Some(cookie));
    let anon = client(None);
    let main_profile = me["profiles"][0]["id"].as_str().unwrap().to_string();
    select(&admin, &main_profile).await;
    let mut others = Vec::new();
    for name in ["Kid", "Guest"] {
        let (s, p) = admin.post("/api/v1/profiles", json!({"name": name})).await;
        assert_eq!(s, 201, "{p}");
        let id = p["id"].as_str().unwrap().to_string();
        let (c, _) = login(&http, &base).await;
        let c = client(Some(c));
        select(&c, &id).await;
        others.push((id, c));
    }
    let (kid_profile, kid) = others.remove(0);
    let (_guest_profile, guest) = others.remove(0);

    let (s, lib) = admin
        .post(
            "/api/v1/admin/libraries",
            json!({"name": "Podcasts", "kind": "podcasts", "paths": [store.to_string_lossy()]}),
        )
        .await;
    assert_eq!(s, 201, "{lib}");
    let lib = lib["id"].as_str().unwrap().to_string();
    for profile in [&main_profile, &kid_profile] {
        let (s, v) = admin
            .post(
                &format!("/api/v1/admin/libraries/{lib}/access"),
                json!({"profile_id": profile}),
            )
            .await;
        assert_eq!(s, 204, "{v}");
    }

    // ---------------------------------------- 1. artwork cached at the add
    let mut ids = HashMap::new();
    for (name, title) in [("lamp.xml", "Lamp Radio"), ("night.xml", "Night Serial")] {
        let (s, show) = admin
            .post(
                &format!("/api/v1/admin/libraries/{lib}/podcasts"),
                json!({"feed_url": at(name)}),
            )
            .await;
        assert_eq!(s, 201, "{show}");
        ids.insert(title, show["id"].as_str().unwrap().to_string());
    }
    let (lamp, night) = (ids["Lamp Radio"].clone(), ids["Night Serial"].clone());
    // The add answers before the artwork is fetched (in the background).
    let mut shows = Value::Null;
    for _ in 0..100 {
        shows = admin.get("/api/v1/podcasts").await;
        if shows
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["artwork_url"].is_string())
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let lamp_art = show_in(&shows, "Lamp Radio").unwrap()["artwork_url"]
        .as_str()
        .unwrap_or_else(|| panic!("artwork cached after the add: {shows}"))
        .to_string();
    let prefix = format!("/api/v1/podcasts/{lamp}/artwork?v=");
    assert!(lamp_art.starts_with(&prefix), "{lamp_art}");
    let key = &lamp_art[prefix.len()..];
    assert_eq!(key.len(), 16, "a 16-hex key: {lamp_art}");
    assert_eq!(cached_files(&artwork, &lamp), [format!("cover-{key}.png")]);

    let res = admin.raw(Method::GET, &lamp_art, None).await;
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "image/png");
    assert_eq!(res.headers()["x-content-type-options"], "nosniff");
    assert!(res.headers()["cache-control"]
        .to_str()
        .unwrap()
        .starts_with("private"));
    assert_eq!(res.bytes().await.unwrap().as_ref(), png.as_slice());
    let detail = admin.get(&format!("/api/v1/podcasts/{lamp}")).await;
    assert_eq!(detail["artwork_url"], lamp_art.as_str());
    assert_eq!(detail["episodes"][0]["artwork_url"], lamp_art.as_str());
    assert!(
        !detail.to_string().contains("image_url") && !detail.to_string().contains("image_path"),
        "no address or file path in a listener's answer: {detail}"
    );
    let ep = |title: &str| episode_id(&detail, title);
    let (l1, l2, l3, l4) = (
        ep("Lamp one"),
        ep("Lamp two"),
        ep("Lamp three"),
        ep("Lamp four"),
    );
    let one = admin.get(&format!("/api/v1/podcast-episodes/{l2}")).await;
    assert_eq!(one["artwork_url"], lamp_art.as_str());

    // ------------------------------------------------- 2. who sees artwork
    let res = anon.raw(Method::GET, &lamp_art, None).await;
    assert_eq!(res.status(), 401, "signed-in only");
    let (s, _) = guest.req(Method::GET, &lamp_art, None).await;
    assert_eq!(s, 404, "no grant, no artwork");
    let (s, v) = admin
        .put(
            &format!("/api/v1/admin/profiles/{kid_profile}/explicit"),
            json!({"allow_explicit": false}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let night_art = show_in(&shows, "Night Serial").unwrap()["artwork_url"]
        .as_str()
        .unwrap()
        .to_string();
    let (s, _) = admin.req(Method::GET, &night_art, None).await;
    assert_eq!(s, 200);
    let (s, _) = kid.req(Method::GET, &night_art, None).await;
    assert_eq!(s, 404, "D41: an explicit show's artwork is not found");
    let (s, _) = kid.req(Method::GET, &lamp_art, None).await;
    assert_eq!(s, 200);

    // ------------------------------------------- 3. Home's library tile
    let tile = |home: &Value| {
        home["libraries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["id"] == lib.as_str())
            .map(|l| l["item_count"].clone())
    };
    assert_eq!(tile(&admin.get("/api/v1/home").await), Some(json!(2)));
    assert_eq!(
        tile(&kid.get("/api/v1/home").await),
        Some(json!(1)),
        "D41: a hidden show isn't counted"
    );
    assert_eq!(tile(&guest.get("/api/v1/home").await), None, "no grant");

    // ------------------------------------------------------- 4. chapters
    let feed_http = parlour_catalog::podcasts::http_client();
    let (s, v) = admin
        .post(
            &format!("/api/v1/admin/podcast-episodes/{l1}/download"),
            json!({}),
        )
        .await;
    assert_eq!(s, 202, "{v}");
    run_queued(&pool, &feed_http, &artwork).await;
    assert_eq!(
        admin
            .get(&format!("/api/v1/podcast-episodes/{l1}/chapters"))
            .await,
        json!({"source": "file", "error": null, "chapters": [
            {"start_ms": 0, "title": "Opening"},
            {"start_ms": 1500, "title": "The rest"}
        ]}),
        "the download's own chapter marks"
    );
    let feed_chapters = admin
        .get(&format!("/api/v1/podcast-episodes/{l2}/chapters"))
        .await;
    assert_eq!(
        feed_chapters,
        json!({"source": "feed", "error": null, "chapters": [
            {"start_ms": 0, "title": "Hello"},
            {"start_ms": 1250, "title": "Sponsor"}
        ]}),
        "the feed's list, fetched server-side: no image, no link, no toc:false"
    );
    let missing = admin
        .get(&format!("/api/v1/podcast-episodes/{l3}/chapters"))
        .await;
    assert_eq!(missing["source"], "feed");
    assert_eq!(missing["chapters"], json!([]));
    assert_eq!(
        missing["error"],
        "The publisher's chapter list couldn't be read: the chapter list answered HTTP 404."
    );
    assert_eq!(
        admin
            .get(&format!("/api/v1/podcast-episodes/{l4}/chapters"))
            .await,
        json!({"source": null, "error": null, "chapters": []})
    );
    let night_detail = admin.get(&format!("/api/v1/podcasts/{night}")).await;
    let n1 = episode_id(&night_detail, "Night one");
    let (s, _) = kid
        .req(
            Method::GET,
            &format!("/api/v1/podcast-episodes/{n1}/chapters"),
            None,
        )
        .await;
    assert_eq!(s, 404, "D41 covers chapters too");
    let (s, _) = guest
        .req(
            Method::GET,
            &format!("/api/v1/podcast-episodes/{l2}/chapters"),
            None,
        )
        .await;
    assert_eq!(s, 404, "no grant, no chapters");

    // ------------------------------- 5. Now Playing, history and stats
    let (s, started) = admin
        .post(
            "/api/v1/playback/start",
            json!({"episode_id": l2, "position_ms": 0, "duration_ms": 3_000,
                   "client_name": "Flow"}),
        )
        .await;
    assert_eq!(s, 200, "{started}");
    let sid = started["session_id"].as_str().unwrap().to_string();
    let now_playing = admin.get("/api/v1/admin/sessions").await;
    let np = now_playing
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["session_id"] == sid.as_str())
        .expect("on Now Playing");
    assert_eq!(np["item_id"], l2.as_str());
    assert_eq!(np["podcast_id"], lamp.as_str());
    assert_eq!(np["artwork_url"], lamp_art.as_str());
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let (s, _) = admin
        .post(
            "/api/v1/playback/stopped",
            json!({"episode_id": l2, "session_id": sid, "position_ms": 1_000,
                   "listened_ms": 1_000}),
        )
        .await;
    assert_eq!(s, 204);
    // The kid listens to the explicit show while still allowed.
    let (s, v) = admin
        .put(
            &format!("/api/v1/admin/profiles/{kid_profile}/explicit"),
            json!({"allow_explicit": true}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    for (episode, ms) in [(&n1, 1_000), (&l4, 1_000)] {
        let (s, started) = kid
            .post(
                "/api/v1/playback/start",
                json!({"episode_id": episode, "position_ms": 0}),
            )
            .await;
        assert_eq!(s, 200, "{started}");
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let (s, _) = kid
            .post(
                "/api/v1/playback/stopped",
                json!({"episode_id": episode, "session_id": started["session_id"],
                       "position_ms": ms, "listened_ms": ms}),
            )
            .await;
        assert_eq!(s, 204);
    }

    let history = admin.get("/api/v1/listening/sessions").await;
    assert_eq!(history["sessions"][0]["podcast_id"], lamp.as_str());
    assert_eq!(history["sessions"][0]["artwork_url"], lamp_art.as_str());
    let stats = admin.get("/api/v1/listening/stats").await;
    assert_eq!(
        stats["shows"],
        json!([{
            "podcast_id": lamp, "title": "Lamp Radio", "artwork_url": lamp_art,
            "listened_ms": 1_000, "sessions": 1, "episodes": 1,
            "last_listened_at": stats["shows"][0]["last_listened_at"]
        }]),
        "listening per show"
    );
    assert_eq!(stats["books"], json!([]), "the book list stays books");
    let kid_stats = kid.get("/api/v1/listening/stats").await;
    let kid_shows: Vec<&str> = kid_stats["shows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["title"].as_str().unwrap())
        .collect();
    assert_eq!(kid_shows, ["Lamp Radio", "Night Serial"], "{kid_stats}");
    let (s, v) = admin
        .put(
            &format!("/api/v1/admin/profiles/{kid_profile}/explicit"),
            json!({"allow_explicit": false}),
        )
        .await;
    assert_eq!(s, 200, "{v}");
    let kid_stats = kid.get("/api/v1/listening/stats").await;
    let kid_shows: Vec<&str> = kid_stats["shows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        kid_shows,
        ["Lamp Radio"],
        "D41: a hidden show leaves the stats"
    );
    assert_eq!(kid_stats["total_ms"], 1_000);

    // --------------------------------- 6. a new address replaces the copy
    stub.put(
        "lamp.xml",
        "application/rss+xml",
        lamp_items(Some(&at("art.jpg"))),
    );
    let (s, v) = admin
        .post(&format!("/api/v1/admin/podcasts/{lamp}/refresh"), json!({}))
        .await;
    assert_eq!(s, 202, "{v}");
    let job = run_queued(&pool, &feed_http, &artwork).await;
    assert_eq!(
        job.result.as_ref().unwrap()["artwork"],
        json!({"cached": 1, "failed": 0})
    );
    let jpg_art = admin.get(&format!("/api/v1/podcasts/{lamp}")).await["artwork_url"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(jpg_art, lamp_art, "a new key for a new address");
    let jpg_key = &jpg_art[prefix.len()..];
    assert_eq!(
        cached_files(&artwork, &lamp),
        [format!("cover-{jpg_key}.jpg")],
        "the old copy is removed"
    );
    let res = admin.raw(Method::GET, &jpg_art, None).await;
    assert_eq!(res.headers()["content-type"], "image/jpeg");
    assert_eq!(res.bytes().await.unwrap().as_ref(), jpg.as_slice());
    // A check that finds the same address fetches nothing.
    let (s, _) = admin
        .post(&format!("/api/v1/admin/podcasts/{lamp}/refresh"), json!({}))
        .await;
    assert_eq!(s, 202);
    let job = run_queued(&pool, &feed_http, &artwork).await;
    assert_eq!(
        job.result.as_ref().unwrap()["artwork"],
        json!({"cached": 0, "failed": 0})
    );

    // ------------------------------- 7. not an image: refused, copy kept
    stub.put(
        "lamp.xml",
        "application/rss+xml",
        lamp_items(Some(&at("art.svg"))),
    );
    let (s, _) = admin
        .post(&format!("/api/v1/admin/podcasts/{lamp}/refresh"), json!({}))
        .await;
    assert_eq!(s, 202);
    let job = run_queued(&pool, &feed_http, &artwork).await;
    assert_eq!(
        job.result.as_ref().unwrap()["artwork"],
        json!({"cached": 0, "failed": 1})
    );
    assert_eq!(
        job.errors
            .to_string()
            .matches(
                "Lamp Radio: the artwork wasn't cached (it isn't a JPEG, PNG, WebP or GIF image)"
            )
            .count(),
        1,
        "named by show, never by address: {}",
        job.errors
    );
    assert_eq!(
        admin.get(&format!("/api/v1/podcasts/{lamp}")).await["artwork_url"],
        jpg_art.as_str(),
        "the older copy is kept"
    );

    // --------------------------------------- 8. no artwork, no copy
    stub.put("lamp.xml", "application/rss+xml", lamp_items(None));
    let (s, _) = admin
        .post(&format!("/api/v1/admin/podcasts/{lamp}/refresh"), json!({}))
        .await;
    assert_eq!(s, 202);
    run_queued(&pool, &feed_http, &artwork).await;
    assert!(admin.get(&format!("/api/v1/podcasts/{lamp}")).await["artwork_url"].is_null());
    let (s, v) = admin.req(Method::GET, &jpg_art, None).await;
    assert_eq!(s, 404, "{v}");
    assert!(
        cached_files(&artwork, &lamp).is_empty(),
        "the copy is removed"
    );

    // ------------------------------------------------ 9. the token, nowhere
    let activity = admin.get("/api/v1/admin/activity?limit=200").await;
    no_token("Activity", &activity.to_string());
    let (leaks,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM jobs WHERE errors::text LIKE '%' || $1 || '%'
            OR COALESCE(result::text, '') LIKE '%' || $1 || '%'",
    )
    .bind(TOKEN)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(leaks, 0, "no job holds the token");

    let _ = std::fs::remove_dir_all(&root);
}
