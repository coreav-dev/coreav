//! Program specific information (ISO/IEC 13818-1 2.4.4): sections
//! reassembled from transport packet payloads, their CRC-32, and the
//! Program Association and Program Map Tables. Sans-IO.

/// `table_id` of the Program Association Table.
pub(crate) const PAT: u8 = 0x00;
/// `table_id` of a Program Map Table section.
pub(crate) const PMT: u8 = 0x02;
/// The Event Information Table: its PID, and the `table_id`s it uses
/// (ETSI EN 300 468 table 2: 0x4E and 0x4F for the present and following
/// events, 0x50 to 0x6F for the schedule).
pub(crate) const EIT_PID: u16 = 0x12;
pub(crate) const EIT_FIRST: u8 = 0x4E;
pub(crate) const EIT_LAST: u8 = 0x6F;
/// The Service Description Table: its PID, and the `table_id` of the
/// sections that describe the services of this transport stream (ETSI EN
/// 300 468 5.2.3 and table 2).
pub(crate) const SDT_PID: u16 = 0x11;
pub(crate) const SDT_ACTUAL: u8 = 0x42;
/// `descriptor_tag` of the service descriptor (ETSI EN 300 468 6.2.33).
const SERVICE_DESCRIPTOR: u8 = 0x48;
/// Most bytes of a PSI section of the tables read here (2.4.4.4: a
/// `section_length` of at most 1,021).
pub(crate) const MAX_SECTION: usize = 1_024;
/// Most bytes of an Event Information Table section (ETSI EN 300 468
/// 5.2.4: a `section_length` of at most 4,093). Only its `table_id` is
/// read, but a section is reassembled whole so that its CRC-32 vouches
/// for that byte.
pub(crate) const MAX_EIT_SECTION: usize = 4_096;

/// Reassembles the sections of one PID from its packets' payloads.
#[derive(Debug)]
pub(crate) struct Assembler {
    buffer: Vec<u8>,
    /// A section has started and is not complete.
    open: bool,
    /// Most bytes of a section of this PID's table.
    max: usize,
}

impl Assembler {
    /// An assembler for sections of at most `max` bytes.
    pub(crate) fn new(max: usize) -> Self {
        Self {
            buffer: Vec::new(),
            open: false,
            max,
        }
    }

