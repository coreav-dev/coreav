//! Bounded native capture driver for hosted checks and registered experiments.

#[path = "common/compare.rs"]
mod compare;
#[path = "common/h264_decode.rs"]
mod decode;

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--help") {
        println!(
            "videotoolbox smoke --mode software|hardware|any --output DIR\nvideotoolbox encode --input RAW --width N --height N --rate-num N --rate-den N --frames N --bitrate N --mode software|hardware --output H264 --report JSON\nvideotoolbox decode --input H264 --output RAW --report JSON"
        );
        println!(
            "videotoolbox compare --source RAW --decoded RAW --width N --height N --frames N --report JSON"
        );
        return;
    }
    let result = if std::env::args().nth(1).as_deref() == Some("decode") {
        decode::run()
    } else if std::env::args().nth(1).as_deref() == Some("compare") {
        compare::run()
    } else {
        #[cfg(target_os = "macos")]
        {
            driver::run()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("macOS is required for encoding".into())
        }
    };
    if let Err(error) = result {
        eprintln!("videotoolbox: {error}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "macos")]
mod driver {
    use coreav_backend_apple::h264::Sync;
    use coreav_backend_apple::videotoolbox::{
        Availability, Completion, Control, Encoder, Error, Frame, Hardware, HardwarePolicy,
        Identity, Limits, Packet, Settings, Time,
    };
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use std::fs::File;
    use std::io::{Cursor, Read, Write};
    use std::path::Path;
    use std::time::{Duration, Instant};

    type Result<T> = std::result::Result<T, String>;

    fn native(error: Error) -> String {
        error.to_string()
    }

    fn create(path: &Path) -> Result<File> {
        File::options()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|_| "cannot create a new output file".into())
    }

    fn document(path: &Path, value: &Value) -> Result<()> {
        let mut file = create(path)?;
        serde_json::to_writer(&mut file, value).map_err(|_| "cannot write report".to_string())?;
        file.write_all(b"\n")
            .map_err(|_| "cannot finish report".into())
    }

    fn implementation(identity: &Identity) -> (&Option<String>, &'static str) {
        (
            &identity.encoder_id,
            match identity.hardware {
                Hardware::Hardware => "hardware",
                Hardware::Software => "software",
                Hardware::Unknown => "unknown",
            },
        )
    }

    fn write_nal(file: &mut File, nal: &[u8]) -> Result<()> {
        file.write_all(&[0, 0, 0, 1])
            .and_then(|()| file.write_all(nal))
            .map_err(|_| "cannot write elementary output".into())
    }

    fn packet(file: &mut File, packet: Packet, records: &mut Vec<Value>) -> Result<()> {
        if packet.frame_id == 0 {
            write_nal(file, packet.config.sps())?;
            write_nal(file, packet.config.pps())?;
        }
        let mut bytes = packet.unit.as_bytes();
        while !bytes.is_empty() {
            let length: [u8; 4] = bytes
                .get(..4)
                .ok_or("invalid NAL length")?
                .try_into()
                .map_err(|_| "invalid NAL length")?;
            let end = usize::try_from(u32::from_be_bytes(length))
                .map_err(|_| "invalid NAL length")?
                .checked_add(4)
                .ok_or("invalid NAL length")?;
            write_nal(file, bytes.get(4..end).ok_or("invalid NAL extent")?)?;
            bytes = bytes.get(end..).ok_or("invalid NAL extent")?;
        }
        records.push(json!({
            "id": packet.frame_id, "pts_value": packet.pts.value(), "pts_scale": packet.pts.scale(),
            "duration_value": packet.duration.value(), "duration_scale": packet.duration.scale(),
            "sync": packet.unit.sync() == Sync::Idr,
        }));
        Ok(())
    }

    fn encode(
        encoder: &mut Encoder,
        source: &mut impl Read,
        output: &mut File,
        settings: Settings,
        frames: u64,
    ) -> Result<Vec<Value>> {
        let width = settings.dimensions().width();
        let height = settings.dimensions().height();
        let size = usize::try_from(u64::from(width) * u64::from(height) * 3 / 2)
            .map_err(|_| "source dimensions exceed storage")?;
        let mut frame = Vec::new();
        frame
            .try_reserve_exact(size)
            .map_err(|_| "cannot reserve source frame")?;
        frame.resize(size, 0);
        let mut records = Vec::new();
        records
            .try_reserve_exact(usize::try_from(frames).map_err(|_| "frame count is too large")?)
            .map_err(|_| "cannot reserve frame records")?;
        let (num, den) = settings.rate();
        let duration = Time::new(i64::from(den), num as i32).map_err(native)?;
        for id in 0..frames {
            source
                .read_exact(&mut frame)
                .map_err(|_| "incomplete raw source frame")?;
            let pts = id
                .checked_mul(u64::from(den))
                .and_then(|n| i64::try_from(n).ok())
                .ok_or("source clock overflow")?;
            let pts = Time::new(pts, num as i32).map_err(native)?;
            let input = Frame::bt709_limited(&frame, width, height).map_err(native)?;
            loop {
                match encoder.submit(input, pts, duration) {
                    Ok(()) => break,
                    Err(Error::Backpressure) => {
                        let ready = encoder
                            .pull()
                            .map_err(native)?
                            .ok_or("backpressure without pending output")?;
                        packet(output, ready, &mut records)?;
                    }
                    Err(error) => return Err(native(error)),
                }
            }
        }
        if source
            .read(&mut [0; 1])
            .map_err(|_| "cannot check source end")?
            != 0
        {
            return Err("unexpected trailing source bytes".into());
        }
        encoder.start_drain().map_err(native)?;
        while let Some(ready) = encoder.pull().map_err(native)? {
            packet(output, ready, &mut records)?;
        }
        output
            .flush()
            .map_err(|_| "cannot flush elementary output")?;
        Ok(records)
    }

    fn report(settings: Settings, mode: &str, frames: Vec<Value>, done: &Completion) -> Value {
        let (encoder_id, hardware) = implementation(&done.identity);
        let (fps_num, fps_den) = settings.rate();
        json!({
            "schema": "coreav.videotoolbox-encode/1", "status": "encoded", "requested_mode": mode,
            "width": settings.dimensions().width(), "height": settings.dimensions().height(),
            "fps_num": fps_num, "fps_den": fps_den, "bitrate": settings.bitrate(),
            "submitted": done.frames, "access_unit_bytes": done.bytes, "encoder_id": encoder_id,
            "hardware": hardware, "frames": frames, "avcc": done.config.as_avcc(),
            "settings": {"profile": "high", "reordering": false, "realtime": false,
                "expected_frame_rate_hint": settings.expected_frame_rate_hint(),
                "keyframe_seconds": 2, "keyframe_frames": 2 * u64::from(fps_num) / u64::from(fps_den),
                "pixel_format": "i420", "native_pixel_format": "420v", "color_range": "limited",
                "primaries": "bt709", "transfer": "bt709", "matrix": "bt709"},
            "drain_complete": true,
        })
    }

    fn smoke(directory: &Path, mode: &str, hardware: HardwarePolicy) -> Result<()> {
        if !directory.is_dir()
            || directory
                .read_dir()
                .map_err(|_| "cannot inspect output directory")?
                .next()
                .is_some()
        {
            return Err("smoke output directory must already exist and be empty".into());
        }
        let settings =
            Settings::bt709_limited(64, 64, 30, 1, 2_000_000, hardware).map_err(native)?;
        let mut manifest = json!({
            "schema": "coreav.videotoolbox-smoke/1", "status": "unavailable", "requested_mode": mode,
            "unavailable_reason": null, "width": 64, "height": 64, "fps_num": 30, "fps_den": 1,
            "bitrate": 2_000_000, "submitted": 0, "encoder_id": null, "hardware": "unknown",
            "frames": [], "elementary_file": null, "source_file": null,
        });
        let control = Control::new(Instant::now() + Duration::from_secs(60));
        let fields = manifest.as_object_mut().ok_or("invalid smoke manifest")?;
        let mut encoder = match Encoder::new(settings, Limits::default(), control) {
            Ok(encoder) => encoder,
            Err(Error::Unavailable(reason)) => {
                fields.insert(
                    "unavailable_reason".into(),
                    json!(match reason {
                        Availability::NoEncoder => "no_encoder",
                        Availability::TemporarilyUnavailable => "temporarily_unavailable",
                    }),
                );
                return document(&directory.join("manifest.json"), &manifest);
            }
            Err(error) => return Err(native(error)),
        };
        let mut source = Vec::with_capacity(24_576);
        for luma in [37, 91, 145, 199] {
            source.extend(std::iter::repeat_n(luma, 4096));
            source.extend(std::iter::repeat_n(128, 2048));
        }
        create(&directory.join("source.yuv"))?
            .write_all(&source)
            .map_err(|_| "cannot write smoke source")?;
        let mut output = create(&directory.join("video.h264"))?;
        let frames = encode(
            &mut encoder,
            &mut Cursor::new(source),
            &mut output,
            settings,
            4,
        )?;
        let done = encoder.complete().map_err(native)?;
        let (encoder_id, hardware) = implementation(&done.identity);
        fields.insert("status".into(), json!("encoded"));
        fields.insert("frames".into(), json!(frames));
        fields.insert("submitted".into(), json!(done.frames));
        fields.insert("hardware".into(), json!(hardware));
        fields.insert("encoder_id".into(), json!(encoder_id));
        fields.insert("elementary_file".into(), json!("video.h264"));
        fields.insert("source_file".into(), json!("source.yuv"));
        document(&directory.join("manifest.json"), &manifest)
    }

    pub(super) fn run() -> Result<()> {
        let mut args = std::env::args().skip(1);
        let command = args.next().ok_or("expected smoke or encode")?;
        let mut options = BTreeMap::new();
        while let Some(key) = args.next() {
            let value = args.next().ok_or("option requires a value")?;
            if options.insert(key, value).is_some() {
                return Err("repeated option".into());
            }
        }
        let mode = options
            .remove("--mode")
            .ok_or("missing implementation mode")?;
        let hardware = match mode.as_str() {
            "hardware" => HardwarePolicy::Require,
            "software" => HardwarePolicy::Disable,
            "any" if command == "smoke" => HardwarePolicy::Any,
            _ => return Err("unsupported implementation mode".into()),
        };
        let output = options.remove("--output").ok_or("missing output")?;
        if command == "smoke" {
            if !options.is_empty() {
                return Err("unknown smoke option".into());
            }
            return smoke(Path::new(&output), &mode, hardware);
        }
        if command != "encode" {
            return Err("expected smoke or encode".into());
        }
        let input = options.remove("--input").ok_or("missing raw source")?;
        let report_path = options.remove("--report").ok_or("missing report")?;
        let mut number = |name: &str| -> Result<u32> {
            options
                .remove(name)
                .ok_or_else(|| format!("missing {name}"))?
                .parse()
                .map_err(|_| "invalid numeric option".into())
        };
        let settings = Settings::bt709_limited(
            number("--width")?,
            number("--height")?,
            number("--rate-num")?,
            number("--rate-den")?,
            number("--bitrate")?,
            hardware,
        )
        .map_err(native)?;
        let frames = u64::from(number("--frames")?);
        if frames == 0 || frames > 36_000 || !options.is_empty() {
            return Err("unsupported frame count or unknown encode option".into());
        }
        let mut source = File::open(input).map_err(|_| "cannot open raw source")?;
        let expected = u64::from(settings.dimensions().width())
            * u64::from(settings.dimensions().height())
            * 3
            / 2;
        if source
            .metadata()
            .map_err(|_| "cannot inspect raw source")?
            .len()
            != expected
                .checked_mul(frames)
                .ok_or("raw source size overflow")?
        {
            return Err("raw source size does not match the declared frames".into());
        }
        let control = Control::new(Instant::now() + Duration::from_secs(900));
        let mut encoder = Encoder::new(settings, Limits::default(), control).map_err(native)?;
        let mut output = create(Path::new(&output))?;
        let frames = encode(&mut encoder, &mut source, &mut output, settings, frames)?;
        let done = encoder.complete().map_err(native)?;
        document(
            Path::new(&report_path),
            &report(settings, &mode, frames, &done),
        )
    }

    #[cfg(test)]
    mod tests {
        #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
        use super::*;
        use std::path::PathBuf;

        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        #[test]
        fn native_fractional_clock_and_cropped_geometry_preserve_presented_frames() {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "coreav-videotoolbox-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap();
            let scratch = Scratch(path);
            let settings =
                Settings::bt709_limited(32, 30, 30_000, 1001, 2_000_000, HardwarePolicy::Require)
                    .unwrap();
            let mut encoder = Encoder::new(
                settings,
                Limits::default(),
                Control::new(Instant::now() + Duration::from_secs(60)),
            )
            .unwrap();
            let mut source = Vec::new();
            for level in [37, 91, 145, 199] {
                source.extend(std::iter::repeat_n(level, 960));
                source.extend(std::iter::repeat_n(128, 480));
            }
            let path = scratch.0.join("video.h264");
            let mut file = create(&path).unwrap();
            let records = encode(
                &mut encoder,
                &mut Cursor::new(source),
                &mut file,
                settings,
                4,
            )
            .unwrap();
            let done = encoder.complete().unwrap();
            assert_eq!(done.frames, 4);
            assert_eq!(records.len(), 4);
            for (id, record) in records.iter().enumerate() {
                assert_eq!(record["id"], json!(id));
                assert_eq!(record["pts_value"], json!(id * 1001));
                assert_eq!(record["pts_scale"], json!(30_000));
                assert_eq!(record["duration_value"], json!(1001));
                assert_eq!(record["duration_scale"], json!(30_000));
            }
            let evidence = report(settings, "hardware", records, &done);
            assert_eq!(evidence["fps_num"], json!(30_000));
            assert_eq!(evidence["fps_den"], json!(1001));
            assert_eq!(evidence["settings"]["expected_frame_rate_hint"], json!(30));
            assert_eq!(evidence["settings"]["keyframe_frames"], json!(59));
            assert_eq!(done.config.dimensions().width(), 32);
            assert_eq!(done.config.dimensions().height(), 30);
            drop(file);
            let mut pixels = Vec::new();
            assert_eq!(
                crate::decode::decode(&mut File::open(path).unwrap(), &mut pixels).unwrap(),
                (32, 30, 4)
            );
            assert_eq!(pixels.len(), 5760);
            for (frame, level) in pixels.chunks_exact(1440).zip([37, 91, 145, 199]) {
                assert!(
                    frame
                        .get(..960)
                        .unwrap()
                        .iter()
                        .all(|&pixel| pixel.abs_diff(level) <= 8)
                );
            }
        }
    }
}
