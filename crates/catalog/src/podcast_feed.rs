//! Podcast feeds (POD2): RSS 2.0 with the iTunes namespace and the
//! Podcasting 2.0 chapter and transcript links, read into the shapes
//! POD1's `podcasts` / `podcast_episodes` tables hold.
//!
//! Pure: text in, values out. No network and no database. Fixture-
//! tested against `tests/fixtures/podcasts/`. The rules are written in
//! `docs/AUDIOBOOKS-PODCASTS.md` → "Feed fetch and parse (POD2)".
//!
//! Elements are recognised by their prefixed names (`itunes:`,
//! `podcast:`, `content:`), the way publishers write them, matched
//! without regard to case.

use std::borrow::Cow;
use std::collections::HashSet;

use chrono::{DateTime, Utc};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

/// The show, as its feed describes it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeedShow {
    pub title: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub language: Option<String>,
    pub explicit: bool,
    /// `episodic` or `serial`; anything else is dropped.
    pub podcast_type: Option<&'static str>,
    /// `itunes:category` names (sub-categories included), first
    /// appearance first, each once.
    pub genres: Vec<String>,
    pub image_url: Option<String>,
}

/// One feed item that is an episode (it has an enclosure URL).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeedEpisode {
    /// The item's `<guid>`, or its enclosure URL when it has none.
    pub guid: String,
    /// `<title>`, else `itunes:title`, else empty.
    pub title: String,
    pub subtitle: Option<String>,
    pub description: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<i64>,
    pub season: Option<i32>,
    pub episode_number: Option<i32>,
    /// `full`, `trailer` or `bonus`; anything else is dropped.
    pub episode_type: Option<&'static str>,
    pub enclosure_url: String,
    pub enclosure_type: Option<String>,
    pub enclosure_length: Option<i64>,
    pub chapters_url: Option<String>,
    pub transcript_url: Option<String>,
    pub transcript_type: Option<String>,
}

/// A whole feed: the show and its episodes in feed order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Feed {
    pub show: FeedShow,
    pub episodes: Vec<FeedEpisode>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FeedError {
    #[error("the feed is not well-formed XML: {0}")]
    Xml(String),
    #[error("not an RSS feed (the document's root is <{0}>)")]
    NotRss(String),
    #[error("the feed has no <channel>")]
    NoChannel,
}

/// The feed's bytes as text: UTF-8 (a byte-order mark is dropped),
/// and anything that isn't valid UTF-8 read as Latin-1.
pub fn decode_feed_text(bytes: &[u8]) -> Cow<'_, str> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => Cow::Borrowed(s),
        Err(_) => Cow::Owned(bytes.iter().map(|&b| b as char).collect()),
    }
}

/// Parse a feed's bytes.
pub fn parse_feed_bytes(bytes: &[u8]) -> Result<Feed, FeedError> {
    parse_feed(&decode_feed_text(bytes))
}

/// The item being read.
#[derive(Default)]
struct ItemDraft {
    guid: Option<String>,
    title: Option<String>,
    itunes_title: Option<String>,
    subtitle: Option<String>,
    description: Option<String>,
    content_encoded: Option<String>,
    summary: Option<String>,
    pub_date: Option<String>,
    duration: Option<String>,
    season: Option<String>,
    episode: Option<String>,
    episode_type: Option<String>,
    enclosure_url: Option<String>,
    enclosure_type: Option<String>,
    enclosure_length: Option<String>,
    chapters_url: Option<String>,
    transcript: Option<(String, Option<String>)>,
}

#[derive(Default)]
struct ShowDraft {
    title: Option<String>,
    author: Option<String>,
    plain_author: Option<String>,
    description: Option<String>,
    summary: Option<String>,
    language: Option<String>,
    explicit: Option<String>,
    podcast_type: Option<String>,
    genres: Vec<String>,
    itunes_image: Option<String>,
    image_url: Option<String>,
}

