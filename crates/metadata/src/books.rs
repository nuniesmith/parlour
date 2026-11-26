//! Book metadata providers for audiobooks (Phase 10, AB6, D30).
//!
//! Matching a book is **manual** (D30, as Audiobookshelf does it): an
//! admin searches a provider for one book and applies a result, or
//! runs a library-wide quick match. Two providers, in D30's order:
//!
//! - **Google Books** (`google`, the default) — the public volumes
//!   search. An API key is optional: Parlour sends one only when
//!   `PARLOUR_GOOGLE_BOOKS_API_KEY` is set (in the `X-Goog-Api-Key`
//!   header, never in a URL), and works without it on Google's
//!   anonymous quota.
//! - **Open Library** (`openlibrary`) — `search.json` and the covers
//!   CDN. No key. Requests carry an identifying User-Agent, as Open
//!   Library asks.
//!
//! Audnexus is deliberately absent: its public instance publishes no
//! terms permitting third-party use (checked 2026-11-25, see
//! `docs/AUDIOBOOKS-PODCASTS.md`).
//!
//! Both are normalised into [`BookCandidate`]. The JSON mapping is
//! pure (fixture-tested); the clients only fetch.

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::ProviderError;

pub const GOOGLE_BOOKS_BASE: &str = "https://www.googleapis.com/books/v1";
pub const OPEN_LIBRARY_BASE: &str = "https://openlibrary.org";
pub const OPEN_LIBRARY_COVERS_BASE: &str = "https://covers.openlibrary.org";
/// The header Google's APIs take an API key in. Parlour sends the key
/// there rather than as `?key=`, so it never appears in a URL.
pub const GOOGLE_KEY_HEADER: &str = "x-goog-api-key";

/// How many results a search asks for.
pub const SEARCH_LIMIT: usize = 10;

/// What Parlour calls itself to the providers. Open Library asks
/// applications to identify themselves; no contact address is sent.
pub fn user_agent() -> String {
    format!(
        "Parlour/{} (+https://github.com/nuniesmith/parlour)",
        env!("CARGO_PKG_VERSION")
    )
}

/// An HTTP client for the book providers and their cover images:
/// Parlour's User-Agent and a 20 s timeout.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(user_agent())
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// One book a provider knows, normalised. Every field but the id and
/// the title is optional; an empty list means the provider has none.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct BookCandidate {
    /// `google` or `openlibrary`.
    pub provider: String,
    /// The provider's own id: a Google volume id, or an Open Library
    /// work id (`OL…W`).
    pub provider_id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub authors: Vec<String>,
    /// Neither provider lists narrators today; kept so a provider
    /// that does can fill it.
    pub narrators: Vec<String>,
    pub publisher: Option<String>,
    pub year: Option<i32>,
    pub description: Option<String>,
    pub genres: Vec<String>,
    pub isbn: Option<String>,
    pub asin: Option<String>,
    pub language: Option<String>,
    pub cover_url: Option<String>,
}

/// A book provider by name.
pub const BOOK_PROVIDERS: [&str; 2] = ["google", "openlibrary"];

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn strings(v: &Value, key: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in v
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        let s = s.trim();
        if !s.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(s)) {
            out.push(s.to_string());
        }
    }
    out
}

/// The year at the start of a date such as `2005-11-15` or `1890`.
pub fn year_of(date: &str) -> Option<i32> {
    let y: i32 = date.trim().get(0..4)?.parse().ok()?;
    (1000..=2999).contains(&y).then_some(y)
}

/// Plain text from the light HTML providers put in descriptions:
/// `<br>` and `</p>` become line breaks, other tags go, the common
/// entities are decoded, and runs of blank lines collapse.
pub fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('>') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let tag = rest[start + 1..start + end]
            .trim()
            .trim_start_matches('/')
            .to_ascii_lowercase();
        let name: String = tag
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if matches!(name.as_str(), "br" | "p" | "div" | "li") {
            out.push('\n');
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    let mut lines: Vec<&str> = Vec::new();
    for line in decoded.lines().map(str::trim) {
        if line.is_empty() && lines.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        lines.push(line);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

fn ids_ok(id: &str, extra: &[char]) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || extra.contains(&c))
}

