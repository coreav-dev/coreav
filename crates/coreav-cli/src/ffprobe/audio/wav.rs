use crate::error::Failure;
use crate::ffprobe::model::{Unit, Value, s, time};
use crate::ffprobe::{Report, Stream, StreamKind, iso, names};
use coreav_isobmff::{Error, LimitKind, Limits, Source};

struct Reader<'a, S> {
    source: &'a mut S,
    size: u64,
    limits: &'a Limits,
    chunks: u64,
    top: u64,
    work: u64,
    retained: u64,
    tables: u64,
}

fn limit(kind: LimitKind, offset: u64) -> Failure {
    Failure::Library(Error::LimitExceeded {
        limit: kind,
        offset,
    })
}

fn add(a: u64, b: u64) -> Result<u64, Failure> {
    a.checked_add(b)
        .ok_or(Failure::Library(Error::Overflow { offset: a }))
}

fn invalid(reason: &'static str) -> Failure {
    Failure::Invalid(reason)
}

fn unsupported(reason: &str) -> Failure {
    Failure::Unsupported(reason.to_owned())
}

fn collected<T, const N: usize>(items: [T; N]) -> Result<Vec<T>, Failure> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(N)
        .map_err(|_| limit(LimitKind::RetainedBytes, 0))?;
    values.extend(items);
    Ok(values)
}

impl<S: Source> Reader<'_, S> {
    fn read<const N: usize>(&mut self, offset: u64) -> Result<[u8; N], Failure> {
        if add(offset, N as u64)? > self.size {
            return Err(Failure::Library(Error::Truncated { offset }));
        }
        let mut bytes = [0; N];
        self.source
            .read_exact_at(offset, &mut bytes)
            .map_err(Failure::library)?;
        Ok(bytes)
    }

    fn word(&mut self, offset: u64) -> Result<u16, Failure> {
        let bytes = self.read(offset)?;
        Ok(u16::from_le_bytes(bytes))
    }

    fn dword(&mut self, offset: u64) -> Result<u32, Failure> {
        let bytes = self.read(offset)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn chunk(&mut self, top: bool, offset: u64) -> Result<(), Failure> {
        self.chunks = add(self.chunks, 1)?;
        if self.chunks > u64::from(self.limits.max_total_boxes) {
            return Err(limit(LimitKind::TotalBoxes, offset));
        }
        if top {
            self.top = add(self.top, 1)?;
            if self.top > u64::from(self.limits.max_top_level_boxes) {
                return Err(limit(LimitKind::TopLevelBoxes, offset));
            }
        }
        self.visit(offset)
    }

    fn visit(&mut self, offset: u64) -> Result<(), Failure> {
        self.work = add(self.work, 1)?;
        if self.work > self.limits.max_sample_work {
            return Err(limit(LimitKind::SampleWork, offset));
        }
        Ok(())
    }

    fn metadata(&self, bytes: u64, offset: u64) -> Result<(), Failure> {
        if bytes > self.limits.max_box_bytes {
            return Err(limit(LimitKind::BoxBytes, offset));
        }
        Ok(())
    }

    fn retain(&mut self, bytes: u64, offset: u64) -> Result<(), Failure> {
        self.retained = add(self.retained, bytes)?;
        if self.retained > self.limits.max_retained_bytes {
            return Err(limit(LimitKind::RetainedBytes, offset));
        }
        self.tables = add(self.tables, bytes)?;
        if self.tables > self.limits.max_table_bytes {
            return Err(limit(LimitKind::TableBytes, offset));
        }
        Ok(())
    }

    fn text(&mut self, offset: u64, len: u64) -> Result<String, Failure> {
        self.metadata(len, offset)?;
        self.retain(len, offset)?;
        let len = usize::try_from(len).map_err(|_| limit(LimitKind::BoxBytes, offset))?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(len)
            .map_err(|_| limit(LimitKind::RetainedBytes, offset))?;
        bytes.resize(len, 0);
        self.source
            .read_exact_at(offset, &mut bytes)
            .map_err(Failure::library)?;
        if let Some(nul) = bytes.iter().position(|b| *b == 0) {
            bytes.truncate(nul);
        }
        String::from_utf8(bytes).map_err(|_| unsupported("non-UTF-8 WAVE INFO text"))
    }
}