/// Parse a feed's text.
pub fn parse_feed(text: &str) -> Result<Feed, FeedError> {
    let mut reader = Reader::from_str(text);
    let xml = |e: quick_xml::Error| FeedError::Xml(e.to_string());

    // The open elements (lower-cased qualified names) and the text
    // gathered for each.
    let mut stack: Vec<(String, String)> = Vec::new();
    let mut seen_root = false;
    let mut seen_channel = false;
    let mut show = ShowDraft::default();
    let mut item: Option<ItemDraft> = None;
    let mut episodes: Vec<FeedEpisode> = Vec::new();
    let mut guids: HashSet<String> = HashSet::new();

    loop {
        let event = reader.read_event().map_err(xml)?;
        match event {
            Event::Eof => break,
            Event::Start(start) | Event::Empty(start) if !seen_root => {
                let name = qname(&start);
                seen_root = true;
                if local(&name) != "rss" {
                    return Err(FeedError::NotRss(name));
                }
                // An empty `<rss/>` has nothing in it.
                stack.push((name, String::new()));
            }
            Event::Start(start) => {
                let name = qname(&start);
                on_open(
                    &name,
                    &start,
                    &stack,
                    &mut show,
                    &mut item,
                    &mut seen_channel,
                );
                stack.push((name, String::new()));
            }
            Event::Empty(start) => {
                let name = qname(&start);
                on_open(
                    &name,
                    &start,
                    &stack,
                    &mut show,
                    &mut item,
                    &mut seen_channel,
                );
                on_close(
                    &name,
                    "",
                    &stack,
                    &mut show,
                    &mut item,
                    &mut episodes,
                    &mut guids,
                );
            }
            Event::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    let t = t.decode().map_err(|e| FeedError::Xml(e.to_string()))?;
                    top.1.push_str(&t);
                }
            }
            Event::CData(t) => {
                if let Some(top) = stack.last_mut() {
                    let t = t.decode().map_err(|e| FeedError::Xml(e.to_string()))?;
                    top.1.push_str(&t);
                }
            }
            Event::GeneralRef(r) => {
                if let Some(top) = stack.last_mut() {
                    match r.resolve_char_ref().map_err(xml)? {
                        Some(ch) => top.1.push(ch),
                        None => {
                            let name = r.decode().map_err(|e| FeedError::Xml(e.to_string()))?;
                            top.1.push_str(&resolve_entity(&name));
                        }
                    }
                }
            }
            Event::End(_) => {
                let Some((name, text)) = stack.pop() else {
                    continue;
                };
                on_close(
                    &name,
                    &text,
                    &stack,
                    &mut show,
                    &mut item,
                    &mut episodes,
                    &mut guids,
                );
                if stack.is_empty() {
                    break;
                }
            }
            // Declarations, comments, processing instructions and a
            // doctype: skipped.
            _ => {}
        }
    }
    if !seen_root {
        return Err(FeedError::NotRss(String::new()));
    }
    if !seen_channel {
        return Err(FeedError::NoChannel);
    }
    Ok(Feed {
        show: finish_show(show),
        episodes,
    })
}

fn qname(start: &BytesStart<'_>) -> String {
    String::from_utf8_lossy(start.name().as_ref()).to_ascii_lowercase()
}

fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// An attribute's value, entities resolved where they can be.
fn attr(start: &BytesStart<'_>, key: &str) -> Option<String> {
    for a in start.attributes().with_checks(false).flatten() {
        if String::from_utf8_lossy(a.key.as_ref()).eq_ignore_ascii_case(key) {
            let v = match a.unescape_value() {
                Ok(v) => v.into_owned(),
                Err(_) => String::from_utf8_lossy(&a.value).into_owned(),
            };
            return Some(v);
        }
    }
    None
}

/// An entity XML doesn't define. Feeds often carry HTML's; the common
/// ones are read, and anything else is kept as written.
fn resolve_entity(name: &str) -> String {
    if let Some(s) = quick_xml::escape::resolve_predefined_entity(name) {
        return s.to_string();
    }
    let ch = match name {
        "nbsp" => '\u{a0}',
        "ndash" => '–',
        "mdash" => '—',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        "hellip" => '…',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        _ => return format!("&{name};"),
    };
    ch.to_string()
}

/// The names of the open elements, outermost first: `["rss",
/// "channel", "item"]` is inside an item.
fn path(stack: &[(String, String)]) -> Vec<&str> {
    stack.iter().map(|(n, _)| n.as_str()).collect()
}

