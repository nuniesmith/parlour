//! OPML in and out for podcast feeds (POD4, D31): a pure reader that
//! lists the feeds an OPML file names, and a writer that lists a
//! podcasts library's shows. OPML is how shows move between podcast
//! apps, and how they move to a new Parlour host until the export
//! bundle carries them (POD1's note).
//!
//! - **Reading.** The root must be `<opml>` with a `<body>`. Every
//!   `<outline>` with a non-blank `xmlUrl` is a feed, at any depth (apps
//!   nest them in folders), whatever its `type` says. Its name is its
//!   `text`, else its `title`. Feeds come back in the file's order, a
//!   repeated URL included: the caller decides what a repeat means.
//! - **Writing.** OPML 2.0, one `<outline type="rss">` per show with its
//!   title as `text` and `title`, and its feed URL as `xmlUrl`.
//!
//! The rules are in `docs/AUDIOBOOKS-PODCASTS.md` → "Managing feeds
//! (POD4)".

use chrono::{DateTime, Utc};
use quick_xml::events::Event;
use quick_xml::Reader;

use crate::podcast_feed::{attr, decode_feed_text, local, qname};

/// One feed an OPML file names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpmlFeed {
    /// The outline's `text`, else its `title` (white space folded).
    pub title: Option<String>,
    /// The outline's `xmlUrl`, trimmed.
    pub feed_url: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OpmlError {
    #[error("the file is not well-formed XML: {0}")]
    Xml(String),
    #[error("not an OPML file (the document's root is <{0}>)")]
    NotOpml(String),
    #[error("the file is empty")]
    Empty,
    #[error("the OPML file has no <body>")]
    NoBody,
}

fn folded(s: &str) -> Option<String> {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.is_empty()).then_some(s)
}