    /// Takes the payload of the next packet of the PID and returns the
    /// sections it completes. A section longer than this PID's bound is
    /// dropped with an error; the next unit start resynchronises.
    pub(crate) fn push(
        &mut self,
        payload: &[u8],
        unit_start: bool,
        out: &mut Vec<Vec<u8>>,
    ) -> Result<(), &'static str> {
        if !unit_start {
            if self.open {
                self.buffer.extend_from_slice(payload);
                self.complete(out)?;
            }
            return Ok(());
        }
        let (&pointer, rest) = payload.split_first().ok_or("an empty PSI payload")?;
        let pointer = usize::from(pointer);
        let (tail, fresh) = (rest.get(..pointer), rest.get(pointer..));
        let (Some(tail), Some(fresh)) = (tail, fresh) else {
            self.reset();
            return Err("a pointer field past its packet");
        };
        // The bytes before the pointer end the open section, if any.
        if self.open {
            self.buffer.extend_from_slice(tail);
            self.complete(out)?;
        }
        self.reset();
        // Sections that start in this packet: one after the other, until
        // stuffing (a `table_id` of 0xFF) or the end of the payload.
        let mut at = fresh;
        while let Some(&table_id) = at.first() {
            if table_id == 0xFF {
                break;
            }
            let Some(total) = section_total(at) else {
                // The section header itself continues in the next packet.
                self.buffer.extend_from_slice(at);
                self.open = true;
                return Ok(());
            };
            if total > self.max {
                return Err("a PSI section longer than its table allows");
            }
            match at.get(..total) {
                Some(section) => {
                    out.push(section.to_vec());
                    at = at.get(total..).unwrap_or_default();
                }
                None => {
                    self.buffer.extend_from_slice(at);
                    self.open = true;
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.buffer.clear();
        self.open = false;
    }

    /// Emits the open section once it is whole.
    fn complete(&mut self, out: &mut Vec<Vec<u8>>) -> Result<(), &'static str> {
        let Some(total) = section_total(&self.buffer) else {
            return Ok(());
        };
        if total > self.max {
            self.reset();
            return Err("a PSI section longer than its table allows");
        }
        if let Some(section) = self.buffer.get(..total) {
            out.push(section.to_vec());
            self.reset();
        }
        Ok(())
    }
}

/// Bytes of the section that starts `data`, once its 3-byte header is
/// there.
fn section_total(data: &[u8]) -> Option<usize> {
    let [_, b1, b2, ..] = *data else {
        return None;
    };
    Some(3 + ((usize::from(b1 & 0x0F) << 8) | usize::from(b2)))
}

/// The CRC-32 of ISO/IEC 13818-1 Annex A (polynomial 0x04C11DB7, initial
/// value all ones, no reflection, no final XOR). Over a whole section with
/// its `CRC_32` field, the result is 0.
pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in data {
        crc ^= u32::from(byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// The fields every long-form section shares (2.4.4.4 and 2.4.4.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Long {
    pub table_id: u8,
    /// `transport_stream_id` (PAT) or `program_number` (PMT).
    pub id: u16,
    pub version: u8,
    pub current: bool,
    pub number: u8,
    pub last: u8,
}

/// Checks a long-form section (syntax indicator 1, CRC-32) and splits it
/// into its header and its body (between the header and the CRC).
pub(crate) fn long(section: &[u8]) -> Result<(Long, &[u8]), &'static str> {
    let [table_id, b1, _, i0, i1, v, number, last, ..] = *section else {
        return Err("a PSI section shorter than its header");
    };
    if b1 & 0x80 == 0 {
        return Err("a PSI section without the long syntax");
    }
    if section.len() < 12 {
        return Err("a PSI section shorter than its header and CRC");
    }
    if crc32(section) != 0 {
        return Err("a PSI section whose CRC-32 does not match");
    }
    let body = section
        .get(8..section.len() - 4)
        .ok_or("a PSI section shorter than its header and CRC")?;
    Ok((
        Long {
            table_id,
            id: u16::from_be_bytes([i0, i1]),
            version: (v >> 1) & 0x1F,
            current: v & 1 != 0,
            number,
            last,
        },
        body,
    ))
}

/// The entries of a PAT section body: (`program_number`, PID). Program 0
/// names the network PID and is left out.
pub(crate) fn pat_entries(body: &[u8]) -> Vec<(u16, u16)> {
    body.chunks_exact(4)
        .filter_map(|e| match *e {
            [p0, p1, d0, d1] => {
                let program = u16::from_be_bytes([p0, p1]);
                let pid = (u16::from(d0 & 0x1F) << 8) | u16::from(d1);
                (program != 0).then_some((program, pid))
            }
            _ => None,
        })
        .collect()
}

/// One descriptor (2.6): its tag and body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Descriptor {
    /// `descriptor_tag`.
    pub tag: u8,
    /// The bytes after `descriptor_length`.
    pub data: Vec<u8>,
}

/// One elementary stream of a PMT.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PmtStream {
    pub stream_type: u8,
    pub pid: u16,
    pub descriptors: Vec<Descriptor>,
}

/// A PMT section body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pmt {
    pub pcr_pid: u16,
    pub descriptors: Vec<Descriptor>,
    pub streams: Vec<PmtStream>,
}

