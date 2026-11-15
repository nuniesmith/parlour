//! Parlour playback decision engine (Milestone 1.6, P1 + P6).
//!
//! A **pure decision function**: given the file's probe facts, the
//! client's capability profile, the network the request arrived
//! on, and the profile's remote bitrate cap, decide DirectPlay or
//! Remux — with a reason the UI can show. Phase 1 never plans a
//! Transcode (that is the Phase 2 engine); when a codec is outside
//! the client's set the plan is Remux with reason
//! `CodecUnsupported` — the best Phase 1 can offer, and the UI
//! says so plainly.
//!
//! Also here: the subtitle auto-pick rules (P6) and the segment
//! precedence rule (SEG1), pure so the server and future clients
//! agree on them.

use parlour_core::{
    ClientKind, MediaSegment, NetworkKind, PlaybackAction, PlaybackPlan, PlaybackReason,
    SegmentKind,
};

/// Probe facts about one media file (from `media_streams`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileFacts {
    pub container: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    /// Overall bit rate of the file, kbps, when known.
    pub bitrate_kbps: Option<i64>,
}

/// What a client family can play natively. Never guessed from a
/// User-Agent: the client declares its kind, the profile is here.
#[derive(Debug, Clone, Copy)]
pub struct ClientCaps {
    pub containers: &'static [&'static str],
    pub video_codecs: &'static [&'static str],
    pub audio_codecs: &'static [&'static str],
}

/// Browser `<video>` (Phase 1's only real client). The sets are
/// deliberately generous — Chromium/Firefox/Safari between them
/// cover these in an MP4/WebM wrapper; anything outside goes the
/// Remux path.
pub const WEB_CAPS: ClientCaps = ClientCaps {
    containers: &["mp4", "m4v", "webm", "mov"],
    video_codecs: &["h264", "hevc", "vp9", "av1", "vp8"],
    audio_codecs: &[
        "aac", "mp3", "opus", "vorbis", "flac", "ac3", "eac3", "alac",
    ],
};

/// Native-app clients play essentially anything the OS ships;
/// for Phase 1 they get the same generous table with MKV added.
pub const NATIVE_CAPS: ClientCaps = ClientCaps {
    containers: &[
        "mp4", "m4v", "webm", "mov", "matroska", "mkv", "avi", "ts", "m2ts",
    ],
    video_codecs: &[
        "h264",
        "hevc",
        "vp9",
        "av1",
        "vp8",
        "mpeg2video",
        "mpeg4",
        "vc1",
    ],
    audio_codecs: &[
        "aac",
        "mp3",
        "opus",
        "vorbis",
        "flac",
        "ac3",
        "eac3",
        "alac",
        "dts",
        "truehd",
        "pcm_s16le",
    ],
};

pub fn caps_for(client: ClientKind) -> ClientCaps {
    match client {
        ClientKind::Web => WEB_CAPS,
        _ => NATIVE_CAPS,
    }
}

fn contains(set: &[&str], value: &str) -> bool {
    set.iter().any(|v| v.eq_ignore_ascii_case(value))
}

/// Normalise container names: ffprobe reports "matroska,webm"
/// (already split by the catalog probe) and "mov,mp4,..." — the
/// scanner stored the first token, so "mov" means the QuickTime
/// family and plays like mp4 in browsers.
fn container_ok(caps: &ClientCaps, container: &str) -> bool {
    contains(caps.containers, container)
        || (container.eq_ignore_ascii_case("mov") && contains(caps.containers, "mp4"))
}

/// Decide the Phase 1 plan for one file + client + network.
pub fn decide(
    facts: &FileFacts,
    client: ClientKind,
    network: NetworkKind,
    remote_cap_kbps: Option<i32>,
) -> PlaybackPlan {
    let caps = caps_for(client);
    let video_ok = facts
        .video_codec
        .as_deref()
        .map(|c| contains(caps.video_codecs, c))
        .unwrap_or(true);
    let audio_ok = facts
        .audio_codec
        .as_deref()
        .map(|c| contains(caps.audio_codecs, c))
        .unwrap_or(true);
    let container = facts.container.as_deref().unwrap_or("");
    let container_is_ok = container_ok(&caps, container);

    let over_cap = network != NetworkKind::Lan
        && match (remote_cap_kbps, facts.bitrate_kbps) {
            (Some(cap), Some(rate)) if cap > 0 => rate > i64::from(cap),
            _ => false,
        };

    if !(video_ok && audio_ok) {
        // A codec the client cannot decode. Phase 1's only tool
        // is the remuxer; the reason carries the truth.
        return PlaybackPlan {
            action: PlaybackAction::Remux,
            reason: PlaybackReason::CodecUnsupported,
        };
    }
    if !container_is_ok {
        return PlaybackPlan {
            action: PlaybackAction::Remux,
            reason: PlaybackReason::ContainerUnsupported,
        };
    }
    if over_cap {
        // Direct play still works — it will just use more than
        // the cap. The plan says so (reason = RemoteBandwidth)
        // instead of pretending; Phase 2 transcode will act on it.
        return PlaybackPlan {
            action: PlaybackAction::DirectPlay,
            reason: PlaybackReason::RemoteBandwidth,
        };
    }
    PlaybackPlan::direct()
}

