//! The library behind the `coreav` command: probing with the
//! library defaults and the JSON documents the command prints, the native
//! schemas `coreav.probe/1` (ISO base media), `coreav.probe-matroska/1`
//! (Matroska and WebM) and `coreav.probe-mpegts/2` (MPEG transport
//! streams), the `--ffprobe-compat` shape and the
//! `coreav.error/1` object; clips with their `coreav.clip/1`
//! summary; splits (`docs/design/split.md`) with their `coreav.split/1`
//! summary; full-file remuxes (`docs/design/remux.md`) with their
//! `coreav.remux/1` summary; audio extraction with its
//! `coreav.extract-audio/1` summary; subtitles and chapters
//! (`docs/design/text.md`) with `coreav.extract-subtitles/1` and
//! `coreav.chapters/1`; joins (`docs/design/join.md`) with their
//! `coreav.join/1` summary; upload policy checks (`coreav-policy`) with
//! their `coreav.verdict/1` verdict. The binary adds only argument parsing
//! and output. Exposed as a library so that other builds of the command's
//! work, such as the WebAssembly module (`coreav-wasm`), carry the same
//! serializer.

pub mod check;
pub mod clip;
pub mod compat;
pub mod concat;
pub mod decode_audio;
pub mod error;
pub mod extract;
pub mod fallback;
pub mod ffmpeg;
pub mod ffprobe;
pub mod hls;
pub mod join;
pub mod matroska;
pub mod mpegts;
pub mod names;
pub mod native;
pub mod output;
mod pretty;
pub mod probe;
#[cfg(feature = "net")]
pub mod remote;
pub mod remux;
pub mod split;
pub mod strict_safe;
pub mod text;
pub mod thumbnail;
pub mod time;
#[cfg(feature = "transcode")]
pub mod transcode;

#[cfg(all(test, feature = "transcode"))]
#[path = "../../coreav-isobmff/tests/common/mod.rs"]
#[allow(clippy::indexing_slicing)]
mod test_isobmff;

/// Whether an input names a URL rather than a file (`remote::is_url`);
/// always false without the `net` feature.
pub fn remote_url(input: &str) -> bool {
    #[cfg(feature = "net")]
    {
        remote::is_url(input)
    }
    #[cfg(not(feature = "net"))]
    {
        let _ = input;
        false
    }
}
