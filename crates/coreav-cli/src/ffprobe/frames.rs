//! Frames, as `-show_frames` lists them and `-count_frames` counts them,
//! decoded by CoreAV's own decoders (`docs/design/compat-cli.md`,
//! "Frames"): H.264 by the native decoder in a build with `native-h264`,
//! MP3 and FLAC by `coreav-audio`, PCM as it is stored. A stream of any
//! other codec is refused before anything is printed.
//!
//! A frame is printed when `ffprobe` prints it: its decoders return an
//! audio frame as soon as its packet is read, and an H.264 picture once as
//! many later packets are read as the stream reorders (`has_b_frames`),
//! the rest when the input ends (found black box). A frame's `pkt_dts` is
//! the decoding time of the packet read when it is returned, unknown for
//! those returned at the end. CoreAV's decoder may finish a picture later
//! than that, so the printed items wait in one queue until each frame that
//! is due has been decoded.

use std::collections::VecDeque;

use super::model::{Sid, Value, s, time};
use super::{Entries, Packet, Report, SideData, Stream, StreamKind};
use crate::error::Failure;
use coreav_audio::flac::{FlacDecoder, StreamInfo};
use coreav_audio::mp3::Mp3Decoder;
use coreav_audio::{Limits as AudioLimits, Pcm};

/// Items held back behind a frame not decoded yet, at most: far more than
/// a decoded picture buffer and the decoder's threads keep.
const MAX_QUEUED: usize = 4096;

/// What a stream's decoder is given besides its packets, as the container
/// stores it.
#[derive(Clone, Debug, Default)]
pub struct Setup {
    /// The codec configuration: `avcC`, `dfLa`, Matroska's `CodecPrivate`.
    pub config: Option<Vec<u8>>,
    /// The picture width the container declares.
    pub width: Option<u32>,
    /// The picture height the container declares.
    pub height: Option<u32>,
    /// Why the stream's packets cannot be given to a decoder as stored,
    /// if they cannot (Matroska frames compressed or encrypted).
    pub unreadable: Option<&'static str>,
    /// Samples the decoder's output starts with that are not presented
    /// and that no packet's side data names (Matroska's `CodecDelay`).
    pub skip: i64,
}

/// One frame, as printed.
#[derive(Clone, Debug)]
pub struct Row {
    /// The entries, in `ffprobe`'s order.
    pub entries: Entries,
    /// The side data.
    pub side_data: Vec<SideData>,
}

/// What the listing prints next.
#[derive(Debug)]
pub enum Item {
    /// A packet (`-show_packets`).
    Packet(Packet),
    /// A frame.
    Frame(Row),
}

/// What a packet says of the frame it holds.
#[derive(Clone, Debug, Default)]
struct Facts {
    pts: Option<i64>,
    duration: Option<i64>,
    pos: Option<u64>,
    size: u64,
    /// Not presented (an edit list's preroll).
    discard: bool,
    key: bool,
    pict_type: &'static str,
    user_data: bool,
    /// Samples to skip at the start, and to drop at the end.
    skip: i64,
    padding: i64,
}

/// A frame a decoder returned, in output order.
#[derive(Clone, Debug)]
struct Decoded {
    facts: Facts,
    shape: Shape,
}

#[derive(Clone, Copy, Debug)]
enum Shape {
    #[cfg_attr(
        not(feature = "native-h264"),
        expect(dead_code, reason = "only the H.264 decoder returns pictures")
    )]
    Picture {
        width: u32,
        height: u32,
    },
    Samples(u64),
}

/// A decoder, as the listing drives it. An error says why the decoder
/// stopped.
trait FrameDecoder {
    /// Decodes one packet, appending the frames it finished to `out` in
    /// output order.
    fn send(
        &mut self,
        data: &[u8],
        facts: Facts,
        out: &mut VecDeque<Decoded>,
    ) -> Result<(), String>;

    /// The input ended: appends the frames left.
    fn finish(&mut self, out: &mut VecDeque<Decoded>) -> Result<(), String>;

    /// Fills in what the packet's bytes say of its frame.
    fn facts(&mut self, _data: &[u8], facts: &mut Facts) {
        facts.key = true;
    }
}

/// The values every frame of a stream takes from the stream.
enum Fixed {
    Video {
        pix_fmt: Value,
        sample_aspect_ratio: Value,
        colors: [Value; 5],
        side_data: Vec<SideData>,
    },
    Audio {
        sample_fmt: Value,
        channels: Value,
        channel_layout: Value,
        rate: Option<i64>,
    },
}