fn in_channel(stack: &[(String, String)]) -> bool {
    path(stack) == ["rss", "channel"]
}

fn in_item(stack: &[(String, String)]) -> bool {
    path(stack) == ["rss", "channel", "item"]
}

/// An element opens: the attributes that carry the data.
fn on_open(
    name: &str,
    start: &BytesStart<'_>,
    stack: &[(String, String)],
    show: &mut ShowDraft,
    item: &mut Option<ItemDraft>,
    seen_channel: &mut bool,
) {
    let p = path(stack);
    if name == "channel" && p == ["rss"] {
        *seen_channel = true;
        return;
    }
    if name == "item" && p == ["rss", "channel"] {
        *item = Some(ItemDraft::default());
        return;
    }
    if in_item(stack) {
        let Some(it) = item.as_mut() else { return };
        match name {
            "enclosure" if it.enclosure_url.is_none() => {
                it.enclosure_url = attr(start, "url");
                it.enclosure_type = attr(start, "type");
                it.enclosure_length = attr(start, "length");
            }
            "podcast:chapters" if it.chapters_url.is_none() => {
                it.chapters_url = attr(start, "url");
            }
            // The first transcript the item lists.
            "podcast:transcript" if it.transcript.is_none() => {
                if let Some(url) = attr(start, "url") {
                    it.transcript = Some((url, attr(start, "type")));
                }
            }
            _ => {}
        }
        return;
    }
    // Channel level, or the categories nested under it.
    if p.len() >= 2 && p[..2] == ["rss", "channel"] && !p.contains(&"item") {
        match name {
            "itunes:image" if p.len() == 2 && show.itunes_image.is_none() => {
                show.itunes_image = attr(start, "href");
            }
            "itunes:category" if p[2..].iter().all(|n| *n == "itunes:category") => {
                if let Some(text) = attr(start, "text").map(|t| clean_line(&t)) {
                    if !text.is_empty() && !show.genres.contains(&text) {
                        show.genres.push(text);
                    }
                }
            }
            _ => {}
        }
    }
}

/// An element closes with the text gathered inside it.
fn on_close(
    name: &str,
    text: &str,
    stack: &[(String, String)],
    show: &mut ShowDraft,
    item: &mut Option<ItemDraft>,
    episodes: &mut Vec<FeedEpisode>,
    guids: &mut HashSet<String>,
) {
    let set = |slot: &mut Option<String>| {
        if slot.is_none() {
            *slot = Some(text.to_string());
        }
    };
    if name == "item" && path(stack) == ["rss", "channel"] {
        if let Some(it) = item.take() {
            if let Some(ep) = finish_item(it) {
                // A guid the feed repeats: the first item keeps it.
                if guids.insert(ep.guid.clone()) {
                    episodes.push(ep);
                }
            }
        }
        return;
    }
    if in_item(stack) {
        let Some(it) = item.as_mut() else { return };
        match name {
            "guid" => set(&mut it.guid),
            "title" => set(&mut it.title),
            "itunes:title" => set(&mut it.itunes_title),
            "itunes:subtitle" => set(&mut it.subtitle),
            "description" => set(&mut it.description),
            "content:encoded" => set(&mut it.content_encoded),
            "itunes:summary" => set(&mut it.summary),
            "pubdate" => set(&mut it.pub_date),
            "itunes:duration" => set(&mut it.duration),
            "itunes:season" => set(&mut it.season),
            "itunes:episode" => set(&mut it.episode),
            "itunes:episodetype" => set(&mut it.episode_type),
            _ => {}
        }
        return;
    }
    if in_channel(stack) {
        match name {
            "title" => set(&mut show.title),
            "itunes:author" => set(&mut show.author),
            "author" => set(&mut show.plain_author),
            "description" => set(&mut show.description),
            "itunes:summary" => set(&mut show.summary),
            "language" => set(&mut show.language),
            "itunes:explicit" => set(&mut show.explicit),
            "itunes:type" => set(&mut show.podcast_type),
            _ => {}
        }
        return;
    }
    if name == "url" && path(stack) == ["rss", "channel", "image"] {
        set(&mut show.image_url);
    }
}

