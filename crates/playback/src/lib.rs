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
//! Also here: the subtitle auto-pick rules (P6), pure so the
//! server and future clients agree on them.

use parlour_core::{ClientKind, NetworkKind, PlaybackAction, PlaybackPlan, PlaybackReason};

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
}