struct Lane {
    stream: usize,
    decoder: Box<dyn FrameDecoder>,
    fixed: Fixed,
    time_base: (i64, i64),
    /// Packets read before `ffprobe`'s decoder returns the first frame.
    delay: u64,
    sent: u64,
    /// Frames due so far, and frames decoded so far.
    due: u64,
    decoded_count: u64,
    decoded: VecDeque<Decoded>,
    /// Samples still to skip, carried past frames skipped whole.
    carry: i64,
    /// Frames listed.
    listed: u64,
    /// The last presentation and decoding times seen, and how often each
    /// did not increase: `best_effort_timestamp` follows the one that
    /// went back less often (found black box).
    last: [Option<i64>; 2],
    faults: [u64; 2],
    /// What a refusal of the stream's frames starts with.
    refusal: String,
}

enum Slot {
    Ready(Item),
    /// The next frame of a lane, with the decoding time of the packet
    /// read when `ffprobe` returns it.
    Frame {
        lane: usize,
        pkt_dts: Option<i64>,
    },
}

/// The decoders of the selected streams and the items waiting to be
/// printed.
pub struct Frames {
    lanes: Vec<Lane>,
    by_stream: Vec<Option<usize>>,
    queue: VecDeque<Slot>,
    /// Whether frames are printed, not only counted.
    show: bool,
    /// The input ended: a frame never decoded is not waited for.
    ended: bool,
}

impl Frames {
    /// The decoders `options` needs for `report`'s streams, or `None`
    /// when it prints and counts no frame. Refuses, with what names the
    /// option and the stream, a selected stream CoreAV has no decoder for.
    pub fn plan(options: &super::args::Options, report: &Report) -> Result<Option<Self>, Failure> {
        let sel = &options.selection;
        let show = sel.shown(Sid::Frames);
        let counted = |sid: Sid| sel.shown(sid) && sel.entry(sid, "nb_read_frames");
        let count = options.count_frames && (counted(Sid::Stream) || counted(Sid::ProgramStream));
        if !show && !count {
            return Ok(None);
        }
        let option = if show {
            "-show_frames"
        } else {
            "-count_frames"
        };
        let mut lanes = Vec::new();
        let mut by_stream = vec![None; report.streams.len()];
        for (index, stream) in report.streams.iter().enumerate() {
            if !super::selected(options.select_streams, &report.streams, index) {
                continue;
            }
            let codec = text(stream, "codec_name").unwrap_or_else(|| "unknown".to_owned());
            let refusal = format!("{option} needs the frames of stream {index} ({codec})");
            let refuse = |why: &str| Failure::Unsupported(format!("{refusal}: {why}"));
            let setup = report.setups.get(index).cloned().unwrap_or_default();
            if let Some(why) = setup.unreadable {
                return Err(refuse(why));
            }
            let decoder: Box<dyn FrameDecoder> = match stream.kind {
                StreamKind::Data | StreamKind::Attachment => continue,
                StreamKind::Subtitle => return Err(refuse("subtitle frames are not decoded")),
                StreamKind::Video if stream.attached_pic => {
                    return Err(refuse("attached pictures are not decoded"));
                }
                StreamKind::Video => video(&codec, &setup).map_err(|why| refuse(&why))?,
                StreamKind::Audio => audio(&codec, stream, &setup).map_err(|why| refuse(&why))?,
            };
            let fixed = match stream.kind {
                StreamKind::Video => {
                    let [pix_fmt, range, space, primaries, transfer] =
                        picture_colors(stream, &setup);
                    Fixed::Video {
                        pix_fmt,
                        sample_aspect_ratio: value(stream, "sample_aspect_ratio"),
                        colors: [
                            range,
                            space,
                            primaries,
                            transfer,
                            value(stream, "chroma_location"),
                        ],
                        side_data: stream
                            .side_data
                            .iter()
                            .filter(|d| d.typ == "display_matrix")
                            .map(|d| SideData {
                                typ: "3x3_displaymatrix",
                                entries: std::iter::once((
                                    "side_data_type",
                                    s("3x3 displaymatrix"),
                                ))
                                .chain(
                                    d.entries
                                        .iter()
                                        .filter(|(k, _)| *k != "side_data_type")
                                        .cloned(),
                                )
                                .collect(),
                            })
                            .collect(),
                    }
                }
                _ => Fixed::Audio {
                    sample_fmt: value(stream, "sample_fmt"),
                    channels: value(stream, "channels"),
                    channel_layout: value(stream, "channel_layout"),
                    rate: number(stream, "sample_rate"),
                },
            };
            let delay = match stream.kind {
                StreamKind::Video => number(stream, "has_b_frames")
                    .and_then(|d| u64::try_from(d).ok())
                    .unwrap_or(0),
                _ => 0,
            };
            if let Some(slot) = by_stream.get_mut(index) {
                *slot = Some(lanes.len());
            }
            lanes.push(Lane {
                stream: index,
                decoder,
                fixed,
                time_base: report.time_bases.get(index).copied().unwrap_or((1, 1)),
                delay,
                sent: 0,
                due: 0,
                decoded_count: 0,
                decoded: VecDeque::new(),
                carry: setup.skip.max(0),
                listed: 0,
                last: [None; 2],
                faults: [0; 2],
                refusal,
            });
        }
        Ok(Some(Frames {
            lanes,
            by_stream,
            queue: VecDeque::new(),
            show,
            ended: false,
        }))
    }

