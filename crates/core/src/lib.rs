//! Parlour core types.
//!
//! This crate has **no I/O** and no dependencies on the other crates.
//! Everything else in the workspace builds on the types defined here.
//!
//! Design rule: if a change here forces a rewrite in `playback`,
//! `catalog`, or `jobs`, the boundary is wrong — fix the boundary,
//! not the feature. See `docs/ARCHITECTURE.md` §11.

use std::fmt;

/// The kind of library a collection belongs to. Movies and TV ship
/// first; the rest are planned growth, not stubs to delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LibraryKind {
    Movies,
    Shows,
    Audiobooks,
    Ebooks,
    Music,
    Podcasts,
}

/// A client family, as an explicit capability profile.
/// Never guess capabilities from a User-Agent string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ClientKind {
    Web,
    AndroidPhone,
    AndroidTv, // includes Fire TV
    Ios,
    TvOs,
}

/// How the client reached the server. Drives remote bitrate caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NetworkKind {
    Lan,
    Tailnet,
    Public,
}

/// What the playback decision engine chose for a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackAction {
    DirectPlay,
    Remux,
    Transcode,
}

/// Why that action was chosen. Always surfaced in the UI —
/// "better" includes explaining the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackReason {
    /// File is compatible and bandwidth is fine.
    Direct,
    /// Codecs are fine, the container is not.
    ContainerUnsupported,
    /// A codec the client cannot decode.
    CodecUnsupported,
    /// Remote connection / per-user bitrate cap.
    RemoteBandwidth,
    /// HDR source, SDR client: tone mapping required.
    HdrToneMap,
    /// A selected picture subtitle must be burned in for this client.
    SubtitleBurnIn,
}

/// The result of a playback decision: what to do, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaybackPlan {
    pub action: PlaybackAction,
    pub reason: PlaybackReason,
}

impl PlaybackPlan {
    pub fn direct() -> Self {
        Self {
            action: PlaybackAction::DirectPlay,
            reason: PlaybackReason::Direct,
        }
    }
}

/// What a media segment covers (SEG1, D15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SegmentKind {
    Recap,
    Intro,
    Credits,
}

impl SegmentKind {
    /// The wire / database name (`media_segments.kind`).
    pub fn as_str(self) -> &'static str {
        match self {
            SegmentKind::Recap => "recap",
            SegmentKind::Intro => "intro",
            SegmentKind::Credits => "credits",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "recap" => Some(SegmentKind::Recap),
            "intro" => Some(SegmentKind::Intro),
            "credits" => Some(SegmentKind::Credits),
            _ => None,
        }
    }
}

/// Where a media segment came from (SEG1, D15). Manual edits win
/// over every automatic source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SegmentSource {
    /// Chapter markers in the file (SEG2).
    Chapters,
    /// Audio-fingerprint match across a season (SEG3).
    Fingerprint,
    /// Entered or corrected by an admin (SEG4).
    Manual,
}

impl SegmentSource {
    /// The wire / database name (`media_segments.source`).
    pub fn as_str(self) -> &'static str {
        match self {
            SegmentSource::Chapters => "chapters",
            SegmentSource::Fingerprint => "fingerprint",
            SegmentSource::Manual => "manual",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "chapters" => Some(SegmentSource::Chapters),
            "fingerprint" => Some(SegmentSource::Fingerprint),
            "manual" => Some(SegmentSource::Manual),
            _ => None,
        }
    }

    pub fn is_manual(self) -> bool {
        self == SegmentSource::Manual
    }
}

/// One stored segment of an item's file: `[start_ms, end_ms)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MediaSegment {
    pub kind: SegmentKind,
    pub start_ms: i64,
    pub end_ms: i64,
    pub source: SegmentSource,
    /// 0.0–1.0; manual rows are 1.0.
    pub confidence: f32,
}

/// Opaque item identifier (backed by the database id for now).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ItemId(pub String);

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_names_round_trip() {
        for kind in [SegmentKind::Recap, SegmentKind::Intro, SegmentKind::Credits] {
            assert_eq!(SegmentKind::parse(kind.as_str()), Some(kind));
        }
        for source in [
            SegmentSource::Chapters,
            SegmentSource::Fingerprint,
            SegmentSource::Manual,
        ] {
            assert_eq!(SegmentSource::parse(source.as_str()), Some(source));
        }
        assert_eq!(SegmentKind::parse("outro"), None);
        assert_eq!(SegmentSource::parse("guess"), None);
    }

    #[test]
    fn direct_plan_is_direct_play() {
        let plan = PlaybackPlan::direct();
        assert_eq!(plan.action, PlaybackAction::DirectPlay);
        assert_eq!(plan.reason, PlaybackReason::Direct);
    }
}