// ---------------------------------------------------------------------
// Subtitle auto-pick (P6)

/// The subtitle-relevant facts of one track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleChoice {
    pub language: Option<String>,
    pub is_forced: bool,
    pub is_default: bool,
    /// Text tracks only — picture subs can't be served as WebVTT.
    pub is_text: bool,
}

/// Pick the subtitle track index to enable by default, or None.
///
/// Rules (Emby/Jellyfin-shaped, simplified):
/// - mode "never" → None
/// - mode "always" → best track in the profile's subtitle
///   language; if none matches, the default track; else first
///   text track
/// - mode "auto" → a *forced* track in the profile's audio
///   language (foreign-dialogue case); otherwise None
///
/// Only text tracks are eligible (picture subs need burn-in,
/// Phase 2).
pub fn pick_subtitle(
    tracks: &[SubtitleChoice],
    mode: &str,
    subtitle_lang: Option<&str>,
    audio_lang: Option<&str>,
) -> Option<usize> {
    let lang_eq = |track: &SubtitleChoice, lang: Option<&str>| -> bool {
        match (track.language.as_deref(), lang) {
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
            _ => false,
        }
    };
    match mode {
        "never" => None,
        "always" => tracks
            .iter()
            .position(|t| t.is_text && lang_eq(t, subtitle_lang))
            .or_else(|| tracks.iter().position(|t| t.is_text && t.is_default))
            .or_else(|| tracks.iter().position(|t| t.is_text)),
        // "auto" and anything unknown
        _ => tracks
            .iter()
            .position(|t| t.is_text && t.is_forced && lang_eq(t, audio_lang)),
    }
}

// ---------------------------------------------------------------------
// Segment precedence (SEG1, D15; floor and heuristic order D17/D18)

/// The lowest confidence an *automatic* segment row may have and
/// still be handed to a player (D17). Heuristic rows are stored at
/// 0.5, so they never pass it. Manual rows are exempt (D18): an
/// admin's edit always wins, whatever its stored confidence.
pub const PLAYBACK_CONFIDENCE_FLOOR: f32 = 0.6;

/// Reduce an item's stored segment rows to what the player gets:
/// **at most one segment per kind**, ordered by start time.
///
/// - A manual row always wins over every automatic source (D15),
///   whatever its stored confidence (D18).
/// - Automatic rows below [`PLAYBACK_CONFIDENCE_FLOOR`] are never
///   served (D17) — a guess is no segment.
/// - Otherwise the automatic row with the highest confidence wins;
///   on a tie, the earlier detector in D17's order (chapters, then
///   fingerprint, then heuristic).
/// - Rows that cannot be a span (`end_ms <= start_ms`, negative
///   start) are ignored rather than handed to a player.
pub fn effective_segments(rows: &[MediaSegment]) -> Vec<MediaSegment> {
    // Lower rank = preferred.
    fn source_rank(seg: &MediaSegment) -> u8 {
        match seg.source {
            parlour_core::SegmentSource::Manual => 0,
            parlour_core::SegmentSource::Chapters => 1,
            parlour_core::SegmentSource::Fingerprint => 2,
            parlour_core::SegmentSource::Heuristic => 3,
        }
    }
    fn servable(seg: &MediaSegment) -> bool {
        seg.start_ms >= 0
            && seg.end_ms > seg.start_ms
            && (seg.source.is_manual() || seg.confidence >= PLAYBACK_CONFIDENCE_FLOOR)
    }
    fn better(a: &MediaSegment, b: &MediaSegment) -> bool {
        match (a.source.is_manual(), b.source.is_manual()) {
            (true, false) => return true,
            (false, true) => return false,
            _ => {}
        }
        if a.confidence != b.confidence {
            return a.confidence > b.confidence;
        }
        source_rank(a) < source_rank(b)
    }

    let mut best: Vec<MediaSegment> = Vec::new();
    for row in rows.iter().filter(|r| servable(r)) {
        match best.iter_mut().find(|b| b.kind == row.kind) {
            Some(current) if better(row, current) => *current = *row,
            Some(_) => {}
            None => best.push(*row),
        }
    }
    best.sort_by_key(|s| (s.start_ms, s.kind));
    best
}

