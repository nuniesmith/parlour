//! Parlour transcoding (placeholder).
//!
//! FFmpeg/ffprobe run as **subprocesses**. Parlour does not build a
//! transcoding engine and does not bind libav*.
//!
//! Every hardware backend implements [`HwBackend`]. "Supported"
//! means a real 1-second test encode succeeded at startup — a
//! present driver alone does not count. Probe results are cached
//! and re-probed when FFmpeg or the driver changes.
//!
//! Planned preference order: NVENC -> QSV -> VAAPI -> software,
//! overridable in settings. The server owner's main GPU is NVIDIA.

/// A hardware (or software) encoding backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Nvenc,
    QuickSync,
    Vaapi,
    Amf,
    VideoToolbox,
    Software,
}

impl BackendKind {
    /// Stable short name used in logs, settings, and the UI.
    pub fn name(&self) -> &'static str {
        match self {
            BackendKind::Nvenc => "nvenc",
            BackendKind::QuickSync => "qsv",
            BackendKind::Vaapi => "vaapi",
            BackendKind::Amf => "amf",
            BackendKind::VideoToolbox => "videotoolbox",
            BackendKind::Software => "software",
        }
    }
}

/// A backend that can build FFmpeg arguments for a transcode plan.
pub trait HwBackend {
    fn kind(&self) -> BackendKind;
    /// Run a real short test encode. Only `true` counts as working.
    fn probe(&self) -> bool;
    /// Build the FFmpeg argument list for a plan (no shell strings).
    fn build_args(&self) -> Vec<String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_names_are_stable() {
        assert_eq!(BackendKind::Nvenc.name(), "nvenc");
        assert_eq!(BackendKind::Software.name(), "software");
    }
}
