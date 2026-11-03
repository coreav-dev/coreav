mod flac;
mod wav;

use super::args::Container;
use super::{Packet, PacketSource, Probed};
use crate::error::Failure;
use coreav_isobmff::{Error, LimitKind, Limits, Source};

pub(super) fn probe<S: Source>(
    source: &mut S,
    size: u64,
    name: &str,
    force: Option<Container>,
    limits: &Limits,
) -> Result<Option<Probed<'static>>, Failure> {
    if force.is_some_and(|f| !matches!(f, Container::Wav | Container::Flac)) {
        return Ok(None);
    }
    let mut header = [0; 12];
    let count = usize::try_from(size.min(12)).map_err(|_| Failure::Invalid("audio header size"))?;
    let bytes = header
        .get_mut(..count)
        .ok_or(Failure::Invalid("audio header size"))?;
    source.read_exact_at(0, bytes).map_err(Failure::library)?;
    let bytes: &[u8] = bytes;
    let wave = matches!(bytes.get(..4), Some(b"RIFF" | b"RIFX" | b"RF64" | b"BW64"))
        && bytes.get(8..12) == Some(b"WAVE");
    let mut flac_start = bytes.starts_with(b"fLaC").then_some(0);
    if bytes.starts_with(b"ID3") && force != Some(Container::Wav) {
        flac_start = id3_flac(source, size, bytes, limits)?;
    }
    let report = if wave && force != Some(Container::Flac) {
        wav::probe(source, size, name, limits)?
    } else if let Some(start) = flac_start.filter(|_| force != Some(Container::Wav)) {
        flac::probe(source, size, name, limits, start)?
    } else {
        if force.is_some() {
            return Err(Failure::Invalid(
                "input does not match the forced audio format",
            ));
        }
        let unsupported = if bytes.starts_with(b"ID3")
            || matches!(bytes, [0xff, b, c, ..] if b & 0xe0 == 0xe0 && b & 0x18 != 0x08 && b & 0x06 != 0 && c & 0xf0 != 0xf0 && c & 0x0c != 0x0c)
        {
            Some("MP3")
        } else if bytes.starts_with(b"OggS") {
            Some("Ogg")
        } else {
            None
        };
        if let Some(format) = unsupported {
            return Err(Failure::Unsupported(format!(
                "standalone {format} input is not supported by CoreAV yet"
            )));
        }
        return Ok(None);
    };
    Ok(Some(Probed {
        report,
        packets: Box::new(HeaderOnly),
    }))
}

fn id3_flac<S: Source>(
    source: &mut S,
    size: u64,
    header: &[u8],
    limits: &Limits,
) -> Result<Option<u64>, Failure> {
    let Some([version @ (3 | 4), 0, flags, a, b, c, d]) = header.get(3..10) else {
        return Ok(None);
    };
    if (a | b | c | d) & 0x80 != 0 {
        return Ok(None);
    }
    let length =
        (u64::from(*a) << 21) | (u64::from(*b) << 14) | (u64::from(*c) << 7) | u64::from(*d);
    let footer = if *version == 4 && flags & 0x10 != 0 {
        10
    } else {
        0
    };
    let start = 10 + length + footer;
    if start.checked_add(4).is_none_or(|end| end > size) {
        return Ok(None);
    }
    let mut marker = [0; 4];
    source
        .read_exact_at(start, &mut marker)
        .map_err(Failure::library)?;
    if marker != *b"fLaC" {
        return Ok(None);
    }
    if length > limits.max_box_bytes {
        return Err(Failure::Library(Error::LimitExceeded {
            limit: LimitKind::BoxBytes,
            offset: 0,
        }));
    }
    if length != 0 || flags & !0x10 != 0 || (*version == 3 && *flags != 0) {
        return Err(Failure::Unsupported(
            "FLAC with ID3 metadata is not supported".to_owned(),
        ));
    }
    if footer != 0 {
        let mut bytes = [0; 10];
        source
            .read_exact_at(start - 10, &mut bytes)
            .map_err(Failure::library)?;
        if bytes.get(..3) != Some(b"3DI") || bytes.get(3..) != header.get(3..10) {
            return Err(Failure::Invalid("ID3 footer does not match its header"));
        }
    }
    Ok(Some(start))
}

struct HeaderOnly;

impl PacketSource for HeaderOnly {
    fn supports_listing(&self) -> bool {
        false
    }

    fn visit(
        &mut self,
        _payloads: &[bool],
        _f: &mut dyn FnMut(Packet) -> Result<bool, Failure>,
    ) -> Result<(), Failure> {
        Err(Failure::Unsupported(
            "standalone audio packet inspection is not supported".to_owned(),
        ))
    }
}
