//! End-to-end test of POD7 (Apple directory search, D31) against a real
//! Postgres, the real router and a local stub host on the loopback that
//! stands in for both Apple's iTunes Search API and the feed host (never
//! the real Apple API, never the internet):
//!
//! - **the query is checked first**: an empty or too-long query, a bad
//!   store country, an unknown field, a library that isn't a podcasts
//!   library — and a feed address typed as a query, which is refused
//!   **before anything is sent to Apple** (the stub sees no request);
//! - **the search** asks Apple's API for podcasts (`media`/`entity`
//!   `podcast`, a limit, the country, Parlour's User-Agent) and answers
//!   each result with title, author, feed address, Apple's artwork,
//!   genres and episode count; a result without a feed address has
//!   none;
//! - **already added**: a result is marked with its show when the show
//!   is in *this* library, by Apple's id (a show added from a search)
//!   or by the exact feed address (a show added by address); the same
//!   search in another podcasts library marks nothing;
//! - **adding from a result** is POD4's add with `itunes_id`: the feed is
//!   read and the show stored with its first check and Apple's id, its
//!   artwork cached in the background; the same Apple id again is a 409,
//!   a result whose feed isn't a feed is refused and nothing is stored;
//!   another podcasts library may add the same show;
//! - **Apple failing** (HTTP 503, an answer too large, an unreadable
//!   one) is a 502 that names Apple and never repeats the query;
//! - **admin only**: the search is a 404 for a non-admin, 401 signed out;
//! - **the token in a feed URL** never appears in an error, a job or a
//!   notice.
//!
//! Ignored by default; needs PARLOUR_TEST_DATABASE_URL (a fresh
//! database).
//!
//! Run: PARLOUR_TEST_DATABASE_URL=postgres://... \
//!   cargo test -p parlour-server --test podcast_search_flow -- --ignored

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path as UrlPath, Query, State};
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

const EPISODIC: &str = include_str!("../../catalog/tests/fixtures/podcasts/episodic.xml");
const NOT_A_FEED: &str = include_str!("../../catalog/tests/fixtures/podcasts/not-a-feed.html");

/// The private token every stub feed address carries; it must never
/// come back in an error, a job or a notice.
const TOKEN: &str = "fake-feed-token";

/// A PNG's signature and the start of its header: all POD6's artwork
/// cache looks at.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x02\0\0\0";

// ------------------------------------------------------------ stub host

