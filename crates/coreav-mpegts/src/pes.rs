//! PES packet headers (ISO/IEC 13818-1 2.4.3.6 and 2.4.3.7). Sans-IO.

/// A PES packet's header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PesHeader {
    /// `stream_id`.
    pub stream_id: u8,
    /// `PES_packet_length`: bytes after this field, 0 for unbounded
    /// (allowed for video in transport streams).
    pub packet_length: u16,
    /// Presentation timestamp, 33 bits at 90 kHz.
    pub pts: Option<u64>,
    /// Decoding timestamp, 33 bits at 90 kHz.
    pub dts: Option<u64>,
    /// Bytes from the start code to the first payload byte.
    pub header_len: usize,
}

/// The outcome of reading a PES header from the bytes there are so far.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Parsed {
    Done(PesHeader),
    /// At least this many bytes are needed.
    NeedMore(usize),
    Invalid(&'static str),
}

/// Stream ids whose PES packets have no optional header (table 2-21).
fn bare(stream_id: u8) -> bool {
    matches!(
        stream_id,
        0xBC | 0xBE | 0xBF | 0xF0 | 0xF1 | 0xF2 | 0xF8 | 0xFF
    )
}

/// Reads the PES header at the start of `data`.
pub(crate) fn parse(data: &[u8]) -> Parsed {
    let [p0, p1, p2, stream_id, l0, l1, ..] = *data else {
        return Parsed::NeedMore(6);
    };
    if (p0, p1, p2) != (0, 0, 1) {
        return Parsed::Invalid("a PES packet without its start code");
    }
    let packet_length = u16::from_be_bytes([l0, l1]);
    if bare(stream_id) {
        return Parsed::Done(PesHeader {
            stream_id,
            packet_length,
            pts: None,
            dts: None,
            header_len: 6,
        });
    }
    // The two fixed bits '10' that open the optional header (2.4.3.6) are
    // not checked: MPEG-1 system stream syntax is not used in transport
    // streams, so the optional header is read whatever they hold, as
    // `ffprobe` reads it (docs/design/mpegts.md, "Layers").
    let [_, _, _, _, _, _, _, b7, data_length, ..] = *data else {
        return Parsed::NeedMore(9);
    };
    let header_len = 9 + usize::from(data_length);
    let Some(fields) = data.get(9..header_len) else {
        return Parsed::NeedMore(header_len);
    };
    let (pts, dts) = match b7 >> 6 {
        0b10 => (timestamp(fields), None),
        0b11 => (timestamp(fields), fields.get(5..).and_then(timestamp)),
        0b00 => (None, None),
        _ => return Parsed::Invalid("PTS_DTS_flags of 01"),
    };
    let flags = b7 >> 6;
    if (flags != 0 && pts.is_none()) || (flags == 0b11 && dts.is_none()) {
        return Parsed::Invalid("a PES header too short for its timestamps");
    }
    if packet_length != 0 && usize::from(packet_length) + 6 < header_len {
        return Parsed::Invalid("a PES header longer than its packet");
    }
    Parsed::Done(PesHeader {
        stream_id,
        packet_length,
        pts,
        dts,
        header_len,
    })
}

/// A 33-bit timestamp in its 5-byte field with marker bits (2.4.3.7).
fn timestamp(field: &[u8]) -> Option<u64> {
    let [a, b, c, d, e, ..] = *field else {
        return None;
    };
    Some(
        (u64::from(a >> 1 & 0x07) << 30)
            | (u64::from(b) << 22)
            | (u64::from(c >> 1) << 15)
            | (u64::from(d) << 7)
            | u64::from(e >> 1),
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn ts_field(prefix: u8, t: u64) -> [u8; 5] {
        [
            (prefix << 4) | (((t >> 30) as u8 & 0x07) << 1) | 1,
            (t >> 22) as u8,
            (((t >> 15) as u8) << 1) | 1,
            (t >> 7) as u8,
            ((t as u8) << 1) | 1,
        ]
    }

    #[test]
    fn timestamps() {
        let mut h = vec![0, 0, 1, 0xE0, 0, 0, 0x80, 0xC0, 10];
        h.extend(ts_field(3, 0x1_2345_6789));
        h.extend(ts_field(1, 0x1_2345_0000));
        let Parsed::Done(p) = parse(&h) else { panic!() };
        assert_eq!(
            (p.pts, p.dts, p.header_len),
            (Some(0x1_2345_6789), Some(0x1_2345_0000), 19)
        );
        assert_eq!(parse(&h[..12]), Parsed::NeedMore(19));
        assert_eq!(parse(&h[..4]), Parsed::NeedMore(6));
        let mut bad = h.clone();
        bad[7] = 0x40;
        assert!(matches!(parse(&bad), Parsed::Invalid(_)));
        // The fixed bits before the flags are not checked.
        let mut unmarked = h.clone();
        unmarked[6] = 0x38;
        let Parsed::Done(p) = parse(&unmarked) else {
            panic!()
        };
        assert_eq!((p.pts, p.header_len), (Some(0x1_2345_6789), 19));
        // Padding streams have no optional header.
        let Parsed::Done(p) = parse(&[0, 0, 1, 0xBE, 0, 4, 0xFF]) else {
            panic!()
        };
        assert_eq!(p.header_len, 6);
        assert!(matches!(parse(&[0, 0, 2, 0xE0, 0, 0]), Parsed::Invalid(_)));
    }
}