    pub fn payloads(&self) -> Vec<bool> {
        self.by_stream.iter().map(Option::is_some).collect()
    }

    /// A packet read: listed (when `list`) and decoded, in that order.
    pub fn packet(&mut self, mut packet: Packet, list: bool) -> Result<(), Failure> {
        let data = std::mem::take(&mut packet.data);
        let dts = packet.dts;
        let lane_index = self.by_stream.get(packet.stream).copied().flatten();
        let mut facts = Facts {
            pts: packet.pts,
            duration: packet.duration,
            pos: packet.pos,
            size: packet.size,
            discard: packet.discard,
            pict_type: "?",
            ..Facts::default()
        };
        for data in &packet.side_data {
            if data.typ == "skip_samples" {
                for (key, v) in &data.entries {
                    match (*key, v) {
                        ("skip_samples", Value::Int(n)) => facts.skip = *n,
                        ("discard_padding", Value::Int(n)) => facts.padding = *n,
                        _ => {}
                    }
                }
            }
        }
        if list {
            self.queue.push_back(Slot::Ready(Item::Packet(packet)));
        }
        if let Some(index) = lane_index {
            if let Some(lane) = self.lanes.get_mut(index) {
                lane.decoder.facts(&data, &mut facts);
                let before = lane.decoded.len();
                lane.decoder
                    .send(&data, facts, &mut lane.decoded)
                    .map_err(|why| lane.stopped(&why))?;
                lane.decoded_count += (lane.decoded.len() - before) as u64;
                lane.sent += 1;
                if lane.sent > lane.delay {
                    lane.due += 1;
                    self.queue.push_back(Slot::Frame {
                        lane: index,
                        pkt_dts: dts,
                    });
                }
            }
        }
        if self.queue.len() > MAX_QUEUED {
            return Err(Failure::Unsupported(format!(
                "a frame is decoded more than {MAX_QUEUED} items after ffprobe lists it"
            )));
        }
        Ok(())
    }

    /// The input ended: every decoder returns what it holds, stream by
    /// stream, each frame with no packet read to return it.
    pub fn finish(&mut self) -> Result<(), Failure> {
        for (index, lane) in self.lanes.iter_mut().enumerate() {
            let before = lane.decoded.len();
            lane.decoder
                .finish(&mut lane.decoded)
                .map_err(|why| lane.stopped(&why))?;
            lane.decoded_count += (lane.decoded.len() - before) as u64;
            while lane.due < lane.decoded_count {
                lane.due += 1;
                self.queue.push_back(Slot::Frame {
                    lane: index,
                    pkt_dts: None,
                });
            }
        }
        self.ended = true;
        Ok(())
    }

    /// The next item that can be printed, if any.
    pub fn next(&mut self) -> Option<Item> {
        loop {
            match self.queue.front()? {
                Slot::Ready(_) => {
                    if let Some(Slot::Ready(item)) = self.queue.pop_front() {
                        return Some(item);
                    }
                }
                Slot::Frame { lane, pkt_dts } => {
                    let (index, pkt_dts) = (*lane, *pkt_dts);
                    let lane = self.lanes.get_mut(index)?;
                    let Some(decoded) = lane.decoded.pop_front() else {
                        if self.ended {
                            self.queue.pop_front();
                            continue;
                        }
                        return None;
                    };
                    self.queue.pop_front();
                    if let Some(row) = lane.row(decoded, pkt_dts) {
                        lane.listed += 1;
                        if self.show {
                            return Some(Item::Frame(row));
                        }
                    }
                }
            }
        }
    }

    /// Frames listed per stream, for `nb_read_frames`; `None` for a stream
    /// not decoded.
    pub fn counts(&self, streams: usize) -> Vec<Option<u64>> {
        let mut out = vec![None; streams];
        for lane in &self.lanes {
            if let Some(slot) = out.get_mut(lane.stream) {
                *slot = Some(lane.listed);
            }
        }
        out
    }
}

impl Lane {
    /// The refusal of a stream whose decoder stopped where `ffprobe`'s
    /// goes on, concealing what it cannot decode.
    fn stopped(&self, why: &str) -> Failure {
        Failure::Unsupported(format!(
            "{}: CoreAV's decoder stopped at packet {}: {why}",
            self.refusal,
            self.sent + 1
        ))
    }