struct Format {
    codec: &'static str,
    tag: u16,
    bits: u16,
    channels: u16,
    rate: u32,
    align: u16,
    mask: u32,
    extra: u64,
}

fn format<S: Source>(r: &mut Reader<'_, S>, at: u64, len: u64) -> Result<Format, Failure> {
    r.metadata(len, at)?;
    if len < 16 {
        return Err(invalid("short WAVE format chunk"));
    }
    let tag = r.word(at)?;
    let channels = r.word(add(at, 2)?)?;
    let rate = r.dword(add(at, 4)?)?;
    let bytes_per_second = r.dword(add(at, 8)?)?;
    let align = r.word(add(at, 12)?)?;
    let bits = r.word(add(at, 14)?)?;
    if !matches!(tag, 1 | 3 | 0xfffe) {
        return Err(unsupported("WAVE audio codec"));
    }
    let mut subtag = tag;
    let mut mask = 0;
    let mut extra = 0;
    if len > 16 {
        if len < 18 {
            return Err(invalid("short WAVE format extension"));
        }
        let cb = u64::from(r.word(add(at, 16)?)?);
        if add(18, cb)? > len {
            return Err(invalid("WAVE format extension exceeds chunk"));
        }
        extra = cb;
        if tag == 0xfffe {
            if cb < 22 {
                return Err(invalid("short extensible WAVE format"));
            }
            let valid = r.word(add(at, 18)?)?;
            if valid > bits || bits % 8 != 0 {
                return Err(invalid("invalid extensible WAVE sample precision"));
            }
            mask = r.dword(add(at, 20)?)?;
            let guid: [u8; 16] = r.read(add(at, 24)?)?;
            let tail = guid
                .get(4..)
                .ok_or_else(|| invalid("short WAVE subtype GUID"))?;
            if tail != [0, 0, 16, 0, 128, 0, 0, 170, 0, 56, 155, 113] {
                return Err(unsupported("WAVE subtype GUID"));
            }
            let head = guid
                .get(..4)
                .ok_or_else(|| invalid("short WAVE subtype GUID"))?;
            subtag = match head {
                [1, 0, 0, 0] => 1,
                [3, 0, 0, 0] => 3,
                _ => return Err(unsupported("WAVE subtype GUID")),
            };
            extra -= 22;
        }
    } else if tag == 0xfffe {
        return Err(invalid("missing extensible WAVE format"));
    }
    if channels == 0 || rate == 0 || align == 0 || bits == 0 {
        return Err(invalid("zero WAVE audio format field"));
    }
    let codec = match (subtag, bits) {
        (1, 8) => "pcm_u8",
        (1, 16) => "pcm_s16le",
        (1, 24) => "pcm_s24le",
        (1, 32) => "pcm_s32le",
        (3, 32) => "pcm_f32le",
        (3, 64) => "pcm_f64le",
        _ => return Err(unsupported("WAVE audio codec or sample width")),
    };
    let expected = u32::from(channels) * u32::from(bits / 8);
    if u32::from(align) != expected
        || u64::from(bytes_per_second) != u64::from(rate) * u64::from(align)
    {
        return Err(invalid(
            "inconsistent WAVE PCM block alignment or byte rate",
        ));
    }
    if mask != 0 && mask.count_ones() != u32::from(channels) {
        return Err(invalid("WAVE channel mask disagrees with channel count"));
    }
    Ok(Format {
        codec,
        tag: subtag,
        bits,
        channels,
        rate,
        align,
        mask,
        extra,
    })
}

struct SizeEntry {
    id: [u8; 4],
    len: u64,
    used: bool,
}

struct Sizes {
    riff: u64,
    data: u64,
    entries: Vec<SizeEntry>,
}

