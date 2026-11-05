use super::tstd::{Bounds, F, Leak, PCR_GAP, SYS_DRAIN};
use super::{Au, Limits, Work, limit, reserve, unsupported};
use crate::ExtractError;
use coreav_isobmff::{LimitKind, PlanningBudget};

enum Kind {
    Clock,
    Table(usize),
    Video {
        unit: usize,
        offset: usize,
        len: usize,
    },
}

struct Packet {
    clock: u64,
    pcr: bool,
    kind: Kind,
}

struct Plan<'a> {
    packets: Vec<Packet>,
    clock: u64,
    step: u64,
    last_pcr: u64,
    limits: &'a Limits,
    planning: &'a PlanningBudget,
    work: &'a mut Work,
}

impl Plan<'_> {
    fn push(&mut self, kind: Kind, pcr: bool) -> Result<(), ExtractError> {
        self.work.charge(1)?;
        if self.packets.len() >= self.limits.max_output_bytes / 188
            || self.clock >= (1u64 << 33) * 300
        {
            return Err(limit(LimitKind::PacketBytes));
        }
        self.planning
            .reserve(&mut self.packets, 1)
            .map_err(|_| limit(LimitKind::PlanningBytes))?;
        self.packets.push(Packet {
            clock: self.clock,
            pcr,
            kind,
        });
        if pcr {
            self.last_pcr = self.clock;
        }
        self.clock = self
            .clock
            .checked_add(self.step)
            .ok_or_else(|| unsupported("transport clock overflow"))?;
        Ok(())
    }

    fn idle(&mut self, until: u64) -> Result<(), ExtractError> {
        if !self
            .packets
            .last()
            .is_some_and(|packet| matches!(packet.kind, Kind::Clock))
        {
            self.push(Kind::Clock, true)?;
        }
        self.clock = until.max(self.clock).min(self.last_pcr + PCR_GAP / 2);
        self.push(Kind::Clock, true)
    }
}