/// Whether a string is a plausible Google volume id.
pub fn valid_google_id(id: &str) -> bool {
    ids_ok(id, &['-', '_'])
}

/// Whether a string is an Open Library work id (`OL123W`).
pub fn valid_open_library_id(id: &str) -> bool {
    id.len() > 3
        && id.starts_with("OL")
        && id.ends_with('W')
        && id[2..id.len() - 1].chars().all(|c| c.is_ascii_digit())
}

// ------------------------------------------------------------ Google

/// Google's image links upgraded where it's safe: `https` for Google's
/// own hosts, and no page-curl effect.
fn google_image(url: &str) -> String {
    let mut u = url.replace("&edge=curl", "");
    for host in ["http://books.google.", "http://books.googleusercontent."] {
        if u.starts_with(host) {
            u = u.replacen("http://", "https://", 1);
        }
    }
    u
}

/// One Google `volume` resource. `None` without an id or a title.
pub fn parse_google_volume(v: &Value) -> Option<BookCandidate> {
    let id = text(v, "id")?;
    let info = v.get("volumeInfo")?;
    let title = text(info, "title")?;
    let idents: Vec<(String, String)> = info
        .get("industryIdentifiers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|i| Some((text(i, "type")?, text(i, "identifier")?)))
        .collect();
    let isbn = ["ISBN_13", "ISBN_10"]
        .iter()
        .find_map(|t| idents.iter().find(|(k, _)| k == t).map(|(_, v)| v.clone()));
    let cover_url = info.get("imageLinks").and_then(|links| {
        [
            "extraLarge",
            "large",
            "medium",
            "small",
            "thumbnail",
            "smallThumbnail",
        ]
        .iter()
        .find_map(|k| text(links, k))
        .map(|u| google_image(&u))
    });
    Some(BookCandidate {
        provider: "google".to_string(),
        provider_id: id,
        title,
        subtitle: text(info, "subtitle"),
        authors: strings(info, "authors"),
        narrators: Vec::new(),
        publisher: text(info, "publisher"),
        year: text(info, "publishedDate").as_deref().and_then(year_of),
        description: text(info, "description")
            .map(|d| strip_html(&d))
            .filter(|d| !d.is_empty()),
        genres: strings(info, "categories"),
        isbn,
        asin: None,
        language: text(info, "language"),
        cover_url,
    })
}

/// A Google `volumes` search response: its `items`, in order.
pub fn parse_google_search(v: &Value) -> Vec<BookCandidate> {
    v.get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(parse_google_volume)
        .collect()
}

/// The `q` Google is asked: the title (and the author when known)
/// with Google's field keywords.
pub fn google_query(title: &str, author: Option<&str>) -> String {
    let mut q = format!("intitle:{}", title.trim());
    if let Some(a) = author.map(str::trim).filter(|a| !a.is_empty()) {
        q.push_str(&format!(" inauthor:{a}"));
    }
    q
}

#[derive(Clone)]
pub struct GoogleBooksClient {
    http: reqwest::Client,
    key: Option<String>,
    base: String,
}

impl GoogleBooksClient {
    /// `key` is optional (D30: matching works without one).
    pub fn new(key: Option<String>, base: Option<&str>) -> Self {
        Self {
            http: http_client(),
            key: key.filter(|k| !k.trim().is_empty()),
            base: base
                .unwrap_or(GOOGLE_BOOKS_BASE)
                .trim_end_matches('/')
                .to_string(),
        }
    }

    /// One GET. The key, when set, travels in Google's
    /// `X-Goog-Api-Key` header, never in the URL, and errors are
    /// stripped of their URL: a failed request's message ends up on
    /// jobs, in API errors and in logs, and must never carry the key.
    async fn get(&self, path: &str, query: Vec<(&str, String)>) -> Result<Value, ProviderError> {
        let mut req = self.http.get(format!("{}{path}", self.base)).query(&query);
        if let Some(k) = &self.key {
            req = req.header(GOOGLE_KEY_HEADER, k);
        }
        let res = req
            .send()
            .await
            .map_err(|e| ProviderError::Http(e.without_url()))?;
        if !res.status().is_success() {
            return Err(ProviderError::Status(res.status().as_u16()));
        }
        res.json()
            .await
            .map_err(|e| ProviderError::Parse(e.without_url().to_string()))
    }

    pub async fn search(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> Result<Vec<BookCandidate>, ProviderError> {
        let v = self
            .get(
                "/volumes",
                vec![
                    ("q", google_query(title, author)),
                    ("maxResults", SEARCH_LIMIT.to_string()),
                    ("printType", "books".to_string()),
                ],
            )
            .await?;
        Ok(parse_google_search(&v))
    }

    pub async fn get_book(&self, id: &str) -> Result<BookCandidate, ProviderError> {
        if !valid_google_id(id) {
            return Err(ProviderError::Other(format!(
                "not a Google Books id: {id:?}"
            )));
        }
        let v = self.get(&format!("/volumes/{id}"), Vec::new()).await?;
        parse_google_volume(&v)
            .ok_or_else(|| ProviderError::Parse("the volume has no id or title".into()))
    }
}

// ------------------------------------------------------- Open Library

/// The search fields Parlour asks Open Library for.
pub const OPEN_LIBRARY_FIELDS: &str =
    "key,title,subtitle,author_name,first_publish_year,publisher,isbn,cover_i,subject,language";

/// How many of a work's subjects become genres. Open Library lists
/// dozens per work, most of them catalogue headings.
pub const OPEN_LIBRARY_GENRES: usize = 5;

/// One Open Library search `doc`. `None` without a work key or title.
pub fn parse_open_library_doc(v: &Value, covers_base: &str) -> Option<BookCandidate> {
    let key = text(v, "key")?;
    let id = key.strip_prefix("/works/").unwrap_or(&key).to_string();
    if !valid_open_library_id(&id) {
        return None;
    }
    let title = text(v, "title")?;
    let isbns = strings(v, "isbn");
    let isbn = isbns
        .iter()
        .find(|i| i.len() == 13)
        .or_else(|| isbns.first())
        .cloned();
    let mut genres = strings(v, "subject");
    genres.truncate(OPEN_LIBRARY_GENRES);
    Some(BookCandidate {
        provider: "openlibrary".to_string(),
        provider_id: id,
        title,
        subtitle: text(v, "subtitle"),
        authors: strings(v, "author_name"),
        narrators: Vec::new(),
        publisher: strings(v, "publisher").into_iter().next(),
        year: v
            .get("first_publish_year")
            .and_then(Value::as_i64)
            .and_then(|y| i32::try_from(y).ok())
            .filter(|y| (1000..=2999).contains(y)),
        description: None,
        genres,
        isbn,
        asin: None,
        language: strings(v, "language").into_iter().next(),
        cover_url: v
            .get("cover_i")
            .and_then(Value::as_i64)
            .filter(|c| *c > 0)
            .map(|c| {
                format!(
                    "{}/b/id/{c}-L.jpg?default=false",
                    covers_base.trim_end_matches('/')
                )
            }),
    })
}

/// An Open Library `search.json` response: its `docs`, in order.
pub fn parse_open_library_search(v: &Value, covers_base: &str) -> Vec<BookCandidate> {
    v.get("docs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|d| parse_open_library_doc(d, covers_base))
        .collect()
}

#[derive(Clone)]
pub struct OpenLibraryClient {
    http: reqwest::Client,
    base: String,
    covers_base: String,
}

impl OpenLibraryClient {
    pub fn new(base: Option<&str>, covers_base: Option<&str>) -> Self {
        Self {
            http: http_client(),
            base: base
                .unwrap_or(OPEN_LIBRARY_BASE)
                .trim_end_matches('/')
                .to_string(),
            covers_base: covers_base
                .unwrap_or(OPEN_LIBRARY_COVERS_BASE)
                .trim_end_matches('/')
                .to_string(),
        }
    }

    async fn search_json(&self, query: Vec<(&str, String)>) -> Result<Value, ProviderError> {
        let res = self
            .http
            .get(format!("{}/search.json", self.base))
            .query(&query)
            .send()
            .await?;
        if !res.status().is_success() {
            return Err(ProviderError::Status(res.status().as_u16()));
        }
        res.json()
            .await
            .map_err(|e| ProviderError::Parse(e.to_string()))
    }

    pub async fn search(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> Result<Vec<BookCandidate>, ProviderError> {
        let mut query = vec![
            ("title", title.trim().to_string()),
            ("fields", OPEN_LIBRARY_FIELDS.to_string()),
            ("limit", SEARCH_LIMIT.to_string()),
        ];
        if let Some(a) = author.map(str::trim).filter(|a| !a.is_empty()) {
            query.push(("author", a.to_string()));
        }
        let v = self.search_json(query).await?;
        Ok(parse_open_library_search(&v, &self.covers_base))
    }

    pub async fn get_book(&self, id: &str) -> Result<BookCandidate, ProviderError> {
        if !valid_open_library_id(id) {
            return Err(ProviderError::Other(format!(
                "not an Open Library work id: {id:?}"
            )));
        }
        let v = self
            .search_json(vec![
                ("q", format!("key:/works/{id}")),
                ("fields", OPEN_LIBRARY_FIELDS.to_string()),
                ("limit", "1".to_string()),
            ])
            .await?;
        parse_open_library_search(&v, &self.covers_base)
            .into_iter()
            .find(|c| c.provider_id == id)
            .ok_or_else(|| ProviderError::Other(format!("Open Library has no work {id}")))
    }
}

// ------------------------------------------------------------ either

/// A configured book provider.
#[derive(Clone)]
pub enum BookProvider {
    Google(GoogleBooksClient),
    OpenLibrary(OpenLibraryClient),
}

impl BookProvider {
    pub fn name(&self) -> &'static str {
        match self {
            BookProvider::Google(_) => "google",
            BookProvider::OpenLibrary(_) => "openlibrary",
        }
    }

    pub async fn search(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> Result<Vec<BookCandidate>, ProviderError> {
        match self {
            BookProvider::Google(c) => c.search(title, author).await,
            BookProvider::OpenLibrary(c) => c.search(title, author).await,
        }
    }

    pub async fn get_book(&self, id: &str) -> Result<BookCandidate, ProviderError> {
        match self {
            BookProvider::Google(c) => c.get_book(id).await,
            BookProvider::OpenLibrary(c) => c.get_book(id).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Value {
        let path = format!("{}/tests/fixtures/books/{name}", env!("CARGO_MANIFEST_DIR"));
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
    }

    #[test]
    fn google_search_maps_volumes() {
        let found = parse_google_search(&fixture("google-search.json"));
        assert_eq!(found.len(), 2, "the volume without a title is skipped");
        let b = &found[0];
        assert_eq!(b.provider, "google");
        assert_eq!(b.provider_id, "gBk0001-_x");
        assert_eq!(b.title, "The Lantern Keeper");
        assert_eq!(b.subtitle.as_deref(), Some("A Harbour Tale"));
        assert_eq!(b.authors, vec!["Ada Fictional", "Bo Imaginary"]);
        assert_eq!(b.publisher.as_deref(), Some("Example Press"));
        assert_eq!(b.year, Some(2011));
        assert_eq!(
            b.description.as_deref(),
            Some("A keeper of lights.\nShe waits & watches.")
        );
        assert_eq!(b.genres, vec!["Fiction"]);
        assert_eq!(b.isbn.as_deref(), Some("9780000000002"), "ISBN-13 first");
        assert_eq!(b.language.as_deref(), Some("en"));
        assert_eq!(
            b.cover_url.as_deref(),
            Some("https://books.google.com/books/content?id=gBk0001-_x&printsec=frontcover&img=1&zoom=1&source=gbs_api"),
            "https, no page curl"
        );
        // Sparse volume: only what is there.
        let b = &found[1];
        assert_eq!(b.title, "Lantern Keeping");
        assert!(b.authors.is_empty() && b.cover_url.is_none() && b.year.is_none());
        assert_eq!(b.isbn.as_deref(), Some("0000000000"), "ISBN-10 when alone");
        assert!(parse_google_search(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn google_volume_prefers_the_largest_image() {
        let b = parse_google_volume(&fixture("google-volume.json")).unwrap();
        assert_eq!(b.provider_id, "gBk0001-_x");
        assert_eq!(
            b.cover_url.as_deref(),
            Some("https://books.google.com/books/content?id=gBk0001-_x&img=1&zoom=6")
        );
        // A non-Google host is left alone (the test stub's, say).
        let v = serde_json::json!({"id": "x1", "volumeInfo": {"title": "T",
            "imageLinks": {"thumbnail": "http://127.0.0.1:9/cover.jpg"}}});
        assert_eq!(
            parse_google_volume(&v).unwrap().cover_url.as_deref(),
            Some("http://127.0.0.1:9/cover.jpg")
        );
    }

    #[test]
    fn open_library_search_maps_docs() {
        let found = parse_open_library_search(
            &fixture("openlibrary-search.json"),
            "https://covers.example",
        );
        assert_eq!(found.len(), 2, "a doc with a non-work key is skipped");
        let b = &found[0];
        assert_eq!(b.provider, "openlibrary");
        assert_eq!(b.provider_id, "OL1000001W");
        assert_eq!(b.title, "The Lantern Keeper");
        assert_eq!(b.authors, vec!["Ada Fictional"]);
        assert_eq!(b.year, Some(2010));
        assert_eq!(b.publisher.as_deref(), Some("Example Press"));
        assert_eq!(b.isbn.as_deref(), Some("9780000000002"));
        assert_eq!(b.language.as_deref(), Some("eng"));
        assert_eq!(b.genres.len(), OPEN_LIBRARY_GENRES, "subjects are capped");
        assert_eq!(
            b.cover_url.as_deref(),
            Some("https://covers.example/b/id/424242-L.jpg?default=false")
        );
        assert!(found[1].cover_url.is_none());
    }

    #[test]
    fn queries_ids_and_text_helpers() {
        assert_eq!(
            google_query(" The Lantern Keeper ", Some("Ada Fictional")),
            "intitle:The Lantern Keeper inauthor:Ada Fictional"
        );
        assert_eq!(google_query("T", Some(" ")), "intitle:T");
        assert!(valid_google_id("gBk0001-_x"));
        assert!(!valid_google_id("../x"));
        assert!(!valid_google_id(""));
        assert!(valid_open_library_id("OL45883W"));
        assert!(!valid_open_library_id("OL45883M"));
        assert!(!valid_open_library_id("OLW"));
        assert_eq!(year_of("2005-11-15"), Some(2005));
        assert_eq!(year_of("unknown"), None);
        assert_eq!(
            strip_html("<p>One<br/>Two &amp; <b>three</b></p><p></p>"),
            "One\nTwo & three"
        );
        assert_eq!(strip_html("no tags"), "no tags");
        assert_eq!(strip_html("a < b"), "a < b");
        assert!(user_agent().starts_with("Parlour/"));
        assert!(!user_agent().contains('@'), "no contact address is sent");
    }
}