fn sizes<S: Source>(r: &mut Reader<'_, S>, at: u64, len: u64) -> Result<Sizes, Failure> {
    r.metadata(len, at)?;
    if len < 28 {
        return Err(invalid("short WAVE ds64 chunk"));
    }
    let riff = u64::from_le_bytes(r.read(at)?);
    let data = u64::from_le_bytes(r.read(add(at, 8)?)?);
    let _: [u8; 8] = r.read(add(at, 16)?)?;
    let count = u32::from_le_bytes(r.read(add(at, 24)?)?);
    let table = u64::from(count) * 12;
    if add(28, table)? != len {
        return Err(invalid("WAVE ds64 table size disagrees with chunk"));
    }
    if add(r.work, u64::from(count))? > r.limits.max_sample_work {
        return Err(limit(LimitKind::SampleWork, at));
    }
    if add(r.chunks, u64::from(count))? > u64::from(r.limits.max_total_boxes) {
        return Err(limit(LimitKind::TotalBoxes, at));
    }
    r.retain(
        u64::from(count) * std::mem::size_of::<SizeEntry>() as u64,
        at,
    )?;
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(usize::try_from(count).map_err(|_| limit(LimitKind::TableBytes, at))?)
        .map_err(|_| limit(LimitKind::TableBytes, at))?;
    let mut pos = add(at, 28)?;
    for _ in 0..count {
        r.chunk(false, pos)?;
        entries.push(SizeEntry {
            id: r.read(pos)?,
            len: u64::from_le_bytes(r.read(add(pos, 4)?)?),
            used: false,
        });
        pos = add(pos, 12)?;
    }
    Ok(Sizes {
        riff,
        data,
        entries,
    })
}

fn info<S: Source>(
    r: &mut Reader<'_, S>,
    at: u64,
    len: u64,
    tags: &mut Vec<(String, String)>,
) -> Result<(), Failure> {
    r.metadata(len, at)?;
    if r.limits.max_depth < 2 {
        return Err(limit(LimitKind::Depth, at));
    }
    if len < 4 {
        return Err(invalid("short WAVE LIST chunk"));
    }
    if r.read::<4>(at)? != *b"INFO" {
        return Err(unsupported("WAVE LIST type"));
    }
    let end = add(at, len)?;
    let mut pos = add(at, 4)?;
    while pos < end {
        if add(pos, 8)? > end {
            return Err(invalid("short WAVE INFO entry header"));
        }
        r.chunk(false, pos)?;
        let id: [u8; 4] = r.read(pos)?;
        let len = u64::from(r.dword(add(pos, 4)?)?);
        let start = add(pos, 8)?;
        let next = add(add(start, len)?, len & 1)?;
        if next > end {
            return Err(invalid("WAVE INFO entry exceeds LIST"));
        }
        r.retain((std::mem::size_of::<(String, String)>() + 32) as u64, pos)?;
        let value = r.text(start, len)?;
        if !value.is_empty() {
            let alias = match &id {
                b"IART" => Some("artist"),
                b"ICMT" => Some("comment"),
                b"ICOP" => Some("copyright"),
                b"ICRD" => Some("date"),
                b"IGNR" => Some("genre"),
                b"ILNG" => Some("language"),
                b"INAM" => Some("title"),
                b"IPRD" => Some("album"),
                b"IPRT" | b"ITRK" => Some("track"),
                b"ISFT" => Some("encoder"),
                _ => None,
            };
            let raw =
                std::str::from_utf8(&id).map_err(|_| unsupported("non-UTF-8 WAVE INFO key"))?;
            if !id.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
                return Err(unsupported("non-text WAVE INFO key"));
            }
            let key = alias.unwrap_or(raw);
            let mut existing = None;
            for (index, (stored, _)) in tags.iter().enumerate() {
                r.visit(pos)?;
                if stored.eq_ignore_ascii_case(key) {
                    existing = Some(index);
                    break;
                }
            }
            if let Some(existing) = existing {
                tags.swap_remove(existing);
            }
            tags.try_reserve_exact(1)
                .map_err(|_| limit(LimitKind::RetainedBytes, pos))?;
            let mut owned = String::new();
            owned
                .try_reserve_exact(key.len())
                .map_err(|_| limit(LimitKind::RetainedBytes, pos))?;
            owned.push_str(key);
            tags.push((owned, value));
        }
        pos = next;
    }
    Ok(())
}

