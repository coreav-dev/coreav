//! Test-only C2 adapter: `vp9-corpus-oracle FILE inspect|first120|all`.
//! Emits the original Matroska clock, decoded frame records and a required
//! completion record. It is not a decode backend and is never distributed.

use std::io::{BufWriter, Write};
use std::path::Path;
use std::process::ExitCode;

use coreav_mkv::{
    Allowlist, BufferedSource, Codec, Encoding, FileSource, Profile, Source, TrackKind,
};
use coreav_vp9::{Decoder, FrameDecoder, Header, Picture, Samples};
use md5::{Digest, Md5};
use serde_json::{Value, json};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Inspect,
    First120,
    All,
}

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let mut output = BufWriter::new(std::io::stdout().lock());
    let result = match args.as_slice() {
        [file, scope] => match scope.as_str() {
            "inspect" => measure(Path::new(file), Scope::Inspect, &mut output),
            "first120" => measure(Path::new(file), Scope::First120, &mut output),
            "all" => measure(Path::new(file), Scope::All, &mut output),
            _ => Err("scope must be inspect, first120 or all".into()),
        },
        _ => Err("usage: vp9-corpus-oracle FILE inspect|first120|all".into()),
    };
    let ok = result.is_ok();
    if let Err(error) = result {
        let _ = emit(&mut output, json!({"kind": "error", "message": error}));
    }
    if output.flush().is_err() || !ok {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

fn emit(output: &mut impl Write, row: Value) -> Result<(), String> {
    writeln!(output, "{row}").map_err(|e| e.to_string())
}

fn measure(path: &Path, scope: Scope, output: &mut impl Write) -> Result<(), String> {
    let source = FileSource::open(path).map_err(|e| e.to_string())?;
    let mut source = BufferedSource::new(source);
    let container_limits = coreav_mkv::Limits::default();
    let segment = coreav_mkv::probe(&mut source, &container_limits, &Allowlist::any())
        .map_err(|e| e.to_string())?;
    let track = segment
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Video)
        .ok_or("no video track")?;
    if track.codec != Codec::Vp9 || !track.timestamp_scale_one {
        return Err("the first video track must be VP9 with unit track timestamp scale".into());
    }
    let prefix = match &track.encoding {
        Encoding::None => &[][..],
        Encoding::HeaderStripping(bytes) => bytes.as_slice(),
        _ => return Err("unsupported packet encoding".into()),
    };
    let bit_depth = match track.profile {
        Some(Profile::Vp9 { bit_depth, .. }) => Some(bit_depth),
        _ => None,
    };
    emit(
        output,
        json!({
            "kind": "stream", "stream_index": track.stream_index,
            "profile": track.frame_profile, "bit_depth": bit_depth,
            "subsampling": track.chroma_subsampling,
            "time_base": [segment.timestamp_scale, 1_000_000_000u64],
        }),
    )?;
    if scope == Scope::Inspect {
        return emit(output, json!({"kind": "complete", "scope": "inspect"}));
    }
    if track.frame_profile != Some(0)
        || bit_depth != Some(8)
        || track.chroma_subsampling != Some((1, 1))
    {
        return Err("pixel comparison requires profile 0, 8-bit 4:2:0".into());
    }
    let limits = coreav_vp9::Limits::default();
    let mut decoder = FrameDecoder::new(limits);
    let mut inspection = Decoder::new(limits);
    let mut coverage = Coverage::default();
    let mut packets = segment.packets(&mut source, &container_limits);
    let mut window = Window::new(scope);
    let mut frames = 0u64;
    while let Some(packet) = packets.next() {
        let packet = packet.map_err(|e| e.to_string())?;
        if packet.track != track.index {
            continue;
        }
        let pts = packet.pts.ok_or("unknown packet timestamp")?;
        let Some(fed) = window.packet(pts)? else {
            continue;
        };
        let size = usize::try_from(packet.stored_size).map_err(|_| "packet size overflow")?;
        if size
            .checked_add(prefix.len())
            .is_none_or(|n| n > limits.max_packet_bytes)
        {
            return Err("packet byte limit".into());
        }
        let mut bytes = prefix.to_vec();
        packets
            .source_mut()
            .read_append(packet.offset, size, &mut bytes)
            .map_err(|e| e.to_string())?;
        for header in inspection.read_headers(&bytes).map_err(|e| e.to_string())? {
            if let Header::Frame { header, .. } = header {
                coverage.observe(&header);
            }
        }
        for picture in decoder.decode(&bytes).map_err(|e| e.to_string())? {
            let (size, md5) = packed(&picture)?;
            emit(
                output,
                json!({
                    "kind": "frame", "packet": fed, "frame": frames, "pts": pts,
                    "width": picture.width, "height": picture.height,
                    "size": size, "md5": md5,
                }),
            )?;
            frames += 1;
        }
    }
    emit(
        output,
        json!({
            "kind": "complete", "scope": if scope == Scope::All { "all" } else { "first120" },
            "packets": window.selected, "fed": window.fed, "frames": frames, "cutoff": window.cutoff,
            "coverage": {
                "coded": coverage.coded, "sharpness": coverage.sharpness,
                "segment_filter": coverage.segment_filter,
            },
        }),
    )
}