/// The feeds an OPML file names, in the file's order.
pub fn parse_opml(bytes: &[u8]) -> Result<Vec<OpmlFeed>, OpmlError> {
    let text = decode_feed_text(bytes);
    let mut reader = Reader::from_str(&text);
    let xml = |e: quick_xml::Error| OpmlError::Xml(e.to_string());
    let mut seen_root = false;
    let mut seen_body = false;
    let mut feeds = Vec::new();
    loop {
        match reader.read_event().map_err(xml)? {
            Event::Eof => break,
            Event::Start(start) | Event::Empty(start) => {
                let name = qname(&start);
                if !seen_root {
                    seen_root = true;
                    if local(&name) != "opml" {
                        return Err(OpmlError::NotOpml(name));
                    }
                    continue;
                }
                match local(&name) {
                    "body" => seen_body = true,
                    "outline" => {
                        let url = attr(&start, "xmlUrl")
                            .map(|u| u.trim().to_string())
                            .filter(|u| !u.is_empty());
                        if let Some(feed_url) = url {
                            let title = attr(&start, "text")
                                .and_then(|t| folded(&t))
                                .or_else(|| attr(&start, "title").and_then(|t| folded(&t)));
                            feeds.push(OpmlFeed { title, feed_url });
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    if !seen_root {
        return Err(OpmlError::Empty);
    }
    if !seen_body {
        return Err(OpmlError::NoBody);
    }
    Ok(feeds)
}

/// One show as the export lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpmlShow {
    pub title: String,
    pub feed_url: String,
}

fn esc(s: &str) -> String {
    // Attribute text: one line, with the XML specials escaped.
    let one_line = s.split_whitespace().collect::<Vec<_>>().join(" ");
    quick_xml::escape::escape(one_line.as_str()).into_owned()
}

/// An OPML 2.0 document listing `shows` under the head title `title`.
pub fn write_opml(title: &str, created: DateTime<Utc>, shows: &[OpmlShow]) -> String {
    let mut out =
        String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<opml version=\"2.0\">\n");
    out.push_str("  <head>\n");
    out.push_str(&format!("    <title>{}</title>\n", esc(title)));
    out.push_str(&format!(
        "    <dateCreated>{}</dateCreated>\n",
        created.to_rfc2822()
    ));
    out.push_str("  </head>\n  <body>\n");
    for show in shows {
        let t = esc(&show.title);
        out.push_str(&format!(
            "    <outline type=\"rss\" text=\"{t}\" title=\"{t}\" xmlUrl=\"{}\"/>\n",
            quick_xml::escape::escape(show.feed_url.trim())
        ));
    }
    out.push_str("  </body>\n</opml>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_nested_outlines_in_order() {
        let opml = br#"<?xml version="1.0" encoding="utf-8"?>
<opml version="1.0">
  <head><title>My shows</title></head>
  <body>
    <outline text="feeds">
      <outline type="rss" text="  First   Show " xmlUrl=" https://a.example/feed.xml "/>
      <outline text="A folder">
        <outline title="Second" xmlUrl="https://b.example/rss?token=x&amp;y=1"></outline>
      </outline>
    </outline>
    <outline text="No URL here"/>
    <outline text="Blank" xmlUrl="   "/>
    <OUTLINE TYPE="link" XMLURL="https://c.example/c"/>
    <outline type="rss" text="First again" xmlUrl="https://a.example/feed.xml"/>
  </body>
</opml>"#;
        let feeds = parse_opml(opml).unwrap();
        assert_eq!(
            feeds,
            vec![
                OpmlFeed {
                    title: Some("First Show".into()),
                    feed_url: "https://a.example/feed.xml".into()
                },
                OpmlFeed {
                    title: Some("Second".into()),
                    feed_url: "https://b.example/rss?token=x&y=1".into()
                },
                OpmlFeed {
                    title: None,
                    feed_url: "https://c.example/c".into()
                },
                OpmlFeed {
                    title: Some("First again".into()),
                    feed_url: "https://a.example/feed.xml".into()
                },
            ]
        );
    }

    #[test]
    fn refuses_what_is_not_opml() {
        assert_eq!(
            parse_opml(b"<rss version=\"2.0\"><channel/></rss>"),
            Err(OpmlError::NotOpml("rss".into()))
        );
        assert_eq!(
            parse_opml(b"<html><body></body></html>"),
            Err(OpmlError::NotOpml("html".into()))
        );
        assert_eq!(
            parse_opml(b"<opml version=\"2.0\"><head/></opml>"),
            Err(OpmlError::NoBody)
        );
        assert_eq!(parse_opml(b""), Err(OpmlError::Empty));
        assert_eq!(parse_opml(b"   \n "), Err(OpmlError::Empty));
        assert!(matches!(
            parse_opml(b"<opml><body><outline"),
            Err(OpmlError::Xml(_))
        ));
        // A body with no feeds is a file with nothing to add, not an error.
        assert_eq!(parse_opml(b"<opml><body/></opml>"), Ok(vec![]));
    }

    #[test]
    fn writes_what_it_reads_back() {
        let shows = vec![
            OpmlShow {
                title: "Tom & Jerry's \"Show\" <live>".into(),
                feed_url: "https://a.example/rss?token=x&y=1".into(),
            },
            OpmlShow {
                title: "Two\nlines".into(),
                feed_url: "http://b.example/feed".into(),
            },
        ];
        let created = DateTime::parse_from_rfc3339("2026-11-26T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let doc = write_opml("Parlour — Podcasts", created, &shows);
        assert!(
            doc.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<opml version=\"2.0\">")
        );
        assert!(doc.contains("<title>Parlour — Podcasts</title>"));
        assert!(doc.contains("xmlUrl=\"https://a.example/rss?token=x&amp;y=1\""));
        let back = parse_opml(doc.as_bytes()).unwrap();
        assert_eq!(
            back,
            vec![
                OpmlFeed {
                    title: Some("Tom & Jerry's \"Show\" <live>".into()),
                    feed_url: "https://a.example/rss?token=x&y=1".into()
                },
                OpmlFeed {
                    title: Some("Two lines".into()),
                    feed_url: "http://b.example/feed".into()
                },
            ]
        );
    }
}