    /// The frame as printed, or `None` for one `ffprobe` does not list:
    /// not presented, or audio skipped whole.
    fn row(&mut self, decoded: Decoded, pkt_dts: Option<i64>) -> Option<Row> {
        let Decoded { facts, shape } = decoded;
        if facts.discard {
            return None;
        }
        let (num, den) = self.time_base;
        let ts = |v: Option<i64>| v.map_or(Value::Na, Value::Int);
        let t = |v: Option<i64>| v.map_or(Value::Na, |v| time(v, num, den));
        let (mut pts, mut pkt_dts, mut duration) = (facts.pts, pkt_dts, facts.duration);
        let mut samples = 0;
        if let (Shape::Samples(n), Fixed::Audio { rate, .. }) = (shape, &self.fixed) {
            let n = i64::try_from(n).unwrap_or(i64::MAX);
            let skip = self.carry.saturating_add(facts.skip.max(0));
            if skip >= n && skip > 0 {
                self.carry = skip - n;
                return None;
            }
            self.carry = 0;
            let mut kept = (n - skip - facts.padding.clamp(0, n - skip)).max(0);
            // A packet in a time base of one tick per sample that lasts
            // less than its samples ends where an edit list ends it.
            if let (Some(d), Some(rate)) = (facts.duration, rate) {
                if (num, den) == (1, *rate) && (0..kept).contains(&d) {
                    kept = d;
                }
            }
            if kept == 0 {
                return None;
            }
            let ticks = |samples: i64| {
                rate.and_then(|rate| {
                    let d = i128::from(rate) * i128::from(num);
                    (d > 0).then(|| {
                        let v = i128::from(samples) * i128::from(den);
                        i64::try_from((2 * v + d) / (2 * d)).unwrap_or(i64::MAX)
                    })
                })
            };
            if skip > 0 {
                let shift = ticks(skip).unwrap_or(0);
                pts = pts.map(|p| p.saturating_add(shift));
                pkt_dts = pkt_dts.map(|p| p.saturating_add(shift));
            }
            if kept != n {
                duration = ticks(kept).or(duration);
            }
            samples = kept;
        }
        for (i, v) in [pts, pkt_dts].into_iter().enumerate() {
            if let (Some(v), Some(last), Some(faults)) =
                (v, self.last.get_mut(i), self.faults.get_mut(i))
            {
                if last.is_some_and(|l| v <= l) {
                    *faults += 1;
                }
                *last = Some(v);
            }
        }
        let [pts_faults, dts_faults] = self.faults;
        let best = match (pts, pkt_dts) {
            (Some(p), Some(d)) => Some(if pts_faults <= dts_faults { p } else { d }),
            (p, d) => p.or(d),
        };
        let mut entries: Entries = Vec::with_capacity(33);
        let media = if matches!(self.fixed, Fixed::Video { .. }) {
            "video"
        } else {
            "audio"
        };
        entries.extend([
            ("media_type", s(media)),
            (
                "stream_index",
                Value::Int(i64::try_from(self.stream).unwrap_or(0)),
            ),
            ("key_frame", Value::Int(i64::from(facts.key))),
            ("pts", ts(pts)),
            ("pts_time", t(pts)),
            ("pkt_dts", ts(pkt_dts)),
            ("pkt_dts_time", t(pkt_dts)),
            ("best_effort_timestamp", ts(best)),
            ("best_effort_timestamp_time", t(best)),
            ("duration", ts(duration)),
            ("duration_time", t(duration)),
            (
                "pkt_pos",
                facts.pos.map_or(Value::Na, |p| Value::Str(p.to_string())),
            ),
            (
                "pkt_size",
                Value::Unit(
                    i64::try_from(facts.size).unwrap_or(i64::MAX),
                    super::model::Unit::Byte,
                ),
            ),
        ]);
        let mut side_data = Vec::new();
        match (&self.fixed, shape) {
            (
                Fixed::Video {
                    pix_fmt,
                    sample_aspect_ratio,
                    colors,
                    side_data: stream_side_data,
                },
                Shape::Picture { width, height },
            ) => {
                let [range, space, primaries, transfer, chroma] = colors.clone();
                entries.extend([
                    ("width", Value::Int(i64::from(width))),
                    ("height", Value::Int(i64::from(height))),
                    ("crop_top", Value::Int(0)),
                    ("crop_bottom", Value::Int(0)),
                    ("crop_left", Value::Int(0)),
                    ("crop_right", Value::Int(0)),
                    ("pix_fmt", pix_fmt.clone()),
                    ("sample_aspect_ratio", sample_aspect_ratio.clone()),
                    ("pict_type", s(facts.pict_type)),
                    ("interlaced_frame", Value::Int(0)),
                    ("top_field_first", Value::Int(0)),
                    ("lossless", Value::Int(0)),
                    ("repeat_pict", Value::Int(0)),
                    ("color_range", range),
                    ("color_space", space),
                    ("color_primaries", primaries),
                    ("color_transfer", transfer),
                    ("chroma_location", chroma),
                    ("alpha_mode", Value::Unset("unspecified")),
                ]);
                side_data.extend(stream_side_data.iter().cloned());
                if facts.user_data {
                    side_data.push(SideData {
                        typ: "h_26_45__user_data_unregistered_sei_message",
                        entries: vec![(
                            "side_data_type",
                            s("H.26[45] User Data Unregistered SEI message"),
                        )],
                    });
                }
            }
            (
                Fixed::Audio {
                    sample_fmt,
                    channels,
                    channel_layout,
                    ..
                },
                _,
            ) => entries.extend([
                ("sample_fmt", sample_fmt.clone()),
                ("nb_samples", Value::Int(samples)),
                ("channels", channels.clone()),
                ("channel_layout", channel_layout.clone()),
            ]),
            (Fixed::Video { .. }, _) => return None,
        }
        Some(Row { entries, side_data })
    }
}