struct Window {
    scope: Scope,
    selected: u64,
    fed: u64,
    cutoff: Option<i64>,
}

impl Window {
    fn new(scope: Scope) -> Self {
        Self {
            scope,
            selected: 0,
            fed: 0,
            cutoff: None,
        }
    }

    fn packet(&mut self, pts: i64) -> Result<Option<u64>, String> {
        if self.selected >= 1_000_000 {
            return Err("packet metadata limit".into());
        }
        self.selected += 1;
        if self.scope == Scope::First120 && self.fed >= 120 {
            self.cutoff = Some(self.cutoff.map_or(pts, |old| old.min(pts)));
            return Ok(None);
        }
        let index = self.fed;
        self.fed += 1;
        Ok(Some(index))
    }
}

#[derive(Default)]
struct Coverage {
    coded: u64,
    sharpness: u64,
    segment_filter: u64,
}

impl Coverage {
    fn observe(&mut self, header: &coreav_vp9::FrameHeader) {
        self.coded += 1;
        self.sharpness += u64::from(header.loop_filter.sharpness != 0);
        self.segment_filter += u64::from(
            header.segmentation.enabled
                && header
                    .segmentation
                    .features
                    .iter()
                    .any(|f| f.get(1).is_some_and(Option::is_some)),
        );
    }
}

fn packed(picture: &Picture) -> Result<(u64, String), String> {
    if picture.bit_depth != 8 || !picture.subsampling_x || !picture.subsampling_y {
        return Err("picture is not 8-bit 4:2:0".into());
    }
    let mut hash = Md5::new();
    let mut size = 0;
    for (i, plane) in picture.planes.iter().enumerate() {
        let Samples::Eight(samples) = plane else {
            return Err("picture sample storage disagrees with bit depth".into());
        };
        let (width, height) = if i == 0 {
            (u64::from(picture.width), u64::from(picture.height))
        } else {
            (
                u64::from(picture.width).div_ceil(2),
                u64::from(picture.height).div_ceil(2),
            )
        };
        if width == 0 || height == 0 || samples.len() as u64 != width * height {
            return Err("picture plane size disagrees with its dimensions".into());
        }
        size += samples.len() as u64;
        hash.update(samples);
    }
    Ok((size, format!("{:x}", hash.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cutoff_includes_every_unsent_packet() -> Result<(), String> {
        let mut window = Window::new(Scope::First120);
        for pts in 0..120 {
            assert_eq!(window.packet(pts)?, Some(pts as u64));
        }
        for _ in 0..70 {
            assert_eq!(window.packet(1000)?, None);
        }
        assert_eq!(window.packet(-3)?, None);
        assert_eq!(
            (window.fed, window.selected, window.cutoff),
            (120, 191, Some(-3))
        );
        let mut whole = Window::new(Scope::All);
        for pts in 0..191 {
            assert_eq!(whole.packet(pts)?, Some(pts as u64));
        }
        assert_eq!((whole.fed, whole.selected, whole.cutoff), (191, 191, None));
        Ok(())
    }

    #[test]
    fn odd_sized_planes_are_hashed_in_visible_order() -> Result<(), String> {
        let picture = Picture {
            width: 3,
            height: 3,
            bit_depth: 8,
            subsampling_x: true,
            subsampling_y: true,
            planes: [
                Samples::Eight(vec![1; 9]),
                Samples::Eight(vec![2; 4]),
                Samples::Eight(vec![3; 4]),
            ],
        };
        let bytes = [vec![1; 9], vec![2; 4], vec![3; 4]].concat();
        assert_eq!(packed(&picture)?, (17, format!("{:x}", Md5::digest(bytes))));
        let mut wrong = picture.clone();
        wrong.planes = core::array::from_fn(|_| Samples::Eight(vec![0; 9]));
        assert!(packed(&wrong).is_err());
        wrong = picture.clone();
        wrong.bit_depth = 10;
        assert!(packed(&wrong).is_err());
        wrong = picture;
        wrong.planes = core::array::from_fn(|_| Samples::High(vec![0; 9]));
        assert!(packed(&wrong).is_err());
        Ok(())
    }
}
