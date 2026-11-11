//! Parlour playback (placeholder).
//!
//! The heart is a **pure decision function**: given the file's probe
//! data, the client's capability profile, the network the request
//! arrived on, and the chosen subtitle track, decide DirectPlay,
//! Remux, or Transcode — with a reason the UI can show.
//!
//! Sessions, HLS serving, and per-profile watch state also live
//! here when implemented.

use parlour_core::PlaybackPlan;

/// Placeholder decision: direct play everything.
/// The real engine lands in Phase 2 with the transcode crate.
pub fn decide() -> PlaybackPlan {
    // TODO(phase-2): take probe data, client profile, network kind,
    // per-user remote cap, and the selected subtitle track as inputs.
    PlaybackPlan::direct()
}

#[cfg(test)]
mod tests {
    use super::*;
    use parlour_core::PlaybackAction;

    #[test]
    fn placeholder_decides_direct_play() {
        assert_eq!(decide().action, PlaybackAction::DirectPlay);
    }
}
