use super::*;
use coreav_decode::{DecoderInfo, Error, ErrorKind, Isolation, StreamTraits, Support};
use coreav_thumbnail::{ImageFormat, VideoStream};
use std::sync::{Arc, Mutex};

struct Tap {
    frames: Arc<Mutex<Vec<Value>>>,
}
struct Tapped {
    decoder: Box<dyn Decoder>,
    frames: Arc<Mutex<Vec<Value>>>,
}
fn failure() -> Error {
    Error::new(ErrorKind::Backend, "C5 receipt failure")
}

fn unsafe_start(error: &Error) -> bool {
    error.kind == ErrorKind::InvalidData
        && matches!(
            error.message.as_str(),
            "invalid data: missing_pps"
                | "invalid data: missing_sps"
                | "invalid data: missing_vps"
                | "invalid data: missing initial IRAP"
                | "invalid data: missing associated IRAP"
                | "invalid data: unavailable trailing reference"
                | "invalid data: unavailable RADL reference"
                | "invalid data: unavailable reference samples"
        )
}

impl Backend for Tap {
    fn kind(&self) -> BackendKind {
        BackendKind::Native
    }
    fn supports(&self, stream: &StreamConfig, traits: &StreamTraits) -> Support {
        NativeHevc::new().supports(stream, traits)
    }
    fn open(
        &self,
        stream: &StreamConfig,
        limits: &Limits,
    ) -> std::result::Result<Box<dyn Decoder>, Error> {
        Ok(Box::new(Tapped {
            decoder: NativeHevc::new().open(stream, limits)?,
            frames: Arc::clone(&self.frames),
        }))
    }
}

impl Decoder for Tapped {
    fn send_packet(&mut self, packet: &Packet<'_>) -> std::result::Result<(), Error> {
        self.decoder.send_packet(packet)
    }
    fn send_eof(&mut self) -> std::result::Result<(), Error> {
        self.decoder.send_eof()
    }
    fn receive_frame(&mut self) -> std::result::Result<Option<Frame>, Error> {
        let frame = self.decoder.receive_frame()?;
        if let Some(frame) = &frame {
            self.frames
                .lock()
                .map_err(|_| failure())?
                .push(frame_record(frame).map_err(|_| failure())?.0);
        }
        Ok(frame)
    }
    fn info(&self) -> &DecoderInfo {
        self.decoder.info()
    }
}

struct Reference(Mutex<Option<Frame>>);
struct ReferenceDecoder {
    frame: Option<Frame>,
    eof: bool,
    info: DecoderInfo,
}

impl Backend for Reference {
    fn kind(&self) -> BackendKind {
        BackendKind::Native
    }
    fn supports(&self, _: &StreamConfig, _: &StreamTraits) -> Support {
        Support::Yes
    }
    fn open(&self, _: &StreamConfig, _: &Limits) -> std::result::Result<Box<dyn Decoder>, Error> {
        Ok(Box::new(ReferenceDecoder {
            frame: self.0.lock().map_err(|_| failure())?.take(),
            eof: false,
            info: DecoderInfo {
                backend: BackendKind::Native,
                isolation: Isolation::InProcess,
                decoder: "qualified full native HEVC frame".into(),
            },
        }))
    }
}

impl Decoder for ReferenceDecoder {
    fn send_packet(&mut self, _: &Packet<'_>) -> std::result::Result<(), Error> {
        Ok(())
    }
    fn send_eof(&mut self) -> std::result::Result<(), Error> {
        self.eof = true;
        Ok(())
    }
    fn receive_frame(&mut self) -> std::result::Result<Option<Frame>, Error> {
        Ok(if self.eof { self.frame.take() } else { None })
    }
    fn info(&self) -> &DecoderInfo {
        &self.info
    }
}