/// Reads a PMT section body (2.4.4.9).
pub(crate) fn pmt(body: &[u8]) -> Result<Pmt, &'static str> {
    let [p0, p1, l0, l1, ..] = *body else {
        return Err("a PMT shorter than its header");
    };
    let pcr_pid = (u16::from(p0 & 0x1F) << 8) | u16::from(p1);
    let info = (usize::from(l0 & 0x0F) << 8) | usize::from(l1);
    let rest = body.get(4..).unwrap_or_default();
    let (program, mut rest) = (
        rest.get(..info)
            .ok_or("program_info_length past its section")?,
        rest.get(info..).unwrap_or_default(),
    );
    let program_descriptors = descriptors(program)?;
    let mut streams = Vec::new();
    while !rest.is_empty() {
        let [stream_type, d0, d1, e0, e1, ..] = *rest else {
            return Err("an elementary stream entry past its section");
        };
        let length = (usize::from(e0 & 0x0F) << 8) | usize::from(e1);
        let es = rest
            .get(5..5 + length)
            .ok_or("ES_info_length past its section")?;
        streams.push(PmtStream {
            stream_type,
            pid: (u16::from(d0 & 0x1F) << 8) | u16::from(d1),
            descriptors: descriptors(es)?,
        });
        rest = rest.get(5 + length..).unwrap_or_default();
    }
    Ok(Pmt {
        pcr_pid,
        descriptors: program_descriptors,
        streams,
    })
}

/// One service of a Service Description Table: its `service_id`, which
/// is the `program_number` of the program that carries it, and the
/// provider and service names of its service descriptor, as stored (a DVB
/// string, ETSI EN 300 468 annex A: its first byte may name a character
/// table).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    /// `service_id`.
    pub id: u16,
    /// `service_provider_name`, as stored.
    pub provider: Vec<u8>,
    /// `service_name`, as stored.
    pub name: Vec<u8>,
}

/// The services of a Service Description Table section body (ETSI EN 300
/// 468 5.2.3): those with a service descriptor, in the section's order. A
/// loop cut short ends the list.
pub(crate) fn sdt_services(body: &[u8]) -> Vec<Service> {
    let mut out = Vec::new();
    // original_network_id and a reserved byte.
    let mut rest = body.get(3..).unwrap_or_default();
    while let [i0, i1, _, l0, l1, ..] = *rest {
        let length = (usize::from(l0 & 0x0F) << 8) | usize::from(l1);
        let Some(loop_bytes) = rest.get(5..5 + length) else {
            break;
        };
        rest = rest.get(5 + length..).unwrap_or_default();
        let Ok(list) = descriptors(loop_bytes) else {
            continue;
        };
        let Some(service) = list.iter().find(|d| d.tag == SERVICE_DESCRIPTOR) else {
            continue;
        };
        // service_type, then two length-prefixed strings.
        let data = service.data.as_slice();
        let provider_len = usize::from(data.get(1).copied().unwrap_or(0));
        let provider = data.get(2..2 + provider_len);
        let name_len = data.get(2 + provider_len).copied().map(usize::from);
        let name = name_len.and_then(|n| data.get(3 + provider_len..3 + provider_len + n));
        if let (Some(provider), Some(name)) = (provider, name) {
            out.push(Service {
                id: u16::from_be_bytes([i0, i1]),
                provider: provider.to_vec(),
                name: name.to_vec(),
            });
        }
    }
    out
}