/// The `size` bytes at `offset`, as a decoder takes them, within the
/// decode boundary's packet limit.
pub(super) fn read<S: coreav_isobmff::Source>(
    source: &mut S,
    offset: u64,
    size: u64,
) -> Result<Vec<u8>, Failure> {
    let limit = coreav_decode::Limits::default().max_packet_bytes;
    if size > limit {
        return Err(Failure::VideoDecode(coreav_decode::Error::new(
            coreav_decode::ErrorKind::Limit,
            format!("a packet of {size} bytes is over the limit of {limit}"),
        )));
    }
    let mut bytes = vec![0u8; usize::try_from(size).unwrap_or(0)];
    source
        .read_exact_at(offset, &mut bytes)
        .map_err(Failure::library)?;
    Ok(bytes)
}

/// A picture's `pix_fmt`, `color_range`, `color_space`,
/// `color_primaries` and `color_transfer`: what the video signal type of
/// its H.264 sequence parameter set states, over what the stream prints
/// from its container (found black box).
fn picture_colors(stream: &Stream, setup: &Setup) -> [Value; 5] {
    let mut out = [
        "pix_fmt",
        "color_range",
        "color_space",
        "color_primaries",
        "color_transfer",
    ]
    .map(|key| value(stream, key));
    let config = setup.config.as_deref().unwrap_or_default();
    let Some((full_range, described)) =
        super::video::avc(config, false).and_then(|info| info.h264_signal)
    else {
        return out;
    };
    let [pix_fmt, range, space, primaries, transfer] = &mut out;
    if let Some(name) = super::video::avc(config, full_range).and_then(|info| info.pix_fmt) {
        *pix_fmt = Value::Str(name);
    }
    *range = s(if full_range { "pc" } else { "tv" });
    if let Some([p, t, m]) = described {
        let name = |code: u32, f: fn(u16) -> Option<&'static str>| {
            u16::try_from(code)
                .ok()
                .and_then(f)
                .map_or(Value::Unset("unknown"), s)
        };
        *space = name(m, crate::names::matrix);
        *primaries = name(p, crate::names::primaries);
        *transfer = name(t, crate::names::transfer);
    }
    out
}

fn value(stream: &Stream, key: &str) -> Value {
    stream
        .entries
        .iter()
        .find(|(k, _)| *k == key)
        .map_or(Value::Na, |(_, v)| v.clone())
}

fn text(stream: &Stream, key: &str) -> Option<String> {
    match value(stream, key) {
        Value::Str(t) => Some(t),
        _ => None,
    }
}

fn number(stream: &Stream, key: &str) -> Option<i64> {
    match value(stream, key) {
        Value::Int(n) | Value::Unit(n, _) => Some(n),
        Value::Str(t) => t.parse().ok(),
        _ => None,
    }
}

fn audio(codec: &str, stream: &Stream, setup: &Setup) -> Result<Box<dyn FrameDecoder>, String> {
    let limits = AudioLimits::default();
    if codec == "mp3" {
        let channels = number(stream, "channels").and_then(|n| u16::try_from(n).ok());
        let decoder = Mp3Decoder::new(channels, &limits).map_err(|e| e.to_string())?;
        return Ok(Box::new(Mp3 {
            decoder: Box::new(decoder),
            pcm: Pcm::new(),
        }));
    }
    if codec == "flac" {
        let config = setup
            .config
            .as_deref()
            .ok_or("the stream has no FLAC configuration")?;
        let info = StreamInfo::from_config(config).map_err(|e| e.to_string())?;
        let decoder = FlacDecoder::new(info, &limits).map_err(|e| e.to_string())?;
        return Ok(Box::new(Flac {
            decoder: Box::new(decoder),
            pcm: Pcm::new(),
        }));
    }
    let bytes = match codec {
        "pcm_u8" | "pcm_s8" | "pcm_alaw" | "pcm_mulaw" => 1,
        "pcm_s16le" | "pcm_s16be" => 2,
        "pcm_s24le" | "pcm_s24be" => 3,
        "pcm_s32le" | "pcm_s32be" | "pcm_f32le" | "pcm_f32be" => 4,
        "pcm_f64le" | "pcm_f64be" => 8,
        _ => return Err(format!("CoreAV has no {codec} decoder")),
    };
    let channels = number(stream, "channels")
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or("the stream states no channel count")?;
    Ok(Box::new(Raw {
        frame_bytes: bytes * channels,
    }))
}

