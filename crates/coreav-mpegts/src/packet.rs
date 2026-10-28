//! Transport packets (ISO/IEC 13818-1 2.4.3.2 and 2.4.3.4): the 4-byte
//! header and the adaptation field. Sans-IO: each function reads one
//! packet's bytes.

/// The sync byte that starts every transport packet.
pub(crate) const SYNC: u8 = 0x47;
/// Bytes of a transport packet.
pub(crate) const TS_PACKET: usize = 188;
/// The PID of null packets, which carry nothing.
pub(crate) const NULL_PID: u16 = 0x1FFF;

/// How transport packets are stored in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketSize {
    /// 188-byte transport packets (ISO/IEC 13818-1).
    Ts188,
    /// 192 bytes: a 4-byte arrival timestamp before each transport packet
    /// (the BDAV layout of Blu-ray and AVCHD `.m2ts` and `.mts` files).
    Ts192,
    /// 204 bytes: 16 bytes of Reed-Solomon parity after each transport
    /// packet.
    Ts204,
}

impl PacketSize {
    /// Bytes each packet occupies in the file.
    pub fn stride(self) -> u64 {
        match self {
            Self::Ts188 => 188,
            Self::Ts192 => 192,
            Self::Ts204 => 204,
        }
    }

    /// Bytes before the sync byte in each stored packet.
    pub(crate) fn prefix(self) -> u64 {
        match self {
            Self::Ts192 => 4,
            Self::Ts188 | Self::Ts204 => 0,
        }
    }

    /// The candidates, in the order they are tried.
    pub(crate) const ALL: [Self; 3] = [Self::Ts188, Self::Ts192, Self::Ts204];
}

/// The adaptation field's flags and clock reference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Adaptation {
    /// `discontinuity_indicator`: the continuity counter or the time base
    /// is discontinuous at this packet.
    pub discontinuity: bool,
    /// `random_access_indicator`: the next PES packet of this PID starts
    /// at a random access point.
    pub random_access: bool,
    /// The program clock reference in 27 MHz ticks (`base` × 300 +
    /// `extension`), when present.
    pub pcr: Option<u64>,
}

/// One transport packet's header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Header {
    /// `transport_error_indicator`.
    pub error: bool,
    /// `payload_unit_start_indicator`.
    pub unit_start: bool,
    /// The packet identifier.
    pub pid: u16,
    /// `transport_scrambling_control` is not 0.
    pub scrambled: bool,
    /// `continuity_counter`.
    pub counter: u8,
    /// The adaptation field, when present.
    pub adaptation: Option<Adaptation>,
    /// Where the payload starts within the 188 bytes; `None` when the
    /// packet has no payload.
    pub payload: Option<usize>,
}

/// Reads the header of the 188-byte transport packet `packet`.
pub(crate) fn parse(packet: &[u8]) -> Result<Header, &'static str> {
    let [SYNC, b1, b2, b3, ..] = *packet else {
        return Err("a transport packet without its sync byte");
    };
    if packet.len() < TS_PACKET {
        return Err("a transport packet shorter than 188 bytes");
    }
    let pid = (u16::from(b1 & 0x1F) << 8) | u16::from(b2);
    let control = (b3 >> 4) & 0b11;
    let mut header = Header {
        error: b1 & 0x80 != 0,
        unit_start: b1 & 0x40 != 0,
        pid,
        scrambled: b3 & 0xC0 != 0,
        counter: b3 & 0x0F,
        adaptation: None,
        payload: None,
    };
    let mut at = 4usize;
    if control & 0b10 != 0 {
        let length = usize::from(*packet.get(4).ok_or("a truncated adaptation field")?);
        at = 5 + length;
        if at > TS_PACKET {
            return Err("an adaptation field longer than its packet");
        }
        if length > 0 {
            let field = packet.get(5..at).ok_or("a truncated adaptation field")?;
            header.adaptation = Some(adaptation(field)?);
        } else {
            header.adaptation = Some(Adaptation::default());
        }
    }
    // A payload of no bytes (an adaptation field that fills the packet) is
    // no payload.
    if control & 0b01 != 0 && at < TS_PACKET {
        header.payload = Some(at);
    }
    Ok(header)
}

/// Reads an adaptation field after its length byte (2.4.3.4).
fn adaptation(field: &[u8]) -> Result<Adaptation, &'static str> {
    let Some(&flags) = field.first() else {
        return Ok(Adaptation::default());
    };
    let mut out = Adaptation {
        discontinuity: flags & 0x80 != 0,
        random_access: flags & 0x40 != 0,
        pcr: None,
    };
    if flags & 0x10 != 0 {
        let [_, a, b, c, d, e, f, ..] = *field else {
            return Err("an adaptation field too short for its PCR");
        };
        let base = (u64::from(a) << 25)
            | (u64::from(b) << 17)
            | (u64::from(c) << 9)
            | (u64::from(d) << 1)
            | u64::from(e >> 7);
        let extension = (u64::from(e & 1) << 8) | u64::from(f);
        out.pcr = Some(base * 300 + extension);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(header: [u8; 4], rest: &[u8]) -> Vec<u8> {
        let mut p = header.to_vec();
        p.extend_from_slice(rest);
        p.resize(TS_PACKET, 0xFF);
        p
    }

    #[test]
    fn header_fields() {
        let p = packet([0x47, 0x41, 0x00, 0x1A], &[]);
        let h = parse(&p).unwrap();
        assert!(h.unit_start && !h.error && !h.scrambled);
        assert_eq!((h.pid, h.counter, h.payload), (0x100, 0xA, Some(4)));
        assert!(h.adaptation.is_none());
        assert!(parse(&p[..100]).is_err());
        assert!(parse(&packet([0x48, 0, 0, 0x10], &[])).is_err());
    }

    #[test]
    fn adaptation_with_pcr() {
        // PCR base 0x1_0000_0001 (33 bits), extension 299.
        let field = [7u8, 0x50, 0x80, 0x00, 0x00, 0x00, 0x81, 0x2B];
        let p = packet([0x47, 0x01, 0x00, 0x30], &field);
        let h = parse(&p).unwrap();
        let a = h.adaptation.unwrap();
        assert!(a.random_access && !a.discontinuity);
        assert_eq!(a.pcr, Some(0x1_0000_0001 * 300 + 299));
        assert_eq!(h.payload, Some(12));
        // An adaptation field that fills the packet leaves no payload.
        let p = packet([0x47, 0x01, 0x00, 0x30], &[183, 0]);
        assert_eq!(parse(&p).unwrap().payload, None);
        // One that claims more than the packet holds is invalid.
        let p = packet([0x47, 0x01, 0x00, 0x30], &[184, 0]);
        assert!(parse(&p).is_err());
    }
}