pub(super) fn serialize(
    units: &[Au],
    bounds: Bounds,
    limits: &Limits,
    planning: &PlanningBudget,
    work: &mut Work,
) -> Result<Vec<u8>, ExtractError> {
    let step = bounds.packet_ticks();
    let mut plan = Plan {
        packets: Vec::new(),
        clock: step,
        step,
        last_pcr: 0,
        limits,
        planning,
        work,
    };
    plan.push(Kind::Clock, true)?;
    let mut tables: [Option<u64>; 3] = [None; 3];
    let mut next_system = 0;
    let mut initial_ready = u64::MAX;
    let mut index = 0usize;
    let mut offset = 0usize;
    let mut release = 0usize;
    let mut reserved = 0u128;
    let mut leak = Leak::default();
    while let Some(au) = units.get(index) {
        let earliest = plan.clock.saturating_sub(step);
        while release < index
            && units
                .get(release)
                .is_some_and(|au| au.dts90 * 300 <= earliest)
        {
            let old = units
                .get(release)
                .ok_or_else(|| unsupported("missing reserved access unit"))?;
            reserved = reserved
                .checked_sub(old.data.len() as u128 * 8)
                .ok_or_else(|| unsupported("transport reservation accounting"))?;
            release += 1;
        }
        let deadline = au.dts90 * 300;
        if plan.clock >= deadline {
            return Err(unsupported("MPEG-TS schedule misses a decode deadline"));
        }
        let pcr_due = plan.clock - plan.last_pcr >= PCR_GAP.saturating_sub(step);
        let mut selected_table = None;
        if !pcr_due {
            for (table, previous) in tables.iter().enumerate() {
                let period = if table == 2 { F } else { 2 * F / 5 };
                if previous.is_none_or(|previous| plan.clock >= previous + period)
                    && (table == 2 || plan.clock >= next_system)
                {
                    selected_table = Some(table);
                    break;
                }
            }
        }
        if let Some(table) = selected_table {
            let clock = plan.clock;
            plan.push(Kind::Table(table), false)?;
            if table < 2 {
                next_system = clock + step + SYS_DRAIN;
                if table == 1 && initial_ready == u64::MAX {
                    initial_ready = next_system;
                }
            }
            if let Some(previous) = tables.get_mut(table) {
                *previous = Some(clock);
            }
            continue;
        }
        let beginning = offset == 0;
        let header_len: usize = if au.pts90 == au.dts90 { 14 } else { 19 };
        let total = header_len
            .checked_add(au.data.len())
            .ok_or_else(|| limit(LimitKind::PacketBytes))?;
        let overhead = if pcr_due {
            8
        } else if beginning && au.idr {
            2
        } else {
            0
        };
        let take = total
            .checked_sub(offset)
            .map(|remaining| remaining.min(184 - overhead))
            .ok_or_else(|| unsupported("video packet offset"))?;
        let mut ready = plan.clock >= initial_ready;
        let mut wake = plan.clock + PCR_GAP / 2;
        if !ready {
            wake = wake
                .min(initial_ready)
                .min(next_system.max(plan.clock + step));
        }
        if beginning {
            if au.data.len() as u128 * 8 + 8 > u128::from(bounds.eb) {
                return Err(unsupported(
                    "AVC access unit exceeds elementary-buffer capacity",
                ));
            }
            if reserved + au.data.len() as u128 * 8 + 8 > u128::from(bounds.eb) {
                ready = false;
                if let Some(old) = units.get(release) {
                    wake = wake.min(old.dts90 * 300 + step);
                }
            }
            if deadline > earliest + 10 * F {
                ready = false;
                wake = wake.min(deadline - 10 * F + step);
            }
        }
        let latest = plan.clock + step + bounds.transport_delay();
        let next_leak = leak.append(earliest, latest, take, bounds);
        if next_leak.is_err() {
            ready = false;
            let drain = leak.finish.div_ceil(u128::from(bounds.rb));
            let drain =
                u64::try_from(drain).map_err(|_| unsupported("multiplex drain clock overflow"))?;
            wake = wake.min(drain + 2 * step);
        }
        if ready {
            if beginning {
                reserved += au.data.len() as u128 * 8;
            }
            leak = next_leak?;
            plan.push(
                Kind::Video {
                    unit: index,
                    offset,
                    len: take,
                },
                pcr_due,
            )?;
            offset += take;
            if offset == total {
                if leak.finish >= u128::from(deadline) * u128::from(bounds.rb) {
                    return Err(unsupported("MPEG-TS multiplex leak misses decode deadline"));
                }
                index += 1;
                offset = 0;
            }
        } else if pcr_due {
            plan.push(Kind::Clock, true)?;
        } else {
            plan.idle(wake)?;
        }
    }
    plan.push(Kind::Clock, true)?;
    let count = plan
        .packets
        .len()
        .checked_mul(188)
        .filter(|bytes| *bytes <= limits.max_output_bytes)
        .ok_or_else(|| limit(LimitKind::PacketBytes))?;
    let mut output = Vec::new();
    reserve(planning, &mut output, count)?;
    let mut counters = [0u8; 4];
    let mut video_payload = false;
    for packet in plan.packets {
        plan.work.charge(1)?;
        let mut bytes = [0xff; 188];
        match packet.kind {
            Kind::Clock => {
                let next = counters.get(3).copied().unwrap_or(0);
                let cc = if video_payload {
                    next.wrapping_sub(1) & 15
                } else {
                    0
                };
                transport_header(&mut bytes, 256, false, cc, 2)?;
                if let Some(length) = bytes.get_mut(4) {
                    *length = 183;
                }
                if let Some(flags) = bytes.get_mut(5) {
                    *flags = 0x10;
                }
                write_pcr(bytes.get_mut(6..12).unwrap_or_default(), packet.clock)?;
            }
            Kind::Table(table) => {
                let pid = match table {
                    0 => 0,
                    1 => 4096,
                    _ => 17,
                };
                let cc = counters
                    .get_mut(table)
                    .ok_or_else(|| unsupported("table continuity channel"))?;
                transport_header(&mut bytes, pid, true, *cc, 1)?;
                *cc = (*cc + 1) & 15;
                if let Some(pointer) = bytes.get_mut(4) {
                    *pointer = 0;
                }
                let (section, len) = section(table)?;
                bytes
                    .get_mut(5..5 + len)
                    .ok_or_else(|| unsupported("table packet extent"))?
                    .copy_from_slice(section.get(..len).unwrap_or_default());
            }
            Kind::Video { unit, offset, len } => {
                let au = units
                    .get(unit)
                    .ok_or_else(|| unsupported("planned access-unit identity"))?;
                let cc = counters
                    .get_mut(3)
                    .ok_or_else(|| unsupported("video continuity channel"))?;
                let adaptation = 184 - len;
                transport_header(
                    &mut bytes,
                    256,
                    offset == 0,
                    *cc,
                    if adaptation > 0 { 3 } else { 1 },
                )?;
                *cc = (*cc + 1) & 15;
                video_payload = true;
                if adaptation > 0 {
                    if let Some(length) = bytes.get_mut(4) {
                        *length = (adaptation - 1) as u8;
                    }
                    if adaptation > 1 {
                        if let Some(flags) = bytes.get_mut(5) {
                            *flags = (if packet.pcr { 0x10 } else { 0 })
                                | if offset == 0 && au.idr { 0x40 } else { 0 };
                        }
                        if packet.pcr {
                            write_pcr(bytes.get_mut(6..12).unwrap_or_default(), packet.clock)?;
                        }
                    }
                }
                let (header, header_len) = pes_header(au);
                let target = bytes
                    .get_mut(4 + adaptation..)
                    .ok_or_else(|| unsupported("video payload extent"))?;
                for (index, byte) in target.iter_mut().enumerate() {
                    let position = offset + index;
                    *byte = if position < header_len {
                        *header
                            .get(position)
                            .ok_or_else(|| unsupported("PES header position"))?
                    } else {
                        *au.data
                            .get(position - header_len)
                            .ok_or_else(|| unsupported("AVC payload position"))?
                    };
                }
            }
        }
        output.extend_from_slice(&bytes);
    }
    Ok(output)
}