struct Mp3 {
    decoder: Box<Mp3Decoder>,
    pcm: Pcm,
}

impl FrameDecoder for Mp3 {
    fn send(
        &mut self,
        data: &[u8],
        facts: Facts,
        out: &mut VecDeque<Decoded>,
    ) -> Result<(), String> {
        self.decoder
            .decode(data, &mut self.pcm)
            .map_err(|e| e.to_string())?;
        out.push_back(Decoded {
            facts,
            shape: Shape::Samples(self.pcm.frames() as u64),
        });
        Ok(())
    }

    fn finish(&mut self, _out: &mut VecDeque<Decoded>) -> Result<(), String> {
        Ok(())
    }
}

struct Flac {
    decoder: Box<FlacDecoder>,
    pcm: Pcm,
}

impl FrameDecoder for Flac {
    fn send(
        &mut self,
        data: &[u8],
        facts: Facts,
        out: &mut VecDeque<Decoded>,
    ) -> Result<(), String> {
        let frame = self
            .decoder
            .decode(data, &mut self.pcm)
            .map_err(|e| e.to_string())?;
        out.push_back(Decoded {
            facts,
            shape: Shape::Samples(u64::from(frame.block_size)),
        });
        Ok(())
    }

    fn finish(&mut self, _out: &mut VecDeque<Decoded>) -> Result<(), String> {
        Ok(())
    }
}

/// PCM: each packet is whole samples of every channel.
struct Raw {
    frame_bytes: usize,
}

impl FrameDecoder for Raw {
    fn send(
        &mut self,
        data: &[u8],
        facts: Facts,
        out: &mut VecDeque<Decoded>,
    ) -> Result<(), String> {
        out.push_back(Decoded {
            facts,
            shape: Shape::Samples((data.len() / self.frame_bytes.max(1)) as u64),
        });
        Ok(())
    }

    fn finish(&mut self, _out: &mut VecDeque<Decoded>) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(not(feature = "native-h264"))]
fn video(codec: &str, _setup: &Setup) -> Result<Box<dyn FrameDecoder>, String> {
    Err(match codec {
        "h264" => {
            "CoreAV's H.264 decoder is not in this build (the native-h264 feature)".to_owned()
        }
        _ => format!("CoreAV has no {codec} decoder"),
    })
}

#[cfg(feature = "native-h264")]
fn video(codec: &str, setup: &Setup) -> Result<Box<dyn FrameDecoder>, String> {
    use coreav_decode::{
        Backend, CodecConfig, ConfigKind, DecodeAllowlist, Limits, Policy, StreamConfig,
    };
    if codec != "h264" {
        return Err(format!("CoreAV has no {codec} decoder"));
    }
    let config = setup
        .config
        .as_ref()
        .filter(|c| !c.is_empty())
        .map(|data| CodecConfig {
            kind: ConfigKind::Avcc,
            data: data.clone(),
        });
    let length_size = config
        .as_ref()
        .map(|c| c.data.get(4).map_or(4, |b| usize::from(b & 3) + 1));
    let stream = StreamConfig {
        config,
        width: setup.width,
        height: setup.height,
        ..StreamConfig::new(coreav_decode::Codec::H264)
    };
    // A listing decodes every picture and keeps none: the work is bounded
    // by the file, not by the bytes or the time a thumbnail may take.
    let limits = Limits {
        max_decoded_bytes: u64::MAX,
        timeout: std::time::Duration::from_secs(7 * 24 * 3600),
        ..Limits::default()
    };
    let backend = coreav_backend_h264::NativeH264::new();
    let backends: [&dyn Backend; 1] = [&backend];
    let policy = Policy {
        allowlist: DecodeAllowlist::default(),
        order: vec![coreav_decode::BackendKind::Native],
    };
    let sps = setup
        .config
        .as_deref()
        .and_then(|config| super::video::avc(config, false));
    if sps.as_ref().is_some_and(|info| info.field_order.is_none()) {
        return Err(FIELDS.to_owned());
    }
    let crop_left = sps.and_then(|info| info.h264_crop_left).unwrap_or(0);
    let opened =
        coreav_decode::open(&stream, &policy, &limits, &backends).map_err(|e| e.message)?;
    Ok(Box::new(H264 {
        decoder: opened.decoder,
        length_size,
        pending: VecDeque::new(),
        next: 0,
        crop_left,
        fields: false,
    }))
}

/// H.264 through the native decoder. Each packet is sent with its
/// position in decoding order as its time, so that each picture names the
/// packet it came from.
#[cfg(feature = "native-h264")]
struct H264 {
    decoder: Box<dyn coreav_decode::Decoder>,
    /// Bytes of each NAL unit's length, or `None` for a byte stream.
    length_size: Option<usize>,
    /// The packets whose pictures have not come out yet, each with the
    /// columns its picture shows that the decoder crops away.
    pending: VecDeque<(i64, Facts, u32)>,
    next: i64,
    /// The left cropping of the last sequence parameter set.
    crop_left: u32,
    /// A sequence parameter set allowed field coding.
    fields: bool,
}

