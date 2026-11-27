//! End-to-end test of POD4 (managing feeds, D31/D32) against a real
//! Postgres, the real router, a local stub feed + enclosure host (never
//! the real internet), real ffprobe and a throwaway folder as the
//! podcast store:
//!
//! - **add by URL**: an address that isn't a URL, isn't a feed (an HTML
//!   page) or doesn't answer is refused with a plain error and nothing
//!   is stored; a feed is stored with its first check done (episodes
//!   listed, nothing downloaded, `folder_path` still NULL); the same
//!   feed again is refused (409); a library that isn't a podcasts
//!   library is refused;
//! - **per-feed settings**: an unreadable or never-firing cron schedule,
//!   a negative limit and an unknown field are refused and change
//!   nothing; a good one is stored (white space folded);
//! - **refresh now** queues POD2's `{"podcast_id"}` job (a second click
//!   returns the same job), and its failures count like any other
//!   check: from the 3rd the Dashboard notice carries the show's id,
//!   which is the show page's address;
//! - **download** queues POD3's `{"episode_id"}` job; the episode is
//!   downloaded into the show's folder; a downloaded episode is refused;
//! - **OPML**: an import adds new feeds, skips repeats and feeds already
//!   in the library, refuses what isn't a feed, and reports by name; the
//!   export lists the shows and imports again into a second library
//!   (round trip);
//! - **admin only**: every POD4 route is a 404 for a non-admin;
//! - **the token in a feed URL** never appears in an error, a job or a
//!   notice.
//!
//! Ignored by default; needs PARLOUR_TEST_DATABASE_URL (a fresh
//! database) and ffmpeg/ffprobe on the PATH.
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test podcast_manage_flow -- --ignored

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
use parlour_server::notices;
use reqwest::Method;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const EPISODIC: &str = include_str!("../../catalog/tests/fixtures/podcasts/episodic.xml");
const SERIAL: &str = include_str!("../../catalog/tests/fixtures/podcasts/serial.xml");
const NOT_A_FEED: &str = include_str!("../../catalog/tests/fixtures/podcasts/not-a-feed.html");

/// The private token every stub feed address carries; it must never
/// come back in an error, a job or a notice.
const TOKEN: &str = "fake-feed-token";

// ------------------------------------------------------------ stub host