fn layout(mask: u32) -> Result<Value, Failure> {
    let name = match mask {
        0 => return Ok(Value::Unset("unknown")),
        0x4 => "mono",
        0x3 => "stereo",
        0xb => "2.1",
        0x7 => "3.0",
        0x103 => "3.0(back)",
        0x107 => "4.0",
        0x33 => "quad",
        0x603 => "quad(side)",
        0xf => "3.1",
        0x37 => "5.0",
        0x607 => "5.0(side)",
        0x10f => "4.1",
        0x3f => "5.1",
        0x60f => "5.1(side)",
        0x707 => "6.0",
        0x6c3 => "6.0(front)",
        0x500f => "3.1.2",
        0x137 => "hexagonal",
        0x13f => "6.1(back)",
        0x70f => "6.1",
        0x6cb => "6.1(front)",
        0x637 => "7.0",
        0x6c7 => "7.0(front)",
        0x63f => "7.1",
        0xff => "7.1(wide)",
        0x6cf => "7.1(wide-side)",
        0x560f => "5.1.2",
        0x503f => "5.1.2(back)",
        0x737 => "octagonal",
        0x2d033 => "cube",
        0x2d60f => "5.1.4",
        0x563f => "7.1.2",
        0x2d63f => "7.1.4",
        0x2d6ff => "9.1.4",
        _ => {
            if mask & !0x3ffff != 0 {
                return Err(unsupported("WAVE speaker mask"));
            }
            let mut text = format!("{} channels (", mask.count_ones());
            for (bit, name) in [
                "FL", "FR", "FC", "LFE", "BL", "BR", "FLC", "FRC", "BC", "SL", "SR", "TC", "TFL",
                "TFC", "TFR", "TBL", "TBC", "TBR",
            ]
            .into_iter()
            .enumerate()
            {
                if mask & (1 << bit) != 0 {
                    if !text.ends_with('(') {
                        text.push('+');
                    }
                    text.push_str(name);
                }
            }
            text.push(')');
            return Ok(s(text));
        }
    };
    Ok(s(name))
}