fn transport_header(
    bytes: &mut [u8],
    pid: u16,
    start: bool,
    cc: u8,
    control: u8,
) -> Result<(), ExtractError> {
    let target = bytes
        .get_mut(..4)
        .ok_or_else(|| unsupported("short transport header"))?;
    target.copy_from_slice(&[
        0x47,
        ((pid >> 8) as u8) | if start { 0x40 } else { 0 },
        pid as u8,
        (control << 4) | cc,
    ]);
    Ok(())
}

fn write_pcr(bytes: &mut [u8], value: u64) -> Result<(), ExtractError> {
    let base = value / 300;
    let extension = value % 300;
    if bytes.len() != 6 || base >= 1 << 33 {
        return Err(unsupported("PCR field range"));
    }
    bytes.copy_from_slice(&[
        (base >> 25) as u8,
        (base >> 17) as u8,
        (base >> 9) as u8,
        (base >> 1) as u8,
        ((base & 1) << 7) as u8 | 0x7e | (extension >> 8) as u8,
        extension as u8,
    ]);
    Ok(())
}

fn pts(value: u64, prefix: u8) -> [u8; 5] {
    [
        (prefix << 4) | (((value >> 30) as u8 & 7) << 1) | 1,
        (value >> 22) as u8,
        ((value >> 14) as u8 & 0xfe) | 1,
        (value >> 7) as u8,
        ((value as u8) << 1) | 1,
    ]
}

fn pes_header(au: &Au) -> ([u8; 19], usize) {
    let both = au.pts90 != au.dts90;
    let mut bytes = [0u8; 19];
    if let Some(head) = bytes.get_mut(..9) {
        head.copy_from_slice(&[
            0,
            0,
            1,
            0xe0,
            0,
            0,
            0x80,
            if both { 0xc0 } else { 0x80 },
            if both { 10 } else { 5 },
        ]);
    }
    if let Some(field) = bytes.get_mut(9..14) {
        field.copy_from_slice(&pts(au.pts90, if both { 3 } else { 2 }));
    }
    if both {
        if let Some(field) = bytes.get_mut(14..19) {
            field.copy_from_slice(&pts(au.dts90, 1));
        }
    }
    (bytes, if both { 19 } else { 14 })
}

fn section(table: usize) -> Result<([u8; 40], usize), ExtractError> {
    let data: &[u8] = match table {
        0 => &[0, 0xb0, 13, 0, 1, 0xc1, 0, 0, 0, 1, 0xf0, 0],
        1 => &[
            2, 0xb0, 18, 0, 1, 0xc1, 0, 0, 0xe1, 0, 0xf0, 0, 0x1b, 0xe1, 0, 0xf0, 0,
        ],
        2 => &[
            0x42, 0xf0, 37, 0, 1, 0xc1, 0, 0, 0xff, 1, 0xff, 0, 1, 0xfc, 0x80, 20, 0x48, 18, 1, 6,
            b'F', b'F', b'm', b'p', b'e', b'g', 9, b'S', b'e', b'r', b'v', b'i', b'c', b'e', b'0',
            b'1',
        ],
        _ => return Err(unsupported("unknown program table")),
    };
    let mut bytes = [0u8; 40];
    bytes
        .get_mut(..data.len())
        .ok_or_else(|| unsupported("program section extent"))?
        .copy_from_slice(data);
    bytes
        .get_mut(data.len()..data.len() + 4)
        .ok_or_else(|| unsupported("program CRC extent"))?
        .copy_from_slice(&coreav_mpegts::crc32(data).to_be_bytes());
    Ok((bytes, data.len() + 4))
}