/// Why a stream that may code fields is refused: `interlaced_frame`,
/// `top_field_first` and `repeat_pict` are not derived.
#[cfg(feature = "native-h264")]
const FIELDS: &str = "pictures that may be coded as fields (frame_mbs_only_flag 0) are not listed";

#[cfg(feature = "native-h264")]
impl H264 {
    fn drain(&mut self, out: &mut VecDeque<Decoded>) -> Result<(), String> {
        while let Some(frame) = self.decoder.receive_frame().map_err(|e| e.message)? {
            let Some(at) = self
                .pending
                .iter()
                .position(|(mark, _, _)| Some(*mark) == frame.pts)
            else {
                continue;
            };
            if let Some((_, facts, uncropped)) = self.pending.remove(at) {
                out.push_back(Decoded {
                    facts,
                    shape: Shape::Picture {
                        width: frame.width().saturating_add(uncropped),
                        height: frame.height(),
                    },
                });
            }
        }
        Ok(())
    }
}

#[cfg(feature = "native-h264")]
impl FrameDecoder for H264 {
    fn facts(&mut self, data: &[u8], facts: &mut Facts) {
        let picture = h264::picture(data, self.length_size);
        if let Some(crop_left) = picture.crop_left {
            self.crop_left = crop_left;
        }
        self.fields |= picture.fields;
        facts.key = picture.idr || picture.recovery;
        facts.pict_type = match picture.slice_type.map(|t| t % 5) {
            Some(0) => "P",
            Some(1) => "B",
            Some(2) => "I",
            Some(3) => "p",
            Some(4) => "i",
            _ => "?",
        };
        facts.user_data = picture.user_data;
    }

    fn send(
        &mut self,
        data: &[u8],
        facts: Facts,
        out: &mut VecDeque<Decoded>,
    ) -> Result<(), String> {
        if self.fields {
            return Err(FIELDS.to_owned());
        }
        let mark = self.next;
        self.next += 1;
        // A left cropping of a width not a multiple of 64 is not applied
        // to the picture `ffprobe` lists, the others are (found black
        // box).
        let uncropped = if self.crop_left % 64 == 0 {
            0
        } else {
            self.crop_left
        };
        self.pending.push_back((mark, facts, uncropped));
        // A packet whose picture never came out (a parameter set alone, a
        // picture the decoder skipped) is not waited for forever.
        if self.pending.len() > 256 {
            self.pending.pop_front();
        }
        self.decoder
            .send_packet(&coreav_decode::Packet {
                data,
                pts: Some(mark),
                dts: Some(mark),
                keyframe: false,
                discard: false,
            })
            .map_err(|e| e.message)?;
        self.drain(out)
    }

    fn finish(&mut self, out: &mut VecDeque<Decoded>) -> Result<(), String> {
        self.decoder.send_eof().map_err(|e| e.message)?;
        self.drain(out)
    }
}

/// What an H.264 access unit's NAL units say about its picture (ITU-T
/// H.264 7.3 and D.1), read without decoding it.
#[cfg(feature = "native-h264")]
mod h264 {
    use super::super::video::{Bits, avc_sps, rbsp};