/// Single-line text: trimmed, inner runs of white space made one space.
fn clean_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Single-line text, or nothing when it is empty.
fn line(s: Option<String>) -> Option<String> {
    s.map(|s| clean_line(&s)).filter(|s| !s.is_empty())
}

/// Multi-line text (descriptions): trimmed, or nothing when empty.
fn block(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn finish_show(d: ShowDraft) -> FeedShow {
    FeedShow {
        title: line(d.title),
        author: line(d.author).or_else(|| line(d.plain_author)),
        description: block(d.description).or_else(|| block(d.summary)),
        language: line(d.language),
        explicit: d.explicit.as_deref().is_some_and(parse_explicit),
        podcast_type: d.podcast_type.as_deref().and_then(parse_podcast_type),
        genres: d.genres,
        image_url: line(d.itunes_image).or_else(|| line(d.image_url)),
    }
}

/// The item as an episode, or `None` when it has no enclosure URL
/// (a feed item without audio isn't an episode).
fn finish_item(d: ItemDraft) -> Option<FeedEpisode> {
    let enclosure_url = line(d.enclosure_url)?;
    let guid = d
        .guid
        .map(|g| g.trim().to_string())
        .filter(|g| !g.is_empty())
        .unwrap_or_else(|| enclosure_url.clone());
    let (transcript_url, transcript_type) = match d.transcript {
        Some((url, kind)) => match line(Some(url)) {
            Some(url) => (Some(url), line(kind)),
            None => (None, None),
        },
        None => (None, None),
    };
    Some(FeedEpisode {
        guid,
        title: line(d.title)
            .or_else(|| line(d.itunes_title))
            .unwrap_or_default(),
        subtitle: line(d.subtitle),
        description: block(d.description)
            .or_else(|| block(d.content_encoded))
            .or_else(|| block(d.summary)),
        published_at: d.pub_date.as_deref().and_then(parse_pub_date),
        duration_ms: d.duration.as_deref().and_then(parse_duration_ms),
        season: d.season.as_deref().and_then(parse_whole),
        episode_number: d.episode.as_deref().and_then(parse_whole),
        episode_type: d.episode_type.as_deref().and_then(parse_episode_type),
        enclosure_url,
        enclosure_type: line(d.enclosure_type),
        enclosure_length: d.enclosure_length.as_deref().and_then(parse_length),
        chapters_url: line(d.chapters_url),
        transcript_url,
        transcript_type,
    })
}

/// `itunes:explicit`: `yes` / `true` / `explicit` mean explicit;
/// anything else (`no`, `false`, `clean`, nothing) does not.
pub fn parse_explicit(s: &str) -> bool {
    matches!(
        s.trim().to_ascii_lowercase().as_str(),
        "yes" | "true" | "explicit"
    )
}

pub fn parse_podcast_type(s: &str) -> Option<&'static str> {
    match s.trim().to_ascii_lowercase().as_str() {
        "episodic" => Some("episodic"),
        "serial" => Some("serial"),
        _ => None,
    }
}

pub fn parse_episode_type(s: &str) -> Option<&'static str> {
    match s.trim().to_ascii_lowercase().as_str() {
        "full" => Some("full"),
        "trailer" => Some("trailer"),
        "bonus" => Some("bonus"),
        _ => None,
    }
}

/// A season or episode number: a whole number 0 or more. Anything
/// else (`Two`, `4a`, `-2`, `1.5`) is dropped (POD1 reading 12).
pub fn parse_whole(s: &str) -> Option<i32> {
    let s = s.trim();
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// An enclosure's byte length. Unreadable, negative or 0 (the usual
/// placeholder) is no length.
pub fn parse_length(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<i64>().ok().filter(|n| *n > 0)
}

/// `itunes:duration` in milliseconds: `H:MM:SS`, `M:SS` or plain
/// seconds, the seconds possibly with a fraction. Minutes past 59 in
/// `M:SS` are read as written (`75:30`). Anything else, or 0, is no
/// duration.
pub fn parse_duration_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let (whole, last) = parts.split_at(parts.len() - 1);
    let mut seconds: i64 = 0;
    for p in whole {
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        seconds = seconds
            .checked_mul(60)?
            .checked_add(p.parse::<i64>().ok()?)?;
    }
    let last = last[0];
    let (int, frac) = last.split_once('.').unwrap_or((last, ""));
    if int.is_empty()
        || !int.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let ms_frac: i64 = if frac.is_empty() {
        0
    } else {
        // Milliseconds from the first three digits of the fraction.
        let digits: String = frac.chars().chain("000".chars()).take(3).collect();
        digits.parse().ok()?
    };
    let total = seconds
        .checked_mul(60)?
        .checked_add(int.parse::<i64>().ok()?)?
        .checked_mul(1000)?
        .checked_add(ms_frac)?;
    (total > 0).then_some(total)
}

