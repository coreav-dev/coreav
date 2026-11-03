use super::super::model::{self, Unit, Value};
use super::super::{Entries, Report, Stream, StreamKind, iso, names};
use crate::error::Failure;
use coreav_isobmff::{Error, LimitKind, Limits, Source};
use std::mem::size_of;

pub(super) fn probe<S: Source>(
    source: &mut S,
    size: u64,
    name: &str,
    limits: &Limits,
    start: u64,
) -> Result<Report, Failure> {
    if limits.max_tracks == 0 {
        return Err(limit(LimitKind::Tracks, start));
    }
    if limits.max_depth == 0 {
        return Err(limit(LimitKind::Depth, start));
    }
    let mut reader = Reader {
        source,
        size,
        limits,
        offset: start,
        blocks: 0,
        work: 0,
        retained: 0,
        tables: 0,
    };
    reader.charge(size_of::<Report>() + 1024, false)?;
    if reader.read::<4>()? != *b"fLaC" {
        return Err(Failure::Invalid("FLAC marker"));
    }
    let mut info = None;
    let mut comments = false;
    let mut seektable = false;
    let mut tags = Vec::new();
    let mut mask = None;
    loop {
        reader.blocks = reader
            .blocks
            .checked_add(1)
            .ok_or_else(|| overflow(reader.offset))?;
        if reader.blocks > limits.max_total_boxes {
            return Err(limit(LimitKind::TotalBoxes, reader.offset));
        }
        if reader.blocks > limits.max_top_level_boxes {
            return Err(limit(LimitKind::TopLevelBoxes, reader.offset));
        }
        reader.work(1)?;
        let [flags, a, b, c] = reader.read::<4>()?;
        let length = u32::from_be_bytes([0, a, b, c]);
        let end = reader.end(u64::from(length))?;
        let typ = flags & 127;
        if info.is_none() && typ != 0 {
            return Err(Failure::Invalid("FLAC STREAMINFO must be first"));
        }
        match typ {
            0 => {
                if info.is_some() || length != 34 {
                    return Err(Failure::Invalid("FLAC requires one 34-byte STREAMINFO"));
                }
                reader.box_bytes(34)?;
                info = Some(Info::parse(reader.read::<34>()?)?);
            }
            1 => reader.offset = end,
            3 => {
                if seektable {
                    return Err(Failure::Invalid("FLAC has multiple seek tables"));
                }
                seektable = true;
                if length % 18 != 0 {
                    return Err(Failure::Invalid("FLAC seek table length"));
                }
                reader.offset = end;
            }
            4 => {
                if comments {
                    return Err(Failure::Invalid("FLAC has multiple Vorbis comment blocks"));
                }
                comments = true;
                reader.box_bytes(u64::from(length))?;
                let mut bytes = Vec::new();
                let length = usize::try_from(length).map_err(|_| overflow(reader.offset))?;
                reader.reserve(&mut bytes, length, true)?;
                bytes.resize(length, 0);
                reader
                    .source
                    .read_exact_at(reader.offset, &mut bytes)
                    .map_err(Failure::library)?;
                parse_comments(&bytes, &mut reader, &mut tags, &mut mask)?;
                reader.offset = end;
            }
            127 => return Err(Failure::Invalid("FLAC forbidden metadata block type")),
            _ => {
                return Err(Failure::Unsupported(
                    "FLAC metadata requiring additional report information".to_owned(),
                ));
            }
        }
        if flags & 128 != 0 {
            break;
        }
    }
    let info = info.ok_or(Failure::Invalid("FLAC missing STREAMINFO"))?;
    for (key, _) in &mut tags {
        let alias = if key.eq_ignore_ascii_case("DESCRIPTION") {
            Some("comment")
        } else if key.eq_ignore_ascii_case("TRACKNUMBER") {
            Some("track")
        } else if key.eq_ignore_ascii_case("DISCNUMBER") {
            Some("disc")
        } else if key.eq_ignore_ascii_case("ALBUMARTIST") {
            Some("album_artist")
        } else {
            None
        };
        if let Some(alias) = alias {
            *key = reader.tag_text(alias)?;
        }
    }
    report(info, tags, mask, name, &mut reader)
}

struct Reader<'a, S> {
    source: &'a mut S,
    size: u64,
    limits: &'a Limits,
    offset: u64,
    blocks: u32,
    work: u64,
    retained: u64,
    tables: u64,
}