pub(super) fn probe<S: Source>(
    source: &mut S,
    size: u64,
    name: &str,
    limits: &Limits,
) -> Result<Report, Failure> {
    if limits.max_tracks == 0 {
        return Err(limit(LimitKind::Tracks, 0));
    }
    if limits.max_depth == 0 {
        return Err(limit(LimitKind::Depth, 0));
    }
    let mut r = Reader {
        source,
        size,
        limits,
        chunks: 0,
        top: 0,
        work: 0,
        retained: 0,
        tables: 0,
    };
    r.retain(add(name.len() as u64, 8192)?, 0)?;
    let mut filename = String::new();
    filename
        .try_reserve_exact(name.len())
        .map_err(|_| limit(LimitKind::RetainedBytes, 0))?;
    filename.push_str(name);
    let magic: [u8; 4] = r.read(0)?;
    if magic == *b"RIFX" {
        return Err(unsupported("RIFX sample byte order"));
    }
    let wide = matches!(&magic, b"RF64" | b"BW64");
    if !matches!(&magic, b"RIFF" | b"RIFX" | b"RF64" | b"BW64") || r.read::<4>(8)? != *b"WAVE" {
        return Err(unsupported("not a WAVE file"));
    }
    let declared = r.dword(4)?;
    if wide && declared != u32::MAX {
        return Err(invalid("RF64 or BW64 requires a placeholder RIFF size"));
    }
    if !wide && declared < 4 {
        return Err(invalid("short WAVE RIFF extent"));
    }
    let mut end = if wide {
        size
    } else {
        add(u64::from(declared), 8)?
    };
    let mut pos = 12;
    let mut fmt = None;
    let mut ds64: Option<Sizes> = None;
    let mut data = None;
    let mut tags = Vec::new();
    while pos < end.min(size) {
        if add(pos, 8)? > end.min(size) {
            return Err(invalid("short WAVE chunk header"));
        }
        r.chunk(true, pos)?;
        let id: [u8; 4] = r.read(pos)?;
        let declared = r.dword(add(pos, 4)?)?;
        if wide && ds64.is_none() && id != *b"ds64" {
            return Err(invalid("missing mandatory leading WAVE ds64 chunk"));
        }
        let mut len = u64::from(declared);
        if declared == u32::MAX {
            let sizes = ds64
                .as_mut()
                .ok_or_else(|| invalid("WAVE size placeholder without ds64"))?;
            if id == *b"data" && data.is_none() {
                len = sizes.data;
            } else {
                let mut entry = None;
                for candidate in &mut sizes.entries {
                    r.visit(pos)?;
                    if candidate.id == id && !candidate.used {
                        entry = Some(candidate);
                        break;
                    }
                }
                let entry =
                    entry.ok_or_else(|| invalid("WAVE size placeholder without ds64 entry"))?;
                entry.used = true;
                len = entry.len;
            }
        }
        let start = add(pos, 8)?;
        let payload_end = add(start, len)?;
        let next = add(payload_end, len & 1)?;
        if id != *b"data" && payload_end > end && end <= size {
            return Err(invalid("WAVE chunk exceeds declared RIFF extent"));
        }
        if id != *b"data" && payload_end > end.min(size) {
            return Err(Failure::Library(Error::Truncated { offset: start }));
        }
        match &id {
            b"fmt " => {
                if fmt.is_some() || data.is_some() {
                    return Err(invalid("duplicate or late WAVE format chunk"));
                }
                fmt = Some(format(&mut r, start, len)?);
            }
            b"ds64" if wide => {
                if ds64.is_some() {
                    return Err(invalid("duplicate WAVE ds64 chunk"));
                }
                let sizes = sizes(&mut r, start, len)?;
                end = add(sizes.riff, 8)?;
                if end < next {
                    return Err(invalid("WAVE ds64 RIFF extent is too small"));
                }
                ds64 = Some(sizes);
            }
            b"data" => {
                if fmt.is_none() {
                    return Err(invalid("WAVE data precedes its format"));
                }
                if data.is_some() {
                    return Err(unsupported("multiple WAVE data chunks"));
                }
                if start > end.min(size) {
                    return Err(invalid("WAVE data starts outside RIFF"));
                }
                data = Some(len.min(end.min(size) - start));
            }
            b"LIST" => info(&mut r, start, len, &mut tags)?,
            b"fact" => {
                r.metadata(len, start)?;
                if len < 4 {
                    return Err(invalid("short WAVE fact chunk"));
                }
                let _: [u8; 4] = r.read(start)?;
                if len > 4 {
                    return Err(unsupported("extended WAVE fact metadata"));
                }
            }
            b"JUNK" | b"PAD " | b"FLLR" => {}
            _ => return Err(unsupported("WAVE chunk metadata")),
        }
        if id == *b"data" && payload_end > end.min(size) {
            break;
        }
        if next > end.min(size) && id != *b"data" {
            return Err(Failure::Library(Error::Truncated {
                offset: payload_end,
            }));
        }
        pos = next;
    }
    if wide && ds64.is_none() {
        return Err(invalid("missing WAVE ds64 chunk"));
    }
    if end < size {
        return Err(unsupported("bytes after the declared WAVE RIFF extent"));
    }
    let fmt = fmt.ok_or_else(|| invalid("missing WAVE format chunk"))?;
    let bytes = data.ok_or_else(|| invalid("missing WAVE data chunk"))?;
    if bytes % u64::from(fmt.align) != 0 {
        return Err(unsupported(
            "WAVE data ends within an interleaved sample frame",
        ));
    }
    let samples = i64::try_from(bytes / u64::from(fmt.align))
        .map_err(|_| invalid("WAVE duration exceeds timestamp range"))?;
    let duration = if bytes == 0 {
        Value::Na
    } else {
        time(samples, 1, i64::from(fmt.rate))
    };
    let bit_rate = i64::from(fmt.rate) * i64::from(fmt.channels) * i64::from(fmt.bits);
    let mut entries = collected([
        ("index", Value::Int(0)),
        ("codec_name", s(fmt.codec)),
        (
            "codec_long_name",
            s(names::long_name(fmt.codec).ok_or_else(|| unsupported("WAVE codec name"))?),
        ),
        ("profile", Value::Unset("unknown")),
        ("codec_type", s("audio")),
        ("codec_tag_string", s(format!("[{}][0][0][0]", fmt.tag))),
        ("codec_tag", s(format!("0x{:04x}", fmt.tag))),
        (
            "sample_fmt",
            s(names::sample_fmt(fmt.codec, Some(fmt.bits))
                .ok_or_else(|| unsupported("WAVE sample format"))?),
        ),
        ("sample_rate", Value::Unit(i64::from(fmt.rate), Unit::Hertz)),
        ("channels", Value::Int(i64::from(fmt.channels))),
        ("channel_layout", layout(fmt.mask)?),
        ("bits_per_sample", Value::Int(i64::from(fmt.bits))),
        ("initial_padding", Value::Int(0)),
        ("id", Value::Na),
        ("r_frame_rate", s("0/0")),
        ("avg_frame_rate", s("0/0")),
        ("time_base", s(format!("1/{}", fmt.rate))),
        ("start_pts", Value::Na),
        ("start_time", Value::Na),
        (
            "duration_ts",
            if bytes == 0 {
                Value::Na
            } else {
                Value::Int(samples)
            },
        ),
        ("duration", duration.clone()),
        ("bit_rate", Value::Unit(bit_rate, Unit::BitRate)),
        ("max_bit_rate", Value::Na),
        (
            "bits_per_raw_sample",
            if fmt.tag == 1 && fmt.bits >= 24 {
                s(fmt.bits.to_string())
            } else {
                Value::Na
            },
        ),
        ("nb_frames", Value::Na),
        ("nb_read_frames", Value::Na),
        ("nb_read_packets", Value::Na),
    ])?;
    if fmt.extra != 0 {
        entries
            .try_reserve_exact(1)
            .map_err(|_| limit(LimitKind::RetainedBytes, 0))?;
        entries.push((
            "extradata_size",
            Value::Int(
                i64::try_from(fmt.extra).map_err(|_| invalid("WAVE extension is too large"))?,
            ),
        ));
    }
    let micros = (u128::try_from(samples).map_err(|_| invalid("negative WAVE duration"))?
        * 1_000_000
        + u128::from(fmt.rate / 2))
        / u128::from(fmt.rate);
    let file_rate = match (u128::from(size) * 8_000_000).checked_div(micros) {
        Some(rate) => Value::Unit(
            i64::try_from(rate).map_err(|_| invalid("WAVE file bit rate exceeds report range"))?,
            Unit::BitRate,
        ),
        None => Value::Unit(bit_rate, Unit::BitRate),
    };
    #[allow(clippy::cast_precision_loss)]
    let format_duration = if micros == 0 {
        Value::Na
    } else {
        Value::Time(micros as f64 / 1_000_000.0)
    };
    Ok(Report {
        streams: collected([Stream {
            kind: StreamKind::Audio,
            attached_pic: false,
            entries,
            disposition: iso::disposition(false),
            tags: Vec::new(),
            side_data: Vec::new(),
        }])?,
        time_bases: collected([(1, i64::from(fmt.rate))])?,
        format: collected([
            ("filename", s(filename)),
            ("nb_streams", Value::Int(1)),
            ("nb_programs", Value::Int(0)),
            ("nb_stream_groups", Value::Int(0)),
            ("format_name", s("wav")),
            ("format_long_name", s("WAV / WAVE (Waveform Audio)")),
            ("start_time", Value::Na),
            ("duration", format_duration),
            (
                "size",
                Value::Unit(
                    i64::try_from(size).map_err(|_| invalid("WAVE size exceeds report range"))?,
                    Unit::Byte,
                ),
            ),
            ("bit_rate", file_rate),
            ("probe_score", Value::Int(if wide { 100 } else { 99 })),
        ])?,
        format_tags: tags,
        ..Report::default()
    })
}

#[cfg(test)]
#[path = "wav_tests.rs"]
mod tests;