    /// What [`picture`] reads.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Picture {
        /// A slice of an IDR picture (`nal_unit_type` 5).
        pub idr: bool,
        /// A recovery point SEI message with `recovery_frame_cnt` 0: the
        /// picture is exact from here on.
        pub recovery: bool,
        /// The first slice's `slice_type`.
        pub slice_type: Option<u32>,
        /// A user data unregistered SEI message (payload type 5).
        pub user_data: bool,
        /// The left cropping of a sequence parameter set in the unit.
        pub crop_left: Option<u32>,
        /// A sequence parameter set in the unit allows field pictures or
        /// field macroblock pairs (`frame_mbs_only_flag` 0).
        pub fields: bool,
    }

    /// Reads the access unit `data`: NAL units each after a big-endian
    /// length of `length_size` bytes, or a byte stream (Annex B) when it
    /// is `None`.
    pub fn picture(data: &[u8], length_size: Option<usize>) -> Picture {
        let mut picture = Picture::default();
        let mut visit = |nal: &[u8]| unit(nal, &mut picture);
        match length_size {
            Some(n) => {
                let mut rest = data;
                while let Some((len, tail)) = rest.split_at_checked(n) {
                    let len = len.iter().fold(0usize, |a, b| (a << 8) | usize::from(*b));
                    let Some(nal) = tail.get(..len) else {
                        break;
                    };
                    visit(nal);
                    rest = tail.get(len..).unwrap_or_default();
                }
            }
            None => byte_stream(data, visit),
        }
        picture
    }

    /// Calls `f` with each NAL unit of a byte stream: what lies between
    /// one start code prefix (`00 00 01`) and the next, without the zero
    /// bytes before the next.
    fn byte_stream(data: &[u8], mut f: impl FnMut(&[u8])) {
        let mut start = None;
        let mut i = 0;
        while let Some(window) = data.get(i..i + 3) {
            if window == [0, 0, 1] {
                if let Some(from) = start {
                    f(trimmed(data.get(from..i).unwrap_or_default()));
                }
                start = Some(i + 3);
                i += 3;
            } else {
                i += 1;
            }
        }
        if let Some(from) = start {
            f(trimmed(data.get(from..).unwrap_or_default()));
        }
    }

    fn trimmed(nal: &[u8]) -> &[u8] {
        let end = nal.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
        nal.get(..end).unwrap_or_default()
    }

    fn unit(nal: &[u8], picture: &mut Picture) {
        let Some(&header) = nal.first() else {
            return;
        };
        match header & 0x1F {
            kind @ (1 | 5) => {
                picture.idr |= kind == 5;
                if picture.slice_type.is_none() {
                    // `first_mb_in_slice`, then `slice_type` (7.3.3).
                    let head = rbsp(nal.get(..17).unwrap_or(nal), 1);
                    let mut bits = Bits::new(&head);
                    if bits.ue().is_some() {
                        picture.slice_type = bits.ue();
                    }
                }
            }
            6 => sei(&rbsp(nal, 1), picture),
            7 => {
                if let Some(info) = avc_sps(nal, false) {
                    picture.crop_left = info.h264_crop_left.or(picture.crop_left);
                    picture.fields |= info.field_order.is_none();
                }
            }
            _ => {}
        }
    }

    /// The SEI messages of an SEI RBSP (7.3.2.3).
    fn sei(data: &[u8], picture: &mut Picture) {
        let mut rest = data;
        while !rest.is_empty() && rest != [0x80] {
            let Some((kind, after)) = number(rest) else {
                return;
            };
            let Some((size, after)) = number(after) else {
                return;
            };
            let Some(payload) = after.get(..size) else {
                return;
            };
            match kind {
                5 => picture.user_data = true,
                6 => picture.recovery |= Bits::new(payload).ue() == Some(0),
                _ => {}
            }
            rest = after.get(size..).unwrap_or_default();
        }
    }

    /// A payload type or size: bytes of 255 summed with the first byte
    /// that is not.
    fn number(data: &[u8]) -> Option<(usize, &[u8])> {
        let mut total = 0usize;
        for (i, &b) in data.iter().enumerate() {
            total = total.checked_add(usize::from(b))?;
            if b != 0xFF {
                return Some((total, data.get(i + 1..).unwrap_or_default()));
            }
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn length_prefixed(nals: &[&[u8]]) -> Vec<u8> {
            let mut data = Vec::new();
            for nal in nals {
                data.extend(u32::try_from(nal.len()).unwrap().to_be_bytes());
                data.extend(*nal);
            }
            data
        }

        fn hex(text: &str) -> Vec<u8> {
            (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
                .collect()
        }

        #[test]
        fn an_idr_slice_and_its_sei() {
            // SEI: user data unregistered (type 5, 17 bytes), recovery
            // point with recovery_frame_cnt 0; then an IDR slice with
            // first_mb_in_slice 0 and slice_type 7 (I).
            let mut sei = vec![0x06, 0x05, 0x11];
            sei.extend([0xAA; 17]);
            sei.extend([0x06, 0x01, 0x80, 0x80]);
            let slice = [0x65, 0x88, 0x80];
            let p = picture(&length_prefixed(&[&sei, &slice]), Some(4));
            assert_eq!(
                p,
                Picture {
                    idr: true,
                    recovery: true,
                    slice_type: Some(7),
                    user_data: true,
                    crop_left: None,
                    fields: false,
                }
            );
        }

        #[test]
        fn a_byte_stream_b_slice() {
            // A non-IDR slice: first_mb_in_slice 0, slice_type 1 (B).
            let p = picture(&[0, 0, 0, 1, 0x41, 0xA0, 0, 0, 1, 0x09, 0xF0], None);
            assert_eq!(p.slice_type, Some(1));
            assert!(!p.idr);
        }

        #[test]
        fn the_left_cropping_of_a_sequence_parameter_set() {
            // A High profile 4:2:0 sequence parameter set of 320x240 that
            // crops 48 luma columns on the left (`frame_crop_left_offset`
            // 24), as an `avcC` record stores it.
            let avcc = hex(
                "0164000dffe1001a6764000dacd94141fc33f011000003000100000300320f14299601000668ebe3cb22c0fdf8f800",
            );
            let sps = &avcc[8..8 + 26];
            let p = picture(&length_prefixed(&[sps]), Some(4));
            assert_eq!(p.crop_left, Some(48));
            assert!(!p.fields);
        }
    }
}