impl<S: Source> Reader<'_, S> {
    fn end(&self, length: u64) -> Result<u64, Failure> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or_else(|| overflow(self.offset))?;
        if end > self.size {
            return Err(Failure::Library(Error::Truncated {
                offset: self.offset,
            }));
        }
        Ok(end)
    }

    fn read<const N: usize>(&mut self) -> Result<[u8; N], Failure> {
        let end = self.end(N as u64)?;
        let mut bytes = [0; N];
        self.source
            .read_exact_at(self.offset, &mut bytes)
            .map_err(Failure::library)?;
        self.offset = end;
        Ok(bytes)
    }

    fn work(&mut self, count: u64) -> Result<(), Failure> {
        self.work = self
            .work
            .checked_add(count)
            .ok_or_else(|| overflow(self.offset))?;
        if self.work > self.limits.max_sample_work {
            return Err(limit(LimitKind::SampleWork, self.offset));
        }
        Ok(())
    }

    fn box_bytes(&self, bytes: u64) -> Result<(), Failure> {
        if bytes > self.limits.max_box_bytes {
            return Err(limit(LimitKind::BoxBytes, self.offset));
        }
        Ok(())
    }

    fn charge(&mut self, bytes: usize, table: bool) -> Result<(), Failure> {
        let bytes = u64::try_from(bytes).map_err(|_| overflow(self.offset))?;
        self.retained = self
            .retained
            .checked_add(bytes)
            .ok_or_else(|| overflow(self.offset))?;
        if self.retained > self.limits.max_retained_bytes {
            return Err(limit(LimitKind::RetainedBytes, self.offset));
        }
        if table {
            self.tables = self
                .tables
                .checked_add(bytes)
                .ok_or_else(|| overflow(self.offset))?;
            if self.tables > self.limits.max_table_bytes {
                return Err(limit(LimitKind::TableBytes, self.offset));
            }
        }
        Ok(())
    }

    fn reserve<T>(
        &mut self,
        values: &mut Vec<T>,
        count: usize,
        table: bool,
    ) -> Result<(), Failure> {
        let old_capacity = values.capacity();
        let needed = values
            .len()
            .checked_add(count)
            .ok_or_else(|| overflow(self.offset))?;
        let added = needed.saturating_sub(old_capacity);
        let bytes = added
            .checked_mul(size_of::<T>())
            .ok_or_else(|| overflow(self.offset))?;
        self.charge(bytes, table)?;
        values
            .try_reserve_exact(count)
            .map_err(|_| limit(LimitKind::RetainedBytes, self.offset))?;
        let extra = values
            .capacity()
            .saturating_sub(old_capacity)
            .saturating_sub(added)
            .checked_mul(size_of::<T>())
            .ok_or_else(|| overflow(self.offset))?;
        self.charge(extra, table)
    }

    fn text(&mut self, value: &str) -> Result<String, Failure> {
        self.copy_text(value, false)
    }

    fn tag_text(&mut self, value: &str) -> Result<String, Failure> {
        self.copy_text(value, true)
    }

    fn copy_text(&mut self, value: &str, table: bool) -> Result<String, Failure> {
        self.charge(value.len(), table)?;
        let mut text = String::new();
        text.try_reserve_exact(value.len())
            .map_err(|_| limit(LimitKind::RetainedBytes, self.offset))?;
        self.charge(text.capacity().saturating_sub(value.len()), table)?;
        text.push_str(value);
        Ok(text)
    }
}

struct Info {
    rate: u32,
    channels: u32,
    bits: u16,
    samples: u64,
    min_block: u64,
    max_block: u64,
    min_frame: u64,
    max_frame: u64,
}

impl Info {
    fn parse(bytes: [u8; 34]) -> Result<Self, Failure> {
        let mut bytes = bytes.as_slice();
        let min_block = number(&mut bytes, 2)?;
        let max_block = number(&mut bytes, 2)?;
        let min_frame = number(&mut bytes, 3)?;
        let max_frame = number(&mut bytes, 3)?;
        let packed = number(&mut bytes, 8)?;
        let rate = (packed >> 44) as u32;
        let channels = ((packed >> 41) & 7) as u32 + 1;
        let bits = ((packed >> 36) & 31) as u16 + 1;
        if min_block < 16
            || max_block < min_block
            || bits < 4
            || (min_frame != 0 && max_frame != 0 && min_frame > max_frame)
        {
            return Err(Failure::Invalid("FLAC STREAMINFO fields"));
        }
        if rate == 0 {
            return Err(Failure::Unsupported(
                "non-audio FLAC sample rate".to_owned(),
            ));
        }
        Ok(Self {
            rate,
            channels,
            bits,
            samples: packed & 0x0f_ffff_ffff,
            min_block,
            max_block,
            min_frame,
            max_frame,
        })
    }