/// Reads a descriptor loop.
fn descriptors(mut data: &[u8]) -> Result<Vec<Descriptor>, &'static str> {
    let mut out = Vec::new();
    while !data.is_empty() {
        let [tag, length, ..] = *data else {
            return Err("a descriptor shorter than its header");
        };
        let end = 2 + usize::from(length);
        let body = data.get(2..end).ok_or("a descriptor past its loop")?;
        out.push(Descriptor {
            tag,
            data: body.to_vec(),
        });
        data = data.get(end..).unwrap_or_default();
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A long-form section with its CRC.
    pub(crate) fn section(table_id: u8, id: u16, version: u8, body: &[u8]) -> Vec<u8> {
        let length = 5 + body.len() + 4;
        let mut s = vec![table_id, 0xB0 | ((length >> 8) as u8 & 0x0F), length as u8];
        s.extend(id.to_be_bytes());
        s.extend([0xC1 | (version << 1), 0, 0]);
        s.extend(body);
        let crc = crc32(&s);
        s.extend(crc.to_be_bytes());
        s
    }

    #[test]
    fn crc_of_a_section_with_its_crc_is_zero() {
        let s = section(PAT, 1, 0, &[0, 1, 0xF0, 0x00]);
        assert_eq!(crc32(&s), 0);
        let (long, body) = long(&s).unwrap();
        assert_eq!((long.table_id, long.id, long.current), (PAT, 1, true));
        assert_eq!(pat_entries(body), vec![(1, 0x1000)]);
        let mut bad = s.clone();
        bad[9] ^= 1;
        assert!(super::long(&bad).is_err());
    }

    #[test]
    fn sections_across_packets_and_several_in_one() {
        let a = section(PAT, 1, 0, &[0, 1, 0xF0, 0x00]);
        let body: Vec<u8> = (0..200u16).flat_map(|i| [0, i as u8, 0xE0, 0x10]).collect();
        let b = section(PAT, 2, 0, &body);
        let mut assembler = Assembler::new(MAX_SECTION);
        let mut out = Vec::new();
        // Two sections in one unit start, the second running on.
        let mut payload = vec![0u8];
        payload.extend(&a);
        payload.extend(&b[..100]);
        assembler.push(&payload, true, &mut out).unwrap();
        assert_eq!(out, vec![a.clone()]);
        for chunk in b[100..652].chunks(184) {
            assembler.push(chunk, false, &mut out).unwrap();
        }
        assert_eq!(out.len(), 1);
        // The rest arrives before the next section's pointer.
        let mut payload = vec![(b.len() - 652) as u8];
        payload.extend(&b[652..]);
        payload.extend(&a);
        assembler.push(&payload, true, &mut out).unwrap();
        assert_eq!(out, vec![a.clone(), b, a]);
        // A pointer past the packet is an error, and nothing is kept.
        assert!(assembler.push(&[200, 1, 2], true, &mut out).is_err());
    }

    #[test]
    fn pmt_streams_and_descriptors() {
        let mut body = vec![0xE1, 0x00, 0xF0, 0x06, 0x05, 0x04];
        body.extend(b"HDMV");
        body.extend([0x1B, 0xE1, 0x00, 0xF0, 0x00]);
        body.extend([0x06, 0xE1, 0x01, 0xF0, 0x03, 0x6A, 0x01, 0x00]);
        let pmt = pmt(&body).unwrap();
        assert_eq!(pmt.pcr_pid, 0x100);
        assert_eq!(pmt.descriptors[0].data, b"HDMV");
        assert_eq!(pmt.streams.len(), 2);
        assert_eq!((pmt.streams[1].stream_type, pmt.streams[1].pid), (6, 0x101));
        assert_eq!(pmt.streams[1].descriptors[0].tag, 0x6A);
        assert!(super::pmt(&body[..body.len() - 1]).is_err());
    }

    #[test]
    fn sdt_services_with_their_names() {
        // original_network_id, reserved, then one service with a service
        // descriptor and one without.
        let mut body = vec![0xFF, 0x01, 0xFF];
        let descriptor = [
            0x48, 0x0A, 0x01, 0x03, b'P', b'r', b'v', 0x04, b'N', b'a', b'm', b'e',
        ];
        body.extend([0x00, 0x01, 0xFC, 0x80, descriptor.len() as u8]);
        body.extend(descriptor);
        body.extend([0x00, 0x02, 0xFC, 0x80, 0x00]);
        let services = sdt_services(&body);
        assert_eq!(
            services,
            vec![Service {
                id: 1,
                provider: b"Prv".to_vec(),
                name: b"Name".to_vec(),
            }]
        );
        // A loop cut short ends the list without a panic.
        assert!(sdt_services(&body[..10]).is_empty());
    }
}