/// The segment of `kind` that contains `position_ms`, if any —
/// what SEG5's Skip button asks about.
pub fn segment_at(
    segments: &[MediaSegment],
    kind: SegmentKind,
    position_ms: i64,
) -> Option<&MediaSegment> {
    segments
        .iter()
        .find(|s| s.kind == kind && s.start_ms <= position_ms && position_ms < s.end_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(container: &str, video: &str, audio: &str) -> FileFacts {
        FileFacts {
            container: Some(container.into()),
            video_codec: Some(video.into()),
            audio_codec: Some(audio.into()),
            bitrate_kbps: None,
        }
    }

    #[test]
    fn mp4_h264_aac_direct_plays_on_web() {
        let plan = decide(
            &facts("mp4", "h264", "aac"),
            ClientKind::Web,
            NetworkKind::Lan,
            None,
        );
        assert_eq!(plan, PlaybackPlan::direct());
    }

    #[test]
    fn mkv_with_good_codecs_remuxes_for_container() {
        let plan = decide(
            &facts("matroska", "h264", "aac"),
            ClientKind::Web,
            NetworkKind::Lan,
            None,
        );
        assert_eq!(plan.action, PlaybackAction::Remux);
        assert_eq!(plan.reason, PlaybackReason::ContainerUnsupported);
    }

    #[test]
    fn mkv_direct_plays_on_native() {
        let plan = decide(
            &facts("matroska", "h264", "aac"),
            ClientKind::AndroidTv,
            NetworkKind::Lan,
            None,
        );
        assert_eq!(plan.action, PlaybackAction::DirectPlay);
    }

    #[test]
    fn unsupported_codec_remuxes_with_codec_reason() {
        let plan = decide(
            &facts("mp4", "mpeg2video", "aac"),
            ClientKind::Web,
            NetworkKind::Lan,
            None,
        );
        assert_eq!(plan.action, PlaybackAction::Remux);
        assert_eq!(plan.reason, PlaybackReason::CodecUnsupported);
    }

    #[test]
    fn over_remote_cap_is_flagged_not_blocked() {
        let mut f = facts("mp4", "h264", "aac");
        f.bitrate_kbps = Some(20_000);
        let plan = decide(&f, ClientKind::Web, NetworkKind::Public, Some(8_000));
        assert_eq!(plan.action, PlaybackAction::DirectPlay);
        assert_eq!(plan.reason, PlaybackReason::RemoteBandwidth);
        // On LAN the cap does not apply.
        let plan = decide(&f, ClientKind::Web, NetworkKind::Lan, Some(8_000));
        assert_eq!(plan, PlaybackPlan::direct());
    }

    fn track(lang: &str, forced: bool, text: bool) -> SubtitleChoice {
        SubtitleChoice {
            language: Some(lang.into()),
            is_forced: forced,
            is_default: false,
            is_text: text,
        }
    }

    #[test]
    fn subtitle_modes() {
        let tracks = vec![track("en", false, true), track("en", true, true)];
        assert_eq!(
            pick_subtitle(&tracks, "never", Some("en"), Some("en")),
            None
        );
        // auto: the forced en track (index 1)
        assert_eq!(
            pick_subtitle(&tracks, "auto", Some("en"), Some("en")),
            Some(1)
        );
        // always: first en track
        assert_eq!(
            pick_subtitle(&tracks, "always", Some("en"), Some("en")),
            Some(0)
        );
        // picture subs are never picked
        let pics = vec![track("en", true, false)];
        assert_eq!(pick_subtitle(&pics, "auto", Some("en"), Some("en")), None);
    }

    fn seg(
        kind: SegmentKind,
        source: parlour_core::SegmentSource,
        start_ms: i64,
        end_ms: i64,
        confidence: f32,
    ) -> MediaSegment {
        MediaSegment {
            kind,
            start_ms,
            end_ms,
            source,
            confidence,
        }
    }

    #[test]
    fn manual_segment_wins_over_auto() {
        use parlour_core::SegmentSource::*;
        let rows = vec![
            seg(SegmentKind::Intro, Fingerprint, 60_000, 90_000, 0.95),
            seg(SegmentKind::Intro, Manual, 61_000, 89_000, 1.0),
            seg(SegmentKind::Intro, Chapters, 0, 30_000, 1.0),
        ];
        let out = effective_segments(&rows);
        assert_eq!(out, vec![rows[1]]);
        // Even a manual row with a lower stored confidence wins.
        let rows = vec![
            seg(SegmentKind::Credits, Chapters, 1_000_000, 1_100_000, 1.0),
            seg(SegmentKind::Credits, Manual, 1_010_000, 1_100_000, 0.5),
        ];
        assert_eq!(effective_segments(&rows), vec![rows[1]]);
    }

    #[test]
    fn auto_sources_pick_by_confidence_then_detector_order() {
        use parlour_core::SegmentSource::*;
        let rows = vec![
            seg(SegmentKind::Intro, Chapters, 0, 30_000, 0.6),
            seg(SegmentKind::Intro, Fingerprint, 5_000, 35_000, 0.9),
        ];
        assert_eq!(effective_segments(&rows), vec![rows[1]]);
        let tie = vec![
            seg(SegmentKind::Intro, Fingerprint, 5_000, 35_000, 0.9),
            seg(SegmentKind::Intro, Chapters, 0, 30_000, 0.9),
        ];
        assert_eq!(effective_segments(&tie), vec![tie[1]]);
    }

    #[test]
    fn one_per_kind_sorted_by_start_and_invalid_dropped() {
        use parlour_core::SegmentSource::*;
        let rows = vec![
            seg(SegmentKind::Credits, Fingerprint, 1_200_000, 1_300_000, 0.8),
            seg(SegmentKind::Intro, Fingerprint, 90_000, 120_000, 0.8),
            seg(SegmentKind::Recap, Chapters, 0, 45_000, 1.0),
            seg(SegmentKind::Recap, Manual, 50_000, 50_000, 1.0), // empty span
        ];
        let out = effective_segments(&rows);
        let kinds: Vec<_> = out.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            vec![SegmentKind::Recap, SegmentKind::Intro, SegmentKind::Credits]
        );
        assert_eq!(out[0].source, Chapters, "invalid manual row is ignored");
        assert!(effective_segments(&[]).is_empty());
    }

    #[test]
    fn automatic_rows_below_the_floor_never_reach_the_player() {
        use parlour_core::SegmentSource::*;
        assert_eq!(PLAYBACK_CONFIDENCE_FLOOR, 0.6);
        // A heuristic guess (stored at 0.5) is no segment at all.
        let rows = vec![
            seg(SegmentKind::Intro, Heuristic, 11_000, 31_000, 0.5),
            seg(SegmentKind::Credits, Heuristic, 51_000, 70_000, 0.5),
        ];
        assert!(effective_segments(&rows).is_empty());
        // The floor holds for every automatic source, and is inclusive.
        let rows = vec![
            seg(SegmentKind::Intro, Fingerprint, 0, 30_000, 0.59),
            seg(SegmentKind::Credits, Chapters, 60_000, 90_000, 0.6),
        ];
        assert_eq!(effective_segments(&rows), vec![rows[1]]);
        // A below-floor row never displaces a servable one, even
        // when it would otherwise win on confidence or order.
        let rows = vec![
            seg(SegmentKind::Intro, Chapters, 1_000, 30_000, 0.4),
            seg(SegmentKind::Intro, Fingerprint, 2_000, 31_000, 0.7),
        ];
        assert_eq!(effective_segments(&rows), vec![rows[1]]);
    }

    #[test]
    fn manual_rows_are_exempt_from_the_floor() {
        use parlour_core::SegmentSource::*;
        let rows = vec![
            seg(SegmentKind::Intro, Heuristic, 11_000, 31_000, 0.5),
            seg(SegmentKind::Intro, Manual, 12_000, 30_000, 0.1),
        ];
        assert_eq!(effective_segments(&rows), vec![rows[1]]);
    }

    #[test]
    fn heuristic_is_last_in_the_detector_order() {
        use parlour_core::SegmentSource::*;
        // Only reachable if the floor ever drops below a heuristic
        // row's confidence; the order is D17's either way.
        let tie = vec![
            seg(SegmentKind::Intro, Heuristic, 0, 30_000, 0.8),
            seg(SegmentKind::Intro, Fingerprint, 5_000, 35_000, 0.8),
        ];
        assert_eq!(effective_segments(&tie), vec![tie[1]]);
        let tie = vec![
            seg(SegmentKind::Credits, Heuristic, 60_000, 90_000, 0.9),
            seg(SegmentKind::Credits, Chapters, 61_000, 90_000, 0.9),
        ];
        assert_eq!(effective_segments(&tie), vec![tie[1]]);
        // Higher confidence still beats detector order.
        let rows = vec![
            seg(SegmentKind::Intro, Chapters, 0, 30_000, 0.7),
            seg(SegmentKind::Intro, Heuristic, 1_000, 29_000, 0.9),
        ];
        assert_eq!(effective_segments(&rows), vec![rows[1]]);
    }

    #[test]
    fn segment_at_is_half_open() {
        use parlour_core::SegmentSource::*;
        let segs = vec![seg(SegmentKind::Intro, Manual, 10_000, 20_000, 1.0)];
        assert!(segment_at(&segs, SegmentKind::Intro, 9_999).is_none());
        assert!(segment_at(&segs, SegmentKind::Intro, 10_000).is_some());
        assert!(segment_at(&segs, SegmentKind::Intro, 19_999).is_some());
        assert!(segment_at(&segs, SegmentKind::Intro, 20_000).is_none());
        assert!(segment_at(&segs, SegmentKind::Credits, 15_000).is_none());
    }
}