    fn start(&self, payload: u64) -> Result<Option<i64>, Failure> {
        if payload == 0
            || (self.max_frame != 0 && payload == self.max_frame)
            || (self.min_frame != 0 && payload < self.min_frame * 2)
            || (self.samples != 0 && self.samples <= self.min_block)
        {
            return Ok(None);
        }
        if (self.max_frame != 0 && payload > self.max_frame) || self.samples > self.max_block {
            return Ok(Some(0));
        }
        Err(Failure::Unsupported(
            "FLAC timing requires inspecting audio frames".to_owned(),
        ))
    }
}

fn number(bytes: &mut &[u8], length: usize) -> Result<u64, Failure> {
    let (head, tail) = bytes
        .split_at_checked(length)
        .ok_or(Failure::Invalid("FLAC metadata length"))?;
    *bytes = tail;
    Ok(head
        .iter()
        .fold(0, |value, byte| (value << 8) | u64::from(*byte)))
}

fn field<'a>(bytes: &mut &'a [u8]) -> Result<&'a str, Failure> {
    let length = little(bytes)?;
    let length = usize::try_from(length).map_err(|_| Failure::Invalid("FLAC comment length"))?;
    let (head, tail) = bytes
        .split_at_checked(length)
        .ok_or(Failure::Invalid("FLAC comment length"))?;
    *bytes = tail;
    std::str::from_utf8(head).map_err(|_| Failure::Invalid("FLAC comment UTF-8"))
}

fn little(bytes: &mut &[u8]) -> Result<u32, Failure> {
    let (head, tail) = bytes
        .split_at_checked(4)
        .ok_or(Failure::Invalid("FLAC comment length"))?;
    *bytes = tail;
    let array = head
        .try_into()
        .map_err(|_| Failure::Invalid("FLAC comment length"))?;
    Ok(u32::from_le_bytes(array))
}

fn parse_comments<S: Source>(
    mut bytes: &[u8],
    reader: &mut Reader<'_, S>,
    tags: &mut Vec<(String, String)>,
    mask: &mut Option<u32>,
) -> Result<(), Failure> {
    field(&mut bytes)?;
    let count = little(&mut bytes)?;
    reader.work(u64::from(count))?;
    if u64::from(count) > bytes.len() as u64 / 4 {
        return Err(Failure::Invalid("FLAC comment count"));
    }
    reader.reserve(
        tags,
        usize::try_from(count).map_err(|_| overflow(reader.offset))?,
        true,
    )?;
    for _ in 0..count {
        let value = field(&mut bytes)?;
        let (key, value) = value
            .split_once('=')
            .ok_or(Failure::Invalid("FLAC comment field"))?;
        if key.is_empty() || !key.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
            return Err(Failure::Invalid("FLAC comment name"));
        }
        if value.is_empty() {
            continue;
        }
        if key.eq_ignore_ascii_case("WAVEFORMATEXTENSIBLE_CHANNEL_MASK") {
            let hex = value
                .strip_prefix("0x")
                .or_else(|| value.strip_prefix("0X"))
                .ok_or(Failure::Invalid("FLAC channel mask"))?;
            let parsed =
                u32::from_str_radix(hex, 16).map_err(|_| Failure::Invalid("FLAC channel mask"))?;
            if parsed != 0 && parsed & !0x3ffff == 0 {
                if mask.is_none() {
                    *mask = Some(parsed);
                }
                continue;
            }
        }
        if key.eq_ignore_ascii_case("METADATA_BLOCK_PICTURE")
            || key.eq_ignore_ascii_case("COVERART")
            || key
                .get(..11)
                .is_some_and(|k| k.eq_ignore_ascii_case("REPLAYGAIN_"))
            || key
                .get(..7)
                .is_some_and(|k| k.eq_ignore_ascii_case("CHAPTER"))
        {
            return Err(Failure::Unsupported(
                "FLAC comments requiring additional report information".to_owned(),
            ));
        }
        let mut found = None;
        for (index, (existing, _)) in tags.iter().enumerate() {
            reader.work(1)?;
            if existing.eq_ignore_ascii_case(key) {
                found = Some(index);
                break;
            }
        }
        let key = reader.tag_text(key)?;
        let value = if let Some(index) = found {
            let (_, old) = tags.swap_remove(index);
            let length = old
                .len()
                .checked_add(1)
                .and_then(|n| n.checked_add(value.len()))
                .ok_or_else(|| overflow(reader.offset))?;
            reader.charge(length, true)?;
            let mut joined = String::new();
            joined
                .try_reserve_exact(length)
                .map_err(|_| limit(LimitKind::RetainedBytes, reader.offset))?;
            reader.charge(joined.capacity().saturating_sub(length), true)?;
            joined.push_str(&old);
            joined.push(';');
            joined.push_str(value);
            joined
        } else {
            reader.tag_text(value)?
        };
        tags.push((key, value));
    }
    if !bytes.is_empty() {
        return Err(Failure::Invalid("FLAC trailing comment bytes"));
    }
    Ok(())
}