/// (status, content type, body)
type Served = (u16, &'static str, Vec<u8>);
/// One search request: its query parameters and User-Agent.
type Search = (HashMap<String, String>, String);

#[derive(Clone, Default)]
struct Stub {
    /// name → what it answers
    files: Arc<Mutex<HashMap<String, Served>>>,
    /// search term → Apple's answer
    answers: Arc<Mutex<HashMap<String, Served>>>,
    /// every search request: its query parameters and User-Agent
    searches: Arc<Mutex<Vec<Search>>>,
}

impl Stub {
    fn put(&self, name: &str, status: u16, content_type: &'static str, body: impl Into<Vec<u8>>) {
        self.files
            .lock()
            .unwrap()
            .insert(name.into(), (status, content_type, body.into()));
    }
    fn answer(&self, term: &str, status: u16, content_type: &'static str, body: Vec<u8>) {
        self.answers
            .lock()
            .unwrap()
            .insert(term.into(), (status, content_type, body));
    }
    fn searches(&self) -> Vec<Search> {
        self.searches.lock().unwrap().clone()
    }
}

fn served((status, ct, body): Served) -> Response {
    let mut h = HeaderMap::new();
    h.insert("content-type", ct.parse().unwrap());
    (StatusCode::from_u16(status).unwrap(), h, body).into_response()
}

async fn stub_file(State(stub): State<Stub>, UrlPath(name): UrlPath<String>) -> Response {
    match stub.files.lock().unwrap().get(&name).cloned() {
        Some(s) => served(s),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn stub_search(
    State(stub): State<Stub>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let ua = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let term = params.get("term").cloned().unwrap_or_default();
    stub.searches.lock().unwrap().push((params, ua));
    match stub.answers.lock().unwrap().get(&term).cloned() {
        Some(s) => served(s),
        None => served((
            200,
            "text/javascript; charset=utf-8",
            br#"{"resultCount":0,"results":[]}"#.to_vec(),
        )),
    }
}

/// A feed of (guid, title, day in November 2026), with artwork.
fn feed(title: &str, image: &str, items: &[(&str, &str, u32)]) -> String {
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\" \
         xmlns:itunes=\"http://www.itunes.com/dtds/podcast-1.0.dtd\">\n\
         <channel><title>{title}</title><description>A test show</description>\
         <itunes:image href=\"{}\"/>\n",
        image.replace('&', "&amp;")
    );
    for (guid, ep_title, day) in items {
        xml.push_str(&format!(
            "<item><guid>{guid}</guid><title>{ep_title}</title>\
             <pubDate>{day:02} Nov 2026 07:00:00 +0000</pubDate>\
             <enclosure url=\"https://media.example/{guid}.mp3\" type=\"audio/mpeg\" \
             length=\"1000\"/></item>\n"
        ));
    }
    xml.push_str("</channel></rss>\n");
    xml
}

/// One result as Apple's iTunes Search API lists a podcast.
fn apple(id: i64, title: &str, feed_url: Option<&str>, artwork: Option<&str>) -> Value {
    let mut r = json!({
        "wrapperType": "track",
        "kind": "podcast",
        "collectionId": id,
        "trackId": id,
        "artistName": format!("{title} Productions"),
        "collectionName": title,
        "trackName": title,
        "trackCount": 12,
        "primaryGenreName": "History",
        "genres": ["History", "Podcasts"],
        "collectionExplicitness": "notExplicit",
        "releaseDate": "2026-11-20T07:00:00Z",
    });
    if let Some(u) = feed_url {
        r["feedUrl"] = json!(u);
    }
    if let Some(a) = artwork {
        r["artworkUrl600"] = json!(a);
    }
    r
}

fn answer_of(results: Vec<Value>) -> Vec<u8> {
    serde_json::to_vec(&json!({"resultCount": results.len(), "results": results})).unwrap()
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

fn no_token(what: &str, text: &str) {
    assert!(!text.contains(TOKEN), "{what} shows the feed token: {text}");
}

async fn count(pool: &PgPool, sql: &'static str) -> i64 {
    let (n,): (i64,) = sqlx::query_as(sql).fetch_one(pool).await.unwrap();
    n
}

fn message(body: &Value) -> String {
    body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs PARLOUR_TEST_DATABASE_URL"]
async fn podcast_search_flow() {
    let db_url = std::env::var("PARLOUR_TEST_DATABASE_URL").expect("test db url");
    let config = test_config(&db_url);
    let pool = parlour_db::connect(&config.database_url).await.unwrap();
    parlour_db::migrate(&pool).await.unwrap();
    parlour_server::auth::bootstrap_admin(&pool, &config)
        .await
        .unwrap();

    let root: PathBuf =
        std::env::temp_dir().join(format!("parlour-pod7-flow-{}", Uuid::new_v4().simple()));
    let store = root.join("podcasts");
    let store2 = root.join("podcasts-2");
    let books = root.join("books");
    let artwork = root.join("artwork");
    for d in [&store, &store2, &books, &artwork] {
        std::fs::create_dir_all(d).unwrap();
    }

    // ------------------------------------------------------------ stub host
    let stub = Stub::default();
    let stub_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let host = format!("http://{}", stub_listener.local_addr().unwrap());
    let stub_app = Router::new()
        .route("/f/{name}", get(stub_file))
        .route("/search", get(stub_search))
        .with_state(stub.clone());
    tokio::spawn(async move { axum::serve(stub_listener, stub_app).await.unwrap() });
    let at = |name: &str| format!("{host}/f/{name}?token={TOKEN}");

    stub.put(
        "lantern.xml",
        200,
        "application/rss+xml",
        EPISODIC.as_bytes().to_vec(),
    );
    let tones = feed(
        "Tone Radio",
        &at("cover.png"),
        &[("tone-1", "Tone one", 10), ("tone-2", "Tone two", 12)],
    );
    stub.put("tones.xml", 200, "application/rss+xml", tones.clone());
    stub.put("tones-moved.xml", 200, "application/rss+xml", tones);
    stub.put("cover.png", 200, "image/png", PNG.to_vec());
    stub.put("art.png", 200, "image/png", PNG.to_vec());
    stub.put(
        "page.html",
        200,
        "text/html",
        NOT_A_FEED.as_bytes().to_vec(),
    );

    // Apple's answers, by search term.
    let art = format!("{host}/f/art.png");
    stub.answer(
        "lantern",
        200,
        "text/javascript; charset=utf-8",
        answer_of(vec![
            apple(
                501,
                "The Lantern Hour",
                Some(&at("lantern.xml")),
                Some(&art),
            ),
            apple(502, "Tone Radio", Some(&at("tones.xml")), Some(&art)),
            apple(503, "Broken Page", Some(&at("page.html")), None),
            apple(504, "No Feed", None, Some(&art)),
            apple(
                505,
                "Publisher Art",
                Some(&at("missing.xml")),
                Some(&format!(
                    "https://cdn.publisher.example/art.jpg?token={TOKEN}"
                )),
            ),
        ]),
    );
    // Tone Radio's feed has moved since it was added: Apple's id still
    // matches.
    stub.answer(
        "moved",
        200,
        "text/javascript; charset=utf-8",
        answer_of(vec![apple(
            502,
            "Tone Radio",
            Some(&at("tones-moved.xml")),
            None,
        )]),
    );
    stub.answer("busy", 503, "text/html", b"<html>busy</html>".to_vec());
    stub.answer("junk", 200, "text/html", b"<html>not json</html>".to_vec());
    stub.answer(
        "huge",
        200,
        "text/javascript",
        vec![b' '; parlour_catalog::podcast_directory::MAX_ANSWER_BYTES + 1],
    );

    // ------------------------------------------------------------ server
    let mut state = AppState::new(pool.clone(), None, false);
    state.apple_search_base = host.clone();
    state.metadata.artwork_dir = artwork.clone();
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
    let search_path = format!("{shows_path}/search");
    let search = |q: &str| json!({ "query": q });

    // ------------------------------------------- 1. refusals, nothing sent
    for (body, expect) in [
        (search("   "), "Enter a show"),
        (search(&"x".repeat(201)), "at most 200"),
        (search(&at("lantern.xml")), "feed's address"),
        (
            search(&format!("feeds.example/show?token={TOKEN}")),
            "feed's address",
        ),
        (json!({"query": "lantern", "country": "USA"}), "two letters"),
    ] {
        let (s, res) = admin
            .req(Method::POST, &search_path, Some(body.clone()))
            .await;
        assert_eq!(s, 400, "{body}: {res}");
        let msg = message(&res);
        assert!(msg.contains(expect), "{body}: {msg}");
        no_token("a refused search", &res.to_string());
        assert!(!msg.contains("xxxxxxxx"), "the query isn't repeated");
    }
    let (s, _) = admin
        .req(
            Method::POST,
            &search_path,
            Some(json!({"query": "lantern", "term": "x"})),
        )
        .await;
    assert_eq!(s, 422, "an unknown field is refused");
    let (s, res) = admin
        .req(
            Method::POST,
            &format!("/api/v1/admin/libraries/{book_lib}/podcasts/search"),
            Some(search("lantern")),
        )
        .await;
    assert_eq!(s, 400, "{res}");
    assert!(message(&res).contains("podcasts library"));
    let (s, _) = admin
        .req(
            Method::POST,
            &format!("/api/v1/admin/libraries/{}/podcasts/search", Uuid::new_v4()),
            Some(search("lantern")),
        )
        .await;
    assert_eq!(s, 404);
    assert!(
        stub.searches().is_empty(),
        "nothing refused reached Apple: {:?}",
        stub.searches()
    );

    // ------------------------------------------- 2. a show added by address
    let (s, lantern) = admin
        .req(
            Method::POST,
            &shows_path,
            Some(json!({"feed_url": at("lantern.xml")})),
        )
        .await;
    assert_eq!(s, 201, "{lantern}");
    let lantern_id = lantern["id"].as_str().unwrap().to_string();
    assert!(lantern["itunes_id"].is_null(), "added by address: no id");

    // ------------------------------------------- 3. the search
    let (s, found) = admin
        .req(Method::POST, &search_path, Some(search("  lantern ")))
        .await;
    assert_eq!(s, 200, "{found}");
    assert_eq!(found["country"], "US");
    let searches = stub.searches();
    assert_eq!(searches.len(), 1, "one request to Apple");
    let (params, ua) = &searches[0];
    assert_eq!(params["term"], "lantern", "white space folded");
    assert_eq!(params["media"], "podcast");
    assert_eq!(params["entity"], "podcast");
    assert_eq!(params["country"], "US");
    assert_eq!(params["limit"], "25");
    assert!(ua.starts_with("Parlour/"), "{ua}");
    let results = found["results"].as_array().unwrap();
    let titles: Vec<&str> = results
        .iter()
        .map(|r| r["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        [
            "The Lantern Hour",
            "Tone Radio",
            "Broken Page",
            "No Feed",
            "Publisher Art"
        ],
        "Apple's order"
    );
    let by_title = |t: &str| results.iter().find(|r| r["title"] == t).unwrap().clone();
    let lantern_r = by_title("The Lantern Hour");
    assert_eq!(
        lantern_r["added"]["podcast_id"], lantern_id,
        "marked by its feed address"
    );
    assert_eq!(lantern_r["itunes_id"], 501);
    let tone_r = by_title("Tone Radio");
    assert!(tone_r["added"].is_null(), "{tone_r}");
    assert_eq!(tone_r["author"], "Tone Radio Productions");
    assert_eq!(tone_r["feed_url"], at("tones.xml"), "admins see it");
    assert_eq!(tone_r["artwork_url"], art, "Apple's artwork (the stub's)");
    assert_eq!(tone_r["genres"], json!(["History"]));
    assert_eq!(tone_r["episode_count"], 12);
    assert_eq!(tone_r["explicit"], false);
    assert!(by_title("No Feed")["feed_url"].is_null());
    assert!(
        by_title("Publisher Art")["artwork_url"].is_null(),
        "artwork off Apple's CDN is dropped"
    );

    let (s, found_ca) = admin
        .req(
            Method::POST,
            &search_path,
            Some(json!({"query": "nothing here", "country": "ca"})),
        )
        .await;
    assert_eq!(s, 200, "{found_ca}");
    assert_eq!(found_ca["country"], "CA");
    assert_eq!(found_ca["results"], json!([]));
    assert_eq!(stub.searches().last().unwrap().0["country"], "CA");

    // ------------------------------------------- 4. add from a result
    let (s, tone) = admin
        .req(
            Method::POST,
            &shows_path,
            Some(json!({"feed_url": tone_r["feed_url"], "itunes_id": tone_r["itunes_id"]})),
        )
        .await;
    assert_eq!(s, 201, "{tone}");
    let tone_id = tone["id"].as_str().unwrap().to_string();
    assert_eq!(tone["title"], "Tone Radio");
    assert_eq!(tone["itunes_id"], 502);
    assert_eq!(tone["episodes"].as_array().unwrap().len(), 2, "first check");
    assert!(tone["last_checked_at"].is_string());
    assert_eq!(tone["settings"]["auto_download"], false, "ABS's defaults");
    assert_eq!(
        count(&pool, "SELECT count(*) FROM jobs").await,
        0,
        "the add queues no job"
    );
    // POD6: the artwork is cached in the background after the add.
    let mut image_path: Option<String> = None;
    for _ in 0..50 {
        let (p,): (Option<String>,) =
            sqlx::query_as("SELECT image_path FROM podcasts WHERE id = $1::uuid")
                .bind(&tone_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        if p.is_some() {
            image_path = p;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let image_path = image_path.expect("the show's artwork was cached");
    assert!(
        image_path.starts_with(&artwork.to_string_lossy().to_string()),
        "{image_path}"
    );

    // ------------------------------------------- 5. marked, by id or address
    let (_, found) = admin
        .req(Method::POST, &search_path, Some(search("lantern")))
        .await;
    let tone_r = found["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["title"] == "Tone Radio")
        .unwrap()
        .clone();
    assert_eq!(tone_r["added"]["podcast_id"], tone_id);
    assert_eq!(tone_r["added"]["title"], "Tone Radio");
    let (_, moved) = admin
        .req(Method::POST, &search_path, Some(search("moved")))
        .await;
    assert_eq!(
        moved["results"][0]["added"]["podcast_id"], tone_id,
        "Apple's id marks it though the feed address changed"
    );

    // ------------------------------------------- 6. duplicates and refusals
    let (s, res) = admin
        .req(
            Method::POST,
            &shows_path,
            Some(json!({"feed_url": at("tones-moved.xml"), "itunes_id": 502})),
        )
        .await;
    assert_eq!(s, 409, "{res}");
    assert!(message(&res).contains("Tone Radio"), "{res}");
    no_token("a duplicate add", &res.to_string());
    let (s, res) = admin
        .req(
            Method::POST,
            &shows_path,
            Some(json!({"feed_url": at("page.html"), "itunes_id": 503})),
        )
        .await;
    assert_eq!(s, 400, "{res}");
    assert!(message(&res).contains("not an RSS feed"), "{res}");
    no_token("a refused add", &res.to_string());
    for bad in [0, -5] {
        let (s, res) = admin
            .req(
                Method::POST,
                &shows_path,
                Some(json!({"feed_url": at("tones-moved.xml"), "itunes_id": bad})),
            )
            .await;
        assert_eq!(s, 400, "{res}");
        assert!(message(&res).contains("Apple podcast id"), "{res}");
    }
    assert_eq!(
        count(&pool, "SELECT count(*) FROM podcasts").await,
        2,
        "nothing refused was stored"
    );

    // ------------------------------------------- 7. another library
    let lib2_shows = format!("/api/v1/admin/libraries/{lib2}/podcasts");
    let (s, found2) = admin
        .req(
            Method::POST,
            &format!("{lib2_shows}/search"),
            Some(search("lantern")),
        )
        .await;
    assert_eq!(s, 200);
    assert!(
        found2["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["added"].is_null()),
        "already added means in this library: {found2}"
    );
    let (s, tone2) = admin
        .req(
            Method::POST,
            &lib2_shows,
            Some(json!({"feed_url": at("tones.xml"), "itunes_id": 502})),
        )
        .await;
    assert_eq!(s, 201, "another podcasts library may hold it: {tone2}");

    // ------------------------------------------- 8. Apple failing
    let before = stub.searches().len();
    for (term, expect) in [
        ("busy", "answered HTTP 503"),
        ("junk", "can't read"),
        ("huge", "larger than 2 MB"),
    ] {
        let (s, res) = admin
            .req(Method::POST, &search_path, Some(search(term)))
            .await;
        assert_eq!(s, 502, "{term}: {res}");
        let msg = message(&res);
        assert!(msg.contains("Apple's podcast directory"), "{msg}");
        assert!(msg.contains(expect), "{term}: {msg}");
        assert!(!msg.contains(term), "the query isn't repeated: {msg}");
        assert!(!msg.contains(&host), "no address: {msg}");
    }
    assert_eq!(stub.searches().len(), before + 3);

    // ------------------------------------------- 9. admin only (D31)
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
    let before = stub.searches().len();
    let (s, body) = sam
        .req(Method::POST, &search_path, Some(search("lantern")))
        .await;
    assert_eq!(s, 404, "the search is hidden from non-admins: {body}");
    let (s, body) = sam
        .req(
            Method::POST,
            &shows_path,
            Some(json!({"feed_url": at("tones-moved.xml"), "itunes_id": 777})),
        )
        .await;
    assert_eq!(s, 404, "{body}");
    let s = http
        .post(format!("{base}{search_path}"))
        .json(&search("lantern"))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16();
    assert_eq!(s, 401, "signed out");
    assert_eq!(stub.searches().len(), before, "no listener reaches Apple");

    // ------------------------------------------- 10. the token stays put
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
    for (params, _) in stub.searches() {
        no_token("a query sent to Apple", &format!("{params:?}"));
    }

    let _ = std::fs::remove_dir_all(&root);
}