/// A `pubDate`. RSS says RFC 822 dates; feeds also use ISO 8601, give
/// the wrong weekday, or name a zone RFC 822 doesn't know. A wrong
/// weekday is ignored; an unknown zone name is read as UTC.
/// Unreadable is no date.
pub fn parse_pub_date(s: &str) -> Option<DateTime<Utc>> {
    let s = clean_line(s);
    if s.is_empty() {
        return None;
    }
    let try_both = |t: &str| {
        DateTime::parse_from_rfc2822(t)
            .or_else(|_| DateTime::parse_from_rfc3339(t))
            .ok()
            .map(|d| d.with_timezone(&Utc))
    };
    if let Some(d) = try_both(&s) {
        return Some(d);
    }
    // Drop the weekday ("Mon, "): a wrong one makes the date invalid.
    let no_weekday = match s.split_once(',') {
        Some((day, rest)) if day.chars().all(|c| c.is_ascii_alphabetic()) => rest.trim(),
        _ => s.as_str(),
    };
    if let Some(d) = try_both(no_weekday) {
        return Some(d);
    }
    // An unknown zone name (BST, CEST, …): read as UTC.
    let (head, zone) = no_weekday.rsplit_once(' ')?;
    if !zone.is_empty() && zone.chars().all(|c| c.is_ascii_alphabetic()) {
        return try_both(&format!("{head} +0000"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const EPISODIC: &[u8] = include_bytes!("../tests/fixtures/podcasts/episodic.xml");
    const SERIAL: &[u8] = include_bytes!("../tests/fixtures/podcasts/serial.xml");
    const ODD: &[u8] = include_bytes!("../tests/fixtures/podcasts/odd.xml");
    const NOT_A_FEED: &[u8] = include_bytes!("../tests/fixtures/podcasts/not-a-feed.html");

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    #[test]
    fn an_episodic_feed() {
        let feed = parse_feed_bytes(EPISODIC).unwrap();
        assert_eq!(
            feed.show,
            FeedShow {
                title: Some("The Lantern Hour".into()),
                author: Some("Imaginary Radio".into()),
                description: Some(
                    "Weekly conversations about lighthouses, lamps & the people who kept them."
                        .into()
                ),
                language: Some("en-ca".into()),
                explicit: false,
                podcast_type: Some("episodic"),
                genres: vec![
                    "History".into(),
                    "Society & Culture".into(),
                    "Places & Travel".into()
                ],
                image_url: Some("https://cdn.example.com/lantern-hour/cover-3000.jpg".into()),
            }
        );
        assert_eq!(feed.episodes.len(), 3);
        let ep3 = &feed.episodes[0];
        assert_eq!(
            ep3,
            &FeedEpisode {
                guid: "lantern-hour-0003".into(),
                title: "Ep. 3: The Keeper's Logbook".into(),
                subtitle: Some("What a century of weather notes tells us.".into()),
                description: Some(
                    "<p>We read the <b>logbook</b> of a keeper on a rocky point.</p>".into()
                ),
                published_at: Some(at(2026, 11, 19, 10, 0, 0)),
                duration_ms: Some(3_723_000),
                season: Some(1),
                episode_number: Some(3),
                episode_type: Some("full"),
                enclosure_url: "https://media.example.com/lantern-hour/ep3.mp3?source=feed&n=3"
                    .into(),
                enclosure_type: Some("audio/mpeg".into()),
                enclosure_length: Some(48_213_760),
                chapters_url: Some(
                    "https://media.example.com/lantern-hour/ep3.chapters.json".into()
                ),
                transcript_url: Some("https://media.example.com/lantern-hour/ep3.vtt".into()),
                transcript_type: Some("text/vtt".into()),
            }
        );
        let ep2 = &feed.episodes[1];
        assert_eq!(ep2.title, "Ep. 2: Fresnel & Friends");
        assert_eq!(
            ep2.description.as_deref(),
            Some("Lenses, prisms — and a visit to a lens workshop.")
        );
        assert_eq!(ep2.published_at, Some(at(2026, 11, 12, 10, 0, 0)));
        assert_eq!(ep2.duration_ms, Some(2_730_000));
        assert_eq!(ep2.episode_type, None);
        assert_eq!(ep2.chapters_url, None);
        let trailer = &feed.episodes[2];
        assert_eq!(trailer.episode_type, Some("trailer"));
        assert_eq!(trailer.duration_ms, Some(125_000));
        assert_eq!(trailer.published_at, Some(at(2026, 11, 2, 14, 30, 0)));
        assert_eq!(
            trailer.description.as_deref(),
            Some("A two-minute taste of the show.")
        );
        assert_eq!((trailer.season, trailer.episode_number), (None, None));
    }

    #[test]
    fn a_serial_feed() {
        let feed = parse_feed_bytes(SERIAL).unwrap();
        assert_eq!(feed.show.podcast_type, Some("serial"));
        assert!(feed.show.explicit);
        assert_eq!(
            feed.show.author.as_deref(),
            Some("Bo Imaginary & Ada Fictional")
        );
        assert_eq!(
            feed.show.image_url.as_deref(),
            Some("https://cdn.example.com/ice-shelf/art.png"),
            "<image><url> stands in for itunes:image"
        );
        assert_eq!(feed.show.genres, vec!["Fiction", "Drama"]);
        let [c2, c1, bonus] = &feed.episodes[..] else {
            panic!("three episodes: {:?}", feed.episodes)
        };
        // Non-numeric season/episode values are dropped.
        assert_eq!((c2.season, c2.episode_number), (None, None));
        assert_eq!(c2.duration_ms, Some(4_530_000), "75:30");
        assert_eq!(c2.enclosure_length, None, "0 is no length");
        assert_eq!((c1.season, c1.episode_number), (Some(1), Some(1)));
        assert_eq!(c1.duration_ms, Some(3_723_500), "1:02:03.5");
        assert_eq!(c1.enclosure_length, None, "abc is no length");
        assert_eq!(bonus.episode_type, Some("bonus"));
        assert_eq!(bonus.episode_number, None, "-2 is dropped");
        assert_eq!(bonus.duration_ms, Some(1_834_750));
        assert_eq!(bonus.enclosure_length, None, "absent");
    }

    #[test]
    fn odd_items() {
        let feed = parse_feed_bytes(ODD).unwrap();
        assert_eq!(feed.show.title.as_deref(), Some("Odd Ends"));
        assert!(!feed.show.explicit, "clean");
        assert_eq!(feed.show.podcast_type, None, "daily is not a type");
        assert_eq!(feed.show.description, None);
        assert_eq!(feed.show.author, None);
        assert_eq!(feed.show.image_url, None);
        let guids: Vec<&str> = feed.episodes.iter().map(|e| e.guid.as_str()).collect();
        assert_eq!(
            guids,
            [
                // No guid / an empty guid: the enclosure URL stands in.
                "https://media.example.com/odd/no-guid.mp3",
                "https://media.example.com/odd/empty-guid.mp3",
                // No enclosure (a post) or an empty URL: not episodes.
                // A repeated guid: the first item keeps it.
                "odd-3",
                "odd-4",
                "odd-5",
            ]
        );
        let e = &feed.episodes;
        assert_eq!(
            e[0].published_at,
            Some(at(2026, 11, 20, 13, 15, 0)),
            "ISO 8601"
        );
        assert_eq!(e[0].duration_ms, None, "empty");
        assert_eq!(e[1].published_at, Some(at(2026, 11, 20, 12, 0, 0)), "EST");
        assert_eq!(e[1].duration_ms, None, "abc");
        assert_eq!(e[1].enclosure_type, None);
        assert_eq!(
            e[2].title, "Wrong weekday and an HTML entity",
            "&nbsp; is read (and folded, as white space in a title)"
        );
        assert_eq!(
            e[2].published_at,
            Some(at(2026, 11, 18, 7, 0, 0)),
            "the wrong weekday is ignored"
        );
        assert_eq!(e[2].duration_ms, None, "0");
        assert_eq!(e[2].enclosure_length, None, "-5");
        assert_eq!(e[3].published_at, None, "unreadable");
        assert_eq!(e[3].duration_ms, None, "1:2:x");
        assert_eq!(e[4].title, "Only an iTunes title");
        assert_eq!(
            e[4].published_at,
            Some(at(2026, 11, 16, 7, 0, 0)),
            "no weekday, no seconds"
        );
    }

    #[test]
    fn documents_that_are_not_feeds() {
        assert_eq!(
            parse_feed_bytes(NOT_A_FEED),
            Err(FeedError::NotRss("html".into()))
        );
        assert_eq!(
            parse_feed("<feed xmlns=\"http://www.w3.org/2005/Atom\"></feed>"),
            Err(FeedError::NotRss("feed".into()))
        );
        assert_eq!(
            parse_feed("<rss version=\"2.0\"></rss>"),
            Err(FeedError::NoChannel)
        );
        assert_eq!(parse_feed("<rss/>"), Err(FeedError::NoChannel));
        assert_eq!(parse_feed(""), Err(FeedError::NotRss(String::new())));
        assert!(matches!(
            parse_feed("<rss><channel><title>x</channel></rss>"),
            Err(FeedError::Xml(_))
        ));
    }

    #[test]
    fn latin1_and_a_byte_order_mark() {
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend_from_slice(
            b"<rss><channel><title>Caf\xc3\xa9</title><item><enclosure url=\"u\"/></item></channel></rss>",
        );
        assert_eq!(
            parse_feed_bytes(&bytes).unwrap().show.title.as_deref(),
            Some("Café")
        );
        let latin1 = b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><rss><channel><title>Caf\xe9</title></channel></rss>";
        assert_eq!(
            parse_feed_bytes(latin1).unwrap().show.title.as_deref(),
            Some("Café")
        );
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration_ms("3600"), Some(3_600_000));
        assert_eq!(parse_duration_ms(" 01:00:00 "), Some(3_600_000));
        assert_eq!(parse_duration_ms("59:59"), Some(3_599_000));
        assert_eq!(parse_duration_ms("90:00"), Some(5_400_000));
        assert_eq!(parse_duration_ms("12.3456"), Some(12_345));
        assert_eq!(parse_duration_ms("0:00:01.5"), Some(1_500));
        assert_eq!(parse_duration_ms("5."), Some(5_000), "an empty fraction");
        for bad in [
            "", "0", "00:00:00", "abc", "1:2:x", "1:2:3:4", ":30", "1::2", "-5", "1e3", "45 min",
        ] {
            assert_eq!(parse_duration_ms(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn whole_numbers_lengths_and_flags() {
        assert_eq!(parse_whole("0"), Some(0));
        assert_eq!(parse_whole(" 12 "), Some(12));
        for bad in ["", "Two", "4a", "-2", "+3", "1.5", "99999999999"] {
            assert_eq!(parse_whole(bad), None, "{bad:?}");
        }
        assert_eq!(parse_length("1024"), Some(1024));
        assert_eq!(parse_length("0"), None);
        assert!(parse_explicit(" Yes "));
        assert!(parse_explicit("explicit"));
        assert!(!parse_explicit("clean"));
        assert_eq!(parse_episode_type("Trailer"), Some("trailer"));
        assert_eq!(parse_episode_type("teaser"), None);
    }

    #[test]
    fn publication_dates() {
        assert_eq!(
            parse_pub_date("Wed, 25 Nov 2026 18:00:00 PST"),
            Some(at(2026, 11, 26, 2, 0, 0))
        );
        assert_eq!(
            parse_pub_date("Tue, 17 Nov 2026 07:00:00 BST"),
            Some(at(2026, 11, 17, 7, 0, 0)),
            "an unknown zone reads as UTC"
        );
        assert_eq!(parse_pub_date("2026-11-20"), None);
        assert_eq!(parse_pub_date("   "), None);
    }
}