fn report<S: Source>(
    info: Info,
    tags: Vec<(String, String)>,
    mask: Option<u32>,
    name: &str,
    reader: &mut Reader<'_, S>,
) -> Result<Report, Failure> {
    let duration = (info.samples != 0).then_some(info.samples as i64);
    let start = info.start(reader.size - reader.offset)?;
    let micros = duration
        .map(|samples| (samples * 1_000_000 + i64::from(info.rate) / 2) / i64::from(info.rate));
    let layout = layout(info.channels, mask, reader)?;
    let filename = reader.text(name)?;
    let mut entries: Entries = Vec::new();
    reader.reserve(&mut entries, 30, false)?;
    let values = [
        ("index", Value::Int(0)),
        ("codec_name", model::s(reader.text("flac")?)),
        (
            "codec_long_name",
            model::s(
                reader
                    .text(names::long_name("flac").ok_or(Failure::Invalid("FLAC codec name"))?)?,
            ),
        ),
        ("profile", Value::Unset("unknown")),
        ("codec_type", model::s(reader.text("audio")?)),
        ("codec_tag_string", model::s(reader.text("[0][0][0][0]")?)),
        ("codec_tag", model::s(reader.text("0x0000")?)),
        ("mime_codec_string", model::s(reader.text("flac")?)),
        (
            "sample_fmt",
            model::s(
                reader.text(
                    names::sample_fmt("flac", Some(info.bits))
                        .ok_or(Failure::Invalid("FLAC sample format"))?,
                )?,
            ),
        ),
        (
            "sample_rate",
            Value::Unit(i64::from(info.rate), Unit::Hertz),
        ),
        ("channels", Value::Int(i64::from(info.channels))),
        ("channel_layout", model::s(layout)),
        ("bits_per_sample", Value::Int(0)),
        ("initial_padding", Value::Int(0)),
        ("id", Value::Na),
        ("r_frame_rate", model::s(reader.text("0/0")?)),
        ("avg_frame_rate", model::s(reader.text("0/0")?)),
        (
            "time_base",
            model::s(reader.text(&format!("1/{}", info.rate))?),
        ),
        ("start_pts", start.map_or(Value::Na, Value::Int)),
        (
            "start_time",
            start.map_or(Value::Na, |v| model::time(v, 1, i64::from(info.rate))),
        ),
        ("duration_ts", duration.map_or(Value::Na, Value::Int)),
        (
            "duration",
            duration.map_or(Value::Na, |v| model::time(v, 1, i64::from(info.rate))),
        ),
        ("bit_rate", Value::Na),
        ("max_bit_rate", Value::Na),
        (
            "bits_per_raw_sample",
            model::s(reader.text(&info.bits.to_string())?),
        ),
        ("nb_frames", Value::Na),
        ("nb_read_frames", Value::Na),
        ("nb_read_packets", Value::Na),
        ("extradata_size", Value::Int(34)),
    ];
    entries.extend(values);
    let mut format = Vec::new();
    reader.reserve(&mut format, 11, false)?;
    let bit_rate = micros
        .filter(|us| *us > 0)
        .and_then(|us| i64::try_from(u128::from(reader.size) * 8_000_000 / us as u128).ok());
    format.extend([
        ("filename", model::s(filename)),
        ("nb_streams", Value::Int(1)),
        ("nb_programs", Value::Int(0)),
        ("nb_stream_groups", Value::Int(0)),
        ("format_name", model::s(reader.text("flac")?)),
        ("format_long_name", model::s(reader.text("raw FLAC")?)),
        (
            "start_time",
            start.map_or(Value::Na, |v| model::time(v, 1, 1_000_000)),
        ),
        (
            "duration",
            micros.map_or(Value::Na, |v| model::time(v, 1, 1_000_000)),
        ),
        (
            "size",
            Value::Unit(
                i64::try_from(reader.size).map_err(|_| overflow(reader.offset))?,
                Unit::Byte,
            ),
        ),
        (
            "bit_rate",
            bit_rate.map_or(Value::Na, |v| Value::Unit(v, Unit::BitRate)),
        ),
        ("probe_score", Value::Int(100)),
    ]);
    reader.charge(19 * size_of::<(&'static str, Value)>(), false)?;
    let mut streams = Vec::new();
    reader.reserve(&mut streams, 1, false)?;
    streams.push(Stream {
        kind: StreamKind::Audio,
        attached_pic: false,
        entries,
        disposition: iso::disposition(false),
        tags: Vec::new(),
        side_data: Vec::new(),
    });
    let mut time_bases = Vec::new();
    reader.reserve(&mut time_bases, 1, false)?;
    time_bases.push((1, i64::from(info.rate)));
    Ok(Report {
        streams,
        time_bases,
        format,
        format_tags: tags,
        ..Report::default()
    })
}

fn layout<S: Source>(
    channels: u32,
    mask: Option<u32>,
    reader: &mut Reader<'_, S>,
) -> Result<String, Failure> {
    let mask = mask.filter(|m| m.count_ones() == channels);
    let default = match channels {
        1 => "mono",
        2 => "stereo",
        3 => "3.0",
        4 => "quad",
        5 => "5.0(side)",
        6 => "5.1(side)",
        7 => "6.1",
        _ => "7.1",
    };
    let Some(mask) = mask else {
        return reader.text(default);
    };
    let named = match mask {
        0x4 => Some("mono"),
        0x3 => Some("stereo"),
        0xb => Some("2.1"),
        0x7 => Some("3.0"),
        0x103 => Some("3.0(back)"),
        0x107 => Some("4.0"),
        0x33 => Some("quad"),
        0x603 => Some("quad(side)"),
        0xf => Some("3.1"),
        0x37 => Some("5.0"),
        0x607 => Some("5.0(side)"),
        0x10f => Some("4.1"),
        0x3f => Some("5.1"),
        0x60f => Some("5.1(side)"),
        0x707 => Some("6.0"),
        0x6c3 => Some("6.0(front)"),
        0x500f => Some("3.1.2"),
        0x137 => Some("hexagonal"),
        0x70f => Some("6.1"),
        0x13f => Some("6.1(back)"),
        0x6cb => Some("6.1(front)"),
        0x637 => Some("7.0"),
        0x6c7 => Some("7.0(front)"),
        0x63f => Some("7.1"),
        0xff => Some("7.1(wide)"),
        0x6cf => Some("7.1(wide-side)"),
        0x560f => Some("5.1.2"),
        0x503f => Some("5.1.2(back)"),
        0x737 => Some("octagonal"),
        0x2d033 => Some("cube"),
        _ => None,
    };
    if let Some(named) = named {
        return reader.text(named);
    }
    let mut text = reader.text(&format!("{channels} channels ("))?;
    reader.charge(128, false)?;
    let old_capacity = text.capacity();
    text.try_reserve_exact(128)
        .map_err(|_| limit(LimitKind::RetainedBytes, reader.offset))?;
    reader.charge(
        text.capacity()
            .saturating_sub(old_capacity)
            .saturating_sub(128),
        false,
    )?;
    let mut first = true;
    for (bit, name) in [
        "FL", "FR", "FC", "LFE", "BL", "BR", "FLC", "FRC", "BC", "SL", "SR", "TC", "TFL", "TFC",
        "TFR", "TBL", "TBC", "TBR",
    ]
    .into_iter()
    .enumerate()
    {
        if mask & (1 << bit) != 0 {
            if !first {
                text.push('+');
            }
            text.push_str(name);
            first = false;
        }
    }
    text.push(')');
    Ok(text)
}

fn limit(kind: LimitKind, offset: u64) -> Failure {
    Failure::Library(Error::LimitExceeded {
        limit: kind,
        offset,
    })
}

fn overflow(offset: u64) -> Failure {
    Failure::Library(Error::Overflow { offset })
}

#[cfg(test)]
#[path = "flac_tests.rs"]
mod tests;