/// (status, content type, body)
type Served = (u16, &'static str, Vec<u8>);

#[derive(Clone, Default)]
struct Stub {
    /// name → what it answers
    files: Arc<Mutex<HashMap<String, Served>>>,
}

impl Stub {
    fn put(&self, name: &str, status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) {
        self.files
            .lock()
            .unwrap()
            .insert(name.into(), (status, content_type, body.into()));
    }
    fn feed(&self, name: &str, body: &str) {
        self.put(name, 200, "application/rss+xml", body.as_bytes().to_vec());
    }
}

async fn stub_file(State(stub): State<Stub>, UrlPath(name): UrlPath<String>) -> Response {
    match stub.files.lock().unwrap().get(&name).cloned() {
        Some((status, ct, body)) => {
            let mut h = HeaderMap::new();
            h.insert("content-type", ct.parse().unwrap());
            (StatusCode::from_u16(status).unwrap(), h, body).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// A feed of (guid, title, day in November 2026, enclosure URL).
fn feed(title: &str, items: &[(&str, &str, u32, String)]) -> String {
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\" \
         xmlns:itunes=\"http://www.itunes.com/dtds/podcast-1.0.dtd\">\n\
         <channel><title>{title}</title><description>A test show</description>\n"
    );
    for (guid, ep_title, day, url) in items {
        xml.push_str(&format!(
            "<item><guid>{guid}</guid><title>{ep_title}</title>\
             <pubDate>{day:02} Nov 2026 07:00:00 +0000</pubDate>\
             <enclosure url=\"{}\" type=\"audio/mpeg\" length=\"1000\"/></item>\n",
            url.replace('&', "&amp;")
        ));
    }
    xml.push_str("</channel></rss>\n");
    xml
}

fn tone(dir: &Path, name: &str) -> Vec<u8> {
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
        .args(["-c:a", "libmp3lame", "-b:a", "64k"])
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

struct Client {
    http: reqwest::Client,
    base: String,
    cookie: String,
}

impl Client {
    async fn send(&self, rb: reqwest::RequestBuilder) -> (u16, Value, String) {
        let res = rb
            .header("Cookie", format!("parlour_session={}", self.cookie))
            .send()
            .await
            .expect("request");
        let status = res.status().as_u16();
        let text = res.text().await.unwrap_or_default();
        let value = serde_json::from_str(&text).unwrap_or(Value::Null);
        (status, value, text)
    }

    async fn req(&self, method: Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut rb = self.http.request(method, format!("{}{path}", self.base));
        if let Some(b) = body {
            rb = rb.json(&b);
        }
        let (s, v, _) = self.send(rb).await;
        (s, v)
    }

    async fn get(&self, path: &str) -> Value {
        let (s, v) = self.req(Method::GET, path, None).await;
        assert_eq!(s, 200, "{path}: {v}");
        v
    }

    async fn post_raw(&self, path: &str, body: &str) -> (u16, Value, String) {
        let rb = self
            .http
            .post(format!("{}{path}", self.base))
            .header("content-type", "text/x-opml")
            .body(body.to_string());
        self.send(rb).await
    }

    async fn get_raw(&self, path: &str) -> (u16, HeaderMap, String) {
        let res = self
            .http
            .get(format!("{}{path}", self.base))
            .header("Cookie", format!("parlour_session={}", self.cookie))
            .send()
            .await
            .expect("request");
        let status = res.status().as_u16();
        let headers = res.headers().clone();
        (status, headers, res.text().await.unwrap_or_default())
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

/// Run the one queued `refresh_podcasts` job, as the worker would.
async fn run_queued(pool: &PgPool, http: &reqwest::Client) -> parlour_jobs::Job {
    let job = parlour_jobs::claim_next(pool, parlour_jobs::JobKind::RefreshPodcasts)
        .await
        .unwrap()
        .expect("a queued refresh_podcasts job");
    let id = job.id;
    parlour_server::podcasts::run_refresh_job_claimed(pool, http, job, None).await;
    let job = parlour_jobs::get(pool, id).await.unwrap().unwrap();
    assert_eq!(job.status, "succeeded", "{job:?}");
    job
}

fn no_token(what: &str, text: &str) {
    assert!(!text.contains(TOKEN), "{what} shows the feed token: {text}");
}

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    let (n,): (i64,) = sqlx::query_as(sql).fetch_one(pool).await.unwrap();
    n
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL and ffmpeg"]
async fn podcast_manage_flow() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test db url");
    let config = test_config(&db_url);
    let pool = parlour_db::connect(&config.database_url).await.unwrap();
    parlour_db::migrate(&pool).await.unwrap();
    parlour_server::auth::bootstrap_admin(&pool, &config)
        .await
        .unwrap();

    let root: PathBuf =
        std::env::temp_dir().join(format!("parlour-pod4-flow-{}", Uuid::new_v4().simple()));
    let store = root.join("podcasts");
    let store2 = root.join("podcasts-2");
    let books = root.join("books");
    for d in [&store, &store2, &books] {
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

    let mp3 = tone(&root, "tone.mp3");
    stub.put("one.mp3", 200, "audio/mpeg", mp3.clone());
    stub.put("two.mp3", 200, "audio/mpeg", mp3.clone());
    stub.feed("lantern.xml", EPISODIC);
    stub.feed("ice.xml", SERIAL);
    stub.put(
        "page.html",
        200,
        "text/html",
        NOT_A_FEED.as_bytes().to_vec(),
    );
    let audio_feed = |extra: bool| {
        let mut items = vec![
            ("tone-1", "Tone one", 10, at("one.mp3")),
            ("tone-2", "Tone two", 12, at("two.mp3")),
        ];
        if extra {
            items.push(("tone-3", "Tone three", 14, at("two.mp3")));
        }
        feed("Tone Radio", &items)
    };
    stub.feed("tones.xml", &audio_feed(false));

    // ------------------------------------------------------------ server
    let state = AppState::new(pool.clone(), None, false);
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
    let (cookie, me) = login(&http, &base, "jordan").await;
    let admin = Client {
        http: http.clone(),
        base: base.clone(),
        cookie,
    };
    let profile = me["profiles"][0]["id"].as_str().unwrap().to_string();
    let (s, _) = admin
        .req(
            Method::POST,
            &format!("/api/v1/profiles/{profile}/select"),
            Some(json!({})),
        )
        .await;
    assert_eq!(s, 200);
    let mut libs = Vec::new();
    for (name, kind, path) in [
        ("Podcasts", "podcasts", &store),
        ("Podcasts Two", "podcasts", &store2),
        ("Books", "audiobooks", &books),
    ] {
        let (s, lib) = admin
            .req(
                Method::POST,
                "/api/v1/admin/libraries",
                Some(json!({"name": name, "kind": kind, "paths": [path.to_string_lossy()]})),
            )
            .await;
        assert_eq!(s, 201, "{lib}");
        libs.push(lib["id"].as_str().unwrap().to_string());
    }
    let (lib, lib2, book_lib) = (libs[0].clone(), libs[1].clone(), libs[2].clone());
    let shows_path = format!("/api/v1/admin/libraries/{lib}/podcasts");
    let feed_http = parlour_catalog::podcasts::http_client();

    // ------------------------------------------- 1. add by URL: refusals
    for (url, expect) in [
        ("   ".to_string(), "Enter the feed"),
        (
            format!("feeds.example/x?token={TOKEN}"),
            "isn't a web address",
        ),
        (
            format!("ftp://feeds.example/x?token={TOKEN}"),
            "http:// or https://",
        ),
        (at("page.html"), "not an RSS feed"),
        (at("missing.xml"), "HTTP 404"),
        (
            format!("http://127.0.0.1:1/f/x.xml?token={TOKEN}"),
            "could not fetch the feed",
        ),
    ] {
        let (s, body) = admin
            .req(Method::POST, &shows_path, Some(json!({"feed_url": url})))
            .await;
        assert_eq!(s, 400, "{url}: {body}");
        let msg = body["error"]["message"].as_str().unwrap();
        assert!(msg.contains(expect), "{url}: {msg}");
        no_token("an add's error", &body.to_string());
    }
    assert_eq!(
        count(&pool, "SELECT count(*) FROM podcasts").await,
        0,
        "nothing stored"
    );
    let (s, body) = admin
        .req(
            Method::POST,
            &format!("/api/v1/admin/libraries/{book_lib}/podcasts"),
            Some(json!({"feed_url": at("lantern.xml")})),
        )
        .await;
    assert_eq!(s, 400, "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("podcasts library"));
    let (s, _) = admin
        .req(
            Method::POST,
            &shows_path,
            Some(json!({"feed": at("lantern.xml")})),
        )
        .await;
    assert_eq!(s, 422, "an unknown field is refused");

    // ------------------------------------------- 2. add by URL: success
    let (s, lantern) = admin
        .req(
            Method::POST,
            &shows_path,
            Some(json!({"feed_url": format!("  {}  ", at("lantern.xml"))})),
        )
        .await;
    assert_eq!(s, 201, "{lantern}");
    assert_eq!(lantern["title"], "The Lantern Hour");
    assert_eq!(lantern["feed_url"], at("lantern.xml"), "stored trimmed");
    assert!(
        lantern["last_checked_at"].is_string(),
        "the first check ran"
    );
    assert_eq!(lantern["consecutive_failures"], 0);
    assert!(
        lantern["folder_path"].is_null(),
        "named at first download (POD3)"
    );
    assert_eq!(
        lantern["settings"],
        json!({"auto_download": false, "check_schedule": "0 * * * *",
               "max_episodes_to_keep": 0, "max_new_episodes_per_check": 3})
    );
    assert_eq!(lantern["settings"], lantern["default"]);
    let eps = lantern["episodes"].as_array().unwrap();
    assert!(!eps.is_empty(), "the show isn't left empty");
    assert!(
        eps.iter().all(|e| e["downloaded_at"].is_null()),
        "nothing downloaded"
    );
    let lantern_id = lantern["id"].as_str().unwrap().to_string();

    let (s, body) = admin
        .req(
            Method::POST,
            &shows_path,
            Some(json!({"feed_url": at("lantern.xml")})),
        )
        .await;
    assert_eq!(s, 409, "{body}");
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("The Lantern Hour"));
    no_token("a duplicate's error", &body.to_string());
    assert_eq!(count(&pool, "SELECT count(*) FROM podcasts").await, 1);

    let (s, tones) = admin
        .req(
            Method::POST,
            &shows_path,
            Some(json!({"feed_url": at("tones.xml")})),
        )
        .await;
    assert_eq!(s, 201, "{tones}");
    let tones_id = tones["id"].as_str().unwrap().to_string();
    assert_eq!(tones["episodes"].as_array().unwrap().len(), 2);
    assert_eq!(
        tones["episodes"][0]["title"], "Tone two",
        "an episodic show lists newest first"
    );

    let list = admin.get(&shows_path).await;
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0]["title"], "The Lantern Hour");
    assert_eq!(list[1]["title"], "Tone Radio");
    assert_eq!(list[1]["episodes"], 2);
    assert_eq!(list[1]["downloaded"], 0);

    // ------------------------------------------- 3. per-feed settings
    let settings_path = format!("/api/v1/admin/podcasts/{tones_id}/settings");
    let good = json!({"auto_download": true, "check_schedule": " 15  */2 * * * ",
                      "max_episodes_to_keep": 5, "max_new_episodes_per_check": 0});
    for (bad, status) in [
        (json!({"check_schedule": "every hour"}), 400),
        (json!({"check_schedule": "0 * * *"}), 400),
        (json!({"check_schedule": "0 0 * * * *"}), 400),
        (json!({"check_schedule": "75 * * * *"}), 400),
        (json!({"check_schedule": "0 0 31 2 *"}), 400),
        (json!({"max_episodes_to_keep": -1}), 400),
        (json!({"max_new_episodes_per_check": -3}), 400),
        (json!({"title": "Renamed"}), 422),
    ] {
        let mut body = good.clone();
        for (k, v) in bad.as_object().unwrap() {
            body[k] = v.clone();
        }
        let (s, err) = admin
            .req(Method::PUT, &settings_path, Some(body.clone()))
            .await;
        assert_eq!(s, status, "{body}: {err}");
    }
    let (sched,): (String,) =
        sqlx::query_as("SELECT check_schedule FROM podcasts WHERE id = $1::uuid")
            .bind(&tones_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(sched, "0 * * * *", "a refused edit changes nothing");
    let (s, err) = admin
        .req(
            Method::PUT,
            &settings_path,
            Some(
                json!({"auto_download": false, "check_schedule": "every hour",
                        "max_episodes_to_keep": 0, "max_new_episodes_per_check": 3}),
            ),
        )
        .await;
    assert_eq!(s, 400);
    assert!(
        err["error"]["message"]
            .as_str()
            .unwrap()
            .contains("can't be read"),
        "{err}"
    );
    let (s, saved) = admin.req(Method::PUT, &settings_path, Some(good)).await;
    assert_eq!(s, 200, "{saved}");
    assert_eq!(
        saved["settings"],
        json!({"auto_download": true, "check_schedule": "15 */2 * * *",
               "max_episodes_to_keep": 5, "max_new_episodes_per_check": 0})
    );
    // Back to auto-download off, so the checks below download nothing on
    // their own.
    let (s, _) = admin
        .req(
            Method::PUT,
            &settings_path,
            Some(
                json!({"auto_download": false, "check_schedule": "15 */2 * * *",
                        "max_episodes_to_keep": 5, "max_new_episodes_per_check": 0}),
            ),
        )
        .await;
    assert_eq!(s, 200);

    // ------------------------------------------- 4. refresh now
    stub.feed("tones.xml", &audio_feed(true));
    let refresh_path = format!("/api/v1/admin/podcasts/{tones_id}/refresh");
    let (s, job) = admin.req(Method::POST, &refresh_path, None).await;
    assert_eq!(s, 202, "{job}");
    assert_eq!(job["kind"], "refresh_podcasts");
    assert_eq!(
        job["payload"],
        json!({"podcast_id": tones_id}),
        "POD2's hook"
    );
    let (s, again) = admin.req(Method::POST, &refresh_path, None).await;
    assert_eq!(s, 202);
    assert_eq!(
        again["id"], job["id"],
        "a second click returns the waiting job"
    );
    assert_eq!(
        admin
            .get(&format!("/api/v1/admin/podcasts/{tones_id}"))
            .await["refresh_queued"],
        true
    );
    let done = run_queued(&pool, &feed_http).await;
    assert_eq!(done.result.as_ref().unwrap()["new_episodes"], 1);
    let show = admin
        .get(&format!("/api/v1/admin/podcasts/{tones_id}"))
        .await;
    assert_eq!(show["episodes"].as_array().unwrap().len(), 3);
    assert_eq!(show["refresh_queued"], false);
    assert!(show["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["downloaded_at"].is_null()));

    // Failures count like any other check; the notice names the show.
    stub.put("tones.xml", 503, "text/plain", b"down".to_vec());
    for n in 1..=3 {
        let (s, _) = admin.req(Method::POST, &refresh_path, None).await;
        assert_eq!(s, 202);
        let job = run_queued(&pool, &feed_http).await;
        no_token(
            "a failed check's job",
            &serde_json::to_string(&job).unwrap(),
        );
        let show = admin
            .get(&format!("/api/v1/admin/podcasts/{tones_id}"))
            .await;
        assert_eq!(show["consecutive_failures"], n);
        assert_eq!(show["last_error"], "the feed answered HTTP 503");
    }
    notices::reconcile_with(&pool, &[]).await.unwrap();
    let list = admin.get("/api/v1/admin/notices").await;
    let notice = list
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["kind"] == "podcast_feed_failing")
        .expect("the failing-feed notice")
        .clone();
    no_token("the notice", &notice.to_string());
    assert_eq!(
        notice["detail"]["podcast_id"],
        tones_id.as_str(),
        "the link's target"
    );
    let linked = notice["detail"]["podcast_id"].as_str().unwrap();
    assert_eq!(
        admin.get(&format!("/api/v1/admin/podcasts/{linked}")).await["title"],
        "Tone Radio",
        "the notice's show id opens the show page's data"
    );
    stub.feed("tones.xml", &audio_feed(true));
    let (s, _) = admin.req(Method::POST, &refresh_path, None).await;
    assert_eq!(s, 202);
    run_queued(&pool, &feed_http).await;
    let show = admin
        .get(&format!("/api/v1/admin/podcasts/{tones_id}"))
        .await;
    assert_eq!(show["consecutive_failures"], 0);
    assert!(show["last_error"].is_null());

    // ------------------------------------------- 5. download by hand
    let one = show["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["title"] == "Tone one")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let download_path = format!("/api/v1/admin/podcast-episodes/{one}/download");
    let (s, job) = admin.req(Method::POST, &download_path, None).await;
    assert_eq!(s, 202, "{job}");
    assert_eq!(job["payload"], json!({"episode_id": one}), "POD3's hook");
    let (s, again) = admin.req(Method::POST, &download_path, None).await;
    assert_eq!(s, 202);
    assert_eq!(again["id"], job["id"]);
    let show = admin
        .get(&format!("/api/v1/admin/podcasts/{tones_id}"))
        .await;
    let ep = show["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == one.as_str())
        .unwrap()
        .clone();
    assert_eq!(ep["download_queued"], true);
    let done = run_queued(&pool, &feed_http).await;
    assert_eq!(done.result.as_ref().unwrap()["downloaded"], 1, "{done:?}");
    let show = admin
        .get(&format!("/api/v1/admin/podcasts/{tones_id}"))
        .await;
    let folder = show["folder_path"]
        .as_str()
        .expect("the folder is named now");
    assert!(Path::new(folder).starts_with(&store), "{folder}");
    let ep = show["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == one.as_str())
        .unwrap()
        .clone();
    assert!(ep["downloaded_at"].is_string());
    assert!(ep["file_duration_ms"].as_i64().unwrap() > 1000);
    assert_eq!(ep["download_queued"], false);
    let others = show["episodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["downloaded_at"].is_string())
        .count();
    assert_eq!(others, 1, "only the one asked for");
    let (s, body) = admin.req(Method::POST, &download_path, None).await;
    assert_eq!(s, 409, "{body}");
    assert_eq!(
        count(&pool, "SELECT count(*) FROM jobs WHERE status = 'queued'").await,
        0,
        "nothing queued for a downloaded episode"
    );
    let (s, _) = admin
        .req(
            Method::POST,
            &format!("/api/v1/admin/podcast-episodes/{}/download", Uuid::new_v4()),
            None,
        )
        .await;
    assert_eq!(s, 404);

    // ------------------------------------------- 6. OPML import
    let opml = format!(
        r#"<?xml version="1.0"?>
<opml version="1.0"><head><title>From another app</title></head><body>
  <outline text="Subscriptions">
    <outline type="rss" text="Lantern (again)" xmlUrl="{lantern}"/>
    <outline type="rss" text="Ice" xmlUrl="{ice}"/>
    <outline type="rss" text="Ice twice" xmlUrl="{ice}"/>
    <outline type="rss" text="A web page" xmlUrl="{page}"/>
    <outline type="rss" xmlUrl="{gone}"/>
    <outline text="Just a folder"/>
  </outline>
</body></opml>"#,
        lantern = at("lantern.xml").replace('&', "&amp;"),
        ice = at("ice.xml").replace('&', "&amp;"),
        page = at("page.html").replace('&', "&amp;"),
        gone = at("gone.xml").replace('&', "&amp;"),
    );
    let opml_path = format!("/api/v1/admin/libraries/{lib}/podcasts/opml");
    let (s, report, raw) = admin.post_raw(&opml_path, &opml).await;
    assert_eq!(s, 200, "{report}");
    no_token("the import report", &raw);
    assert!(!raw.contains(&feeds), "no address in the report: {raw}");
    assert_eq!(report["feeds"], 5);
    assert_eq!(report["added"], 1);
    assert_eq!(report["skipped"], 2);
    assert_eq!(report["refused"], 2);
    let outcomes: Vec<(String, String)> = report["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| {
            (
                l["name"].as_str().unwrap().to_string(),
                l["outcome"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        outcomes,
        vec![
            ("Lantern (again)".into(), "already_in_library".into()),
            ("Ice".into(), "added".into()),
            ("Ice twice".into(), "repeated_in_file".into()),
            ("A web page".into(), "refused".into()),
            ("Feed 5 in the file".into(), "refused".into()),
        ]
    );
    assert!(report["lines"][3]["error"]
        .as_str()
        .unwrap()
        .contains("not an RSS feed"));
    assert!(report["lines"][4]["error"]
        .as_str()
        .unwrap()
        .contains("HTTP 404"));
    assert_eq!(
        count(&pool, "SELECT count(*) FROM podcasts").await,
        3,
        "Ice added once"
    );
    let ice_id = report["lines"][1]["podcast_id"]
        .as_str()
        .unwrap()
        .to_string();
    let ice = admin.get(&format!("/api/v1/admin/podcasts/{ice_id}")).await;
    assert_eq!(ice["title"], "Under the Ice Shelf");
    assert_eq!(ice["podcast_type"], "serial");
    let ice_eps = ice["episodes"].as_array().unwrap();
    assert!(ice_eps.len() >= 2);
    assert!(
        ice_eps[0]["published_at"].as_str() < ice_eps[1]["published_at"].as_str(),
        "a serial show lists oldest first"
    );

    for bad in [
        "<rss version=\"2.0\"><channel/></rss>",
        "not xml at all <",
        "",
    ] {
        let (s, body, _) = admin.post_raw(&opml_path, bad).await;
        assert_eq!(s, 400, "{bad}: {body}");
    }
    let (s, _, _) = admin
        .post_raw(
            &format!("/api/v1/admin/libraries/{book_lib}/podcasts/opml"),
            &opml,
        )
        .await;
    assert_eq!(s, 400, "not a podcasts library");

    // ------------------------------------------- 7. OPML export + round trip
    let (s, headers, doc) = admin.get_raw(&opml_path).await;
    assert_eq!(s, 200);
    assert!(headers["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/x-opml"));
    assert_eq!(
        headers["content-disposition"].to_str().unwrap(),
        "attachment; filename=\"Podcasts.opml\""
    );
    let listed = parlour_catalog::opml::parse_opml(doc.as_bytes()).unwrap();
    let mut urls: Vec<&str> = listed.iter().map(|f| f.feed_url.as_str()).collect();
    urls.sort();
    let mut expected = [at("ice.xml"), at("lantern.xml"), at("tones.xml")];
    expected.sort();
    assert_eq!(
        urls,
        expected.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(
        listed[0].title.as_deref(),
        Some("The Lantern Hour"),
        "sorted by title"
    );
    let (s, report, _) = admin
        .post_raw(
            &format!("/api/v1/admin/libraries/{lib2}/podcasts/opml"),
            &doc,
        )
        .await;
    assert_eq!(s, 200, "{report}");
    assert_eq!(report["added"], 3, "{report}");
    let (s, report, _) = admin
        .post_raw(
            &format!("/api/v1/admin/libraries/{lib2}/podcasts/opml"),
            &doc,
        )
        .await;
    assert_eq!(s, 200);
    assert_eq!(report["added"], 0);
    assert_eq!(report["skipped"], 3, "a second import duplicates nothing");
    assert_eq!(count(&pool, "SELECT count(*) FROM podcasts").await, 6);

    // ------------------------------------------- 8. admin only (D31)
    let (s, invite) = admin.req(Method::POST, "/api/v1/admin/invites", None).await;
    assert_eq!(s, 201, "{invite}");
    let res = http
        .post(format!("{base}/api/v1/invites/accept"))
        .json(&json!({"token": invite["token"], "username": "sam",
                      "password": "admin-pass-123"}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    let (sam_cookie, _) = login(&http, &base, "sam").await;
    let sam = Client {
        http: http.clone(),
        base: base.clone(),
        cookie: sam_cookie,
    };
    for (method, path, body) in [
        (Method::GET, shows_path.clone(), None),
        (
            Method::POST,
            shows_path.clone(),
            Some(json!({"feed_url": at("ice.xml")})),
        ),
        (Method::GET, opml_path.clone(), None),
        (Method::POST, opml_path.clone(), Some(json!({}))),
        (
            Method::GET,
            format!("/api/v1/admin/podcasts/{lantern_id}"),
            None,
        ),
        (
            Method::PUT,
            format!("/api/v1/admin/podcasts/{lantern_id}/settings"),
            Some(json!({"auto_download": true, "check_schedule": "0 * * * *",
                        "max_episodes_to_keep": 0, "max_new_episodes_per_check": 3})),
        ),
        (
            Method::POST,
            format!("/api/v1/admin/podcasts/{lantern_id}/refresh"),
            None,
        ),
        (
            Method::POST,
            format!("/api/v1/admin/podcast-episodes/{one}/download"),
            None,
        ),
    ] {
        let (s, body) = sam.req(method.clone(), &path, body).await;
        assert_eq!(s, 404, "{method} {path} is hidden from non-admins: {body}");
    }
    let (auto,): (bool,) = sqlx::query_as("SELECT auto_download FROM podcasts WHERE id = $1::uuid")
        .bind(&lantern_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!auto, "a non-admin's edit changed nothing");
    assert_eq!(
        count(&pool, "SELECT count(*) FROM jobs WHERE status = 'queued'").await,
        0
    );
    let (s, _) = http
        .get(format!("{base}{shows_path}"))
        .send()
        .await
        .map(|r| (r.status().as_u16(), ()))
        .unwrap();
    assert_eq!(s, 401, "signed out");

    // ------------------------------------------- 9. the token stays put
    let jobs: Vec<(Value, Value, Option<Value>)> =
        sqlx::query_as("SELECT payload, errors, result FROM jobs")
            .fetch_all(&pool)
            .await
            .unwrap();
    for j in &jobs {
        no_token("a job", &format!("{j:?}"));
    }
    let notices_text: Vec<(String, Value)> = sqlx::query_as("SELECT summary, detail FROM notices")
        .fetch_all(&pool)
        .await
        .unwrap();
    no_token("a notice", &format!("{notices_text:?}"));

    let _ = std::fs::remove_dir_all(&root);
}