pub(super) fn check(
    path: &std::ffi::OsStr,
    source: &mut FileSource,
    stream: &VideoStream,
    limits: &Limits,
    baselines: Vec<(usize, Frame)>,
    sent: usize,
) -> Result<Value> {
    let candidates = baselines.len();
    let (mut matched, mut refused) = (0usize, 0usize);
    let mut records = Vec::new();
    let mut duplicate_checked = false;
    for (index, mut frame) in baselines {
        let packet = stream.packets.get(index).ok_or("C5 packet index")?;
        let mut options =
            ThumbnailOptions::at(packet.time.ok_or("C5 presentation time is absent")?);
        options.stream = Some(stream.stream_index);
        options.width = Some(64);
        options.format = ImageFormat::Png;
        options.decode_limits = limits.clone();
        let choice = coreav_thumbnail::oracle_choice(&mut FileSource::open(path)?, &options)?;
        if choice.start != packet.offset || choice.target != packet.offset {
            return Err("C5 selector chose a different source packet".into());
        }
        let captured = Arc::new(Mutex::new(Vec::new()));
        let tap = Tap {
            frames: Arc::clone(&captured),
        };
        let native = match coreav_thumbnail::thumbnail(FileSource::open(path)?, &options, &[&tap]) {
            Ok(thumbnail) => thumbnail,
            Err(coreav_thumbnail::Error::Decode(error)) if unsafe_start(&error) => {
                refused += 1;
                records.push(json!({"packet":index,"status":"unsafe_start_refused","category":error.kind.name()}));
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        frame.pts = Some(packet.pts);
        let (expected, _) = frame_record(&frame)?;
        let observed = captured.lock().map_err(|_| "C5 capture lock")?;
        if observed.as_slice() != std::slice::from_ref(&expected) {
            return Err("C5 random-access frame differs from full decode".into());
        }
        drop(observed);
        if !duplicate_checked && index + 1 < sent {
            let first = stream.read_packet(source, index, 64 << 20)?;
            let second = stream.read_packet(source, index + 1, 64 << 20)?;
            let mut decoder = NativeHevc::new().open(&stream.stream, limits)?;
            let mut found = Vec::new();
            for (data, discard) in [(&first, false), (&second, true)] {
                decoder.send_packet(&Packet {
                    data,
                    pts: Some(7),
                    dts: None,
                    keyframe: true,
                    discard,
                })?;
                while let Some(frame) = decoder.receive_frame()? {
                    found.push(frame_record(&frame)?.0);
                }
            }
            decoder.send_eof()?;
            while let Some(frame) = decoder.receive_frame()? {
                found.push(frame_record(&frame)?.0);
            }
            let mut expected_duplicate = expected.clone();
            *expected_duplicate
                .get_mut("timestamp")
                .ok_or("C5 frame record has no timestamp")? = json!(7);
            if found != [expected_duplicate] {
                return Err("C5 duplicate-PTS discard association differs".into());
            }
            duplicate_checked = true;
        }
        let reference = Reference(Mutex::new(Some(frame)));
        let expected_image =
            coreav_thumbnail::thumbnail(FileSource::open(path)?, &options, &[&reference])?;
        if native.image != expected_image.image
            || native.packet != (packet.offset, packet.size)
            || native.packet != expected_image.packet
            || native.time != expected_image.time
            || !native.keyframe
            || native.decoder.backend != BackendKind::Native
            || native.packets_decoded != 1
        {
            return Err("C5 keyframe thumbnail differs from full decode".into());
        }
        matched += 1;
        let frame_md5 = expected
            .get("output_md5")
            .ok_or("C5 frame record has no MD5")?;
        records.push(
            json!({"packet":index,"status":"matched","frame_md5":frame_md5,
            "image_md5":format!("{:x}",Md5::digest(&native.image)),"pts":packet.pts}),
        );
    }
    let mut dishonest_sync_checked = false;
    let width = stream
        .stream
        .config
        .as_ref()
        .and_then(|config| config.data.get(21))
        .map(|n| (n & 3) + 1);
    if let Some(width) = width {
        for (index, _) in stream.packets.iter().take(sent).enumerate() {
            let data = stream.read_packet(source, index, 64 << 20)?;
            let mut first_vcl = None;
            for nal in coreav_hevc::LengthPrefixed::new(&data, width)? {
                let header = NalHeader::parse(nal?)?;
                if header.unit_type < 32 {
                    first_vcl = Some(header);
                    break;
                }
            }
            let Some(header) = first_vcl else {
                continue;
            };
            if (16..=21).contains(&header.unit_type) {
                continue;
            }
            let mut decoder = NativeHevc::new().open(&stream.stream, limits)?;
            let refused = decoder
                .send_packet(&Packet {
                    data: &data,
                    pts: Some(7),
                    dts: None,
                    keyframe: true,
                    discard: false,
                })
                .and_then(|()| decoder.send_eof());
            if !matches!(
                refused,
                Err(Error {
                    kind: ErrorKind::InvalidData | ErrorKind::Unsupported,
                    ..
                })
            ) || decoder.receive_frame().is_ok()
                || decoder.send_eof().is_ok()
            {
                return Err("C5 dishonest sync flag was not a terminal refusal".into());
            }
            dishonest_sync_checked = true;
            break;
        }
    }
    Ok(
        json!({"complete":matched+refused==candidates,"candidates":candidates,"matched":matched,
        "unsafe_refusals":refused,"duplicate_pts_checked":duplicate_checked,
        "dishonest_sync_checked":dishonest_sync_checked,"results":records,
        "unavailable":if candidates==0 {Some("no unique presented keyframe output in this prefix")} else {None}}),
    )
}
