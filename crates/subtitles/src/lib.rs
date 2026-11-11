//! Parlour subtitles (placeholder).
//!
//! Subtitles are a first-class subsystem — the server owner uses
//! them on most playback.
//!
//! Planned behaviour:
//! - On scan, index every subtitle stream: codec, language, forced,
//!   default, SDH. Extract embedded text subtitles; generate WebVTT
//!   copies for the web player.
//! - Text subtitles are served as sidecars; no transcode.
//! - Picture subtitles (PGS/VobSub/DVB) pass through to clients that
//!   can render them (native Android/Media3); in the web player,
//!   selecting one means burn-in and therefore a transcode — and the
//!   transcode reason says so.
//! - Forced-track rule: foreign-dialogue track when the audio is in
//!   the user's language, full track otherwise.
//! - OpenSubtitles downloads are background jobs, never in the
//!   playback path.

/// How a subtitle track is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleKind {
    /// Text formats: SRT, ASS/SSA, MOV text, external files.
    Text,
    /// Bitmap formats: PGS, VobSub, DVB — may need burn-in.
    Picture,
}

/// One subtitle stream or sidecar file belonging to a media file.
#[derive(Debug, Clone)]
pub struct SubtitleTrack {
    pub kind: SubtitleKind,
    pub language: Option<String>,
    pub forced: bool,
    pub default: bool,
    pub sdh: bool,
}
