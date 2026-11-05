use super::{Au, Work, limit, unsupported};
use crate::ExtractError;
use coreav_isobmff::{AvcCopyFacts, LimitKind, PlanningBudget};

pub(super) const F: u64 = 27_000_000;
pub(super) const B: u64 = 1504;
pub(super) const PCR_GAP: u64 = F / 10;
pub(super) const SYS_DRAIN: u64 = 548_208;

#[derive(Clone, Copy)]
pub(super) struct Bounds {
    pub(super) rx: u64,
    pub(super) rb: u64,
    pub(super) eb: u64,
    pub(super) mb: u64,
}

impl Bounds {
    pub(super) fn from_sps(facts: AvcCopyFacts) -> Result<Self, ExtractError> {
        if facts.profile_idc != 100 || !matches!(facts.constraint_flags, 0 | 8 | 12) {
            return Err(unsupported("unqualified AVC transport profile"));
        }
        let (br, cpb) = match facts.level_idc {
            10 => (64, 175),
            9 => (128, 350),
            11 => (192, 500),
            12 => (384, 1000),
            13 => (768, 2000),
            20 => (2000, 2000),
            21 | 22 => (4000, 4000),
            30 => (10_000, 10_000),
            31 => (14_000, 14_000),
            32 => (20_000, 20_000),
            40 => (20_000, 25_000),
            41 | 42 => (50_000, 62_500),
            _ => return Err(unsupported("unqualified AVC transport level")),
        };
        let rb = 1200 * br;
        Ok(Self {
            rx: 1800 * br,
            rb,
            eb: 1200 * cpb,
            mb: rb.max(2_000_000) * 2 / 375,
        })
    }

    pub(super) fn packet_ticks(self) -> u64 {
        B * F / self.rx + 1
    }

    pub(super) fn transport_delay(self) -> u64 {
        (B * F).div_ceil(self.rx)
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct Leak {
    pub(super) finish: u128,
    pub(super) busy_start: u64,
}

impl Leak {
    pub(super) fn append(
        self,
        earliest: u64,
        latest: u64,
        bytes: usize,
        bounds: Bounds,
    ) -> Result<Self, ExtractError> {
        let previous_end = self.finish.div_ceil(u128::from(bounds.rb));
        let busy_start = if previous_end < u128::from(earliest) {
            earliest
        } else {
            self.busy_start
        };
        let arrival = u128::from(latest) * u128::from(bounds.rb);
        let finish = self.finish.max(arrival) + bytes as u128 * 8 * u128::from(F);
        let backlog = (finish - arrival).div_ceil(u128::from(F));
        if backlog + u128::from(2 * B) > u128::from(bounds.mb)
            || finish > u128::from(busy_start + F) * u128::from(bounds.rb)
        {
            return Err(unsupported("MPEG-TS multiplex-buffer bound"));
        }
        Ok(Self { finish, busy_start })
    }
}

pub(super) struct Header<'a> {
    pub(super) pid: u16,
    pub(super) cc: u8,
    pub(super) start: bool,
    pub(super) payload: &'a [u8],
    pub(super) payload_at: usize,
    pub(super) pcr: Option<u64>,
    random_access: bool,
}

pub(super) fn header(packet: &[u8]) -> Result<Header<'_>, ExtractError> {
    let [0x47, first, second, control, ..] = packet else {
        return Err(unsupported("invalid transport sync"));
    };
    if packet.len() != 188 || first & 0x80 != 0 || control & 0xc0 != 0 {
        return Err(unsupported("unqualified transport header"));
    }
    let afc = (control >> 4) & 3;
    if afc == 0 {
        return Err(unsupported("missing transport adaptation control"));
    }
    let mut at = 4usize;
    let mut pcr = None;
    let mut random_access = false;
    if afc & 2 != 0 {
        let len = usize::from(
            *packet
                .get(4)
                .ok_or_else(|| unsupported("short adaptation"))?,
        );
        at = 5 + len;
        if at > 188 || (afc == 2 && at != 188) || (afc == 3 && at >= 188) {
            return Err(unsupported("invalid adaptation extent"));
        }
        if len > 0 {
            let flags = *packet
                .get(5)
                .ok_or_else(|| unsupported("short adaptation flags"))?;
            if flags & !0x50 != 0 {
                return Err(unsupported("unqualified adaptation flags"));
            }
            random_access = flags & 0x40 != 0;
            let mut end = 6;
            if flags & 0x10 != 0 {
                let [a, b, c, d, e, f] = packet
                    .get(6..12)
                    .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
                    .ok_or_else(|| unsupported("short PCR"))?;
                let extension = (u64::from(e & 1) << 8) | u64::from(f);
                if len < 7 || e & 0x7e != 0x7e || extension >= 300 {
                    return Err(unsupported("invalid PCR syntax"));
                }
                let base = (u64::from(a) << 25)
                    | (u64::from(b) << 17)
                    | (u64::from(c) << 9)
                    | (u64::from(d) << 1)
                    | u64::from(e >> 7);
                pcr = Some(base * 300 + extension);
                end = 12;
            }
            if packet
                .get(end..at)
                .is_none_or(|stuffing| stuffing.iter().any(|byte| *byte != 0xff))
            {
                return Err(unsupported("invalid adaptation stuffing"));
            }
        }
    }
    let payload = if afc & 1 != 0 {
        packet
            .get(at..)
            .ok_or_else(|| unsupported("transport payload extent"))?
    } else {
        &[]
    };
    Ok(Header {
        pid: (u16::from(first & 31) << 8) | u16::from(*second),
        cc: control & 15,
        start: first & 0x40 != 0,
        payload,
        payload_at: at,
        pcr,
        random_access,
    })
}

struct Anchor {
    byte: usize,
    ticks: u64,
}

fn clock(
    anchors: &[Anchor],
    interval: &mut usize,
    byte: usize,
    up: bool,
) -> Result<u64, ExtractError> {
    while anchors
        .get(*interval + 1)
        .is_some_and(|anchor| anchor.byte <= byte)
    {
        *interval += 1;
    }
    let left = anchors
        .get(*interval)
        .ok_or_else(|| unsupported("missing PCR interval"))?;
    if left.byte == byte {
        return Ok(left.ticks);
    }
    let right = anchors
        .get(*interval + 1)
        .ok_or_else(|| unsupported("unbracketed transport byte"))?;
    let width = right
        .byte
        .checked_sub(left.byte)
        .filter(|width| *width > 0)
        .ok_or_else(|| unsupported("invalid PCR positions"))? as u128;
    let distance =
        byte.checked_sub(left.byte)
            .ok_or_else(|| unsupported("transport byte precedes first PCR"))? as u128;
    let delta = right
        .ticks
        .checked_sub(left.ticks)
        .ok_or_else(|| unsupported("nonmonotonic PCR"))?;
    let offset = distance * u128::from(delta);
    let value = if up {
        offset.div_ceil(width)
    } else {
        offset / width
    };
    u64::try_from(u128::from(left.ticks) + value)
        .map_err(|_| unsupported("transport clock overflow"))
}

pub(super) fn verify(
    bytes: &[u8],
    units: &[Au],
    bounds: Bounds,
    planning: &PlanningBudget,
    work: &mut Work,
) -> Result<(), ExtractError> {
    if bytes.is_empty() || bytes.len() % 188 != 0 {
        return Err(unsupported("incomplete transport packets"));
    }
    let mut anchors: Vec<Anchor> = Vec::new();
    for (index, packet) in bytes.chunks_exact(188).enumerate() {
        work.charge(1)?;
        let header = header(packet)?;
        if let Some(ticks) = header.pcr {
            if header.pid != 256 {
                return Err(unsupported("PCR on an unexpected PID"));
            }
            let byte = index * 188 + 10;
            if let Some(previous) = anchors.last() {
                let delta = ticks
                    .checked_sub(previous.ticks)
                    .filter(|delta| *delta > 0 && *delta <= PCR_GAP)
                    .ok_or_else(|| unsupported("PCR repetition or range"))?;
                if (byte - previous.byte) as u128 * 8 * u128::from(F)
                    >= u128::from(bounds.rx) * u128::from(delta)
                {
                    return Err(unsupported("transport-buffer arrival rate"));
                }
            }
            planning
                .reserve(&mut anchors, 1)
                .map_err(|_| limit(LimitKind::PlanningBytes))?;
            anchors.push(Anchor { byte, ticks });
        }
    }
    let first = header(
        bytes
            .get(..188)
            .ok_or_else(|| unsupported("missing first packet"))?,
    )?;
    let last = header(
        bytes
            .get(bytes.len() - 188..)
            .ok_or_else(|| unsupported("missing terminal packet"))?,
    )?;
    if anchors.len() < 2
        || anchors.first().is_none_or(|anchor| anchor.byte != 10)
        || anchors
            .last()
            .is_none_or(|anchor| anchor.byte != bytes.len() - 178)
        || !first.payload.is_empty()
        || !last.payload.is_empty()
    {
        return Err(unsupported("transport payload lacks PCR brackets"));
    }
    let mut interval = 0;
    let mut continuity = [None; 4];
    let mut tables = [None; 3];
    let mut sys_end = None;
    let mut initial_tables_end = 0;
    let mut current = None;
    let mut au_index = 0;
    let mut copied = 0usize;
    let mut release = 0;
    let mut reserved = 0u128;
    let mut leak = Leak::default();
    for (index, packet) in bytes.chunks_exact(188).enumerate() {
        work.charge(1)?;
        let head = header(packet)?;
        let channel = match head.pid {
            0 => 0,
            4096 => 1,
            17 => 2,
            256 => 3,
            8191 if !head.start && head.pcr.is_none() => continue,
            _ => return Err(unsupported("unexpected transport PID")),
        };
        if head.payload.is_empty() {
            if head.start
                || head.random_access
                || channel != 3
                || continuity.get(3).copied().flatten().unwrap_or(0) != head.cc
            {
                return Err(unsupported("adaptation-only continuity or signaling"));
            }
            continue;
        }
        let previous = continuity
            .get_mut(channel)
            .ok_or_else(|| unsupported("transport continuity channel"))?;
        if previous.is_some_and(|cc: u8| (cc + 1) & 15 != head.cc) {
            return Err(unsupported("transport payload continuity"));
        }
        *previous = Some(head.cc);
        let earliest = clock(
            &anchors,
            &mut interval,
            index * 188 + head.payload_at,
            false,
        )?;
        let end = clock(&anchors, &mut interval, index * 188 + 187, true)?;
        if channel < 3 {
            verify_table(channel, head.start, head.payload)?;
            let slot = tables
                .get_mut(channel)
                .ok_or_else(|| unsupported("transport table channel"))?;
            let maximum = if channel == 2 { 2 * F } else { F / 2 };
            if slot.is_some_and(|previous| {
                earliest < previous + F / 40 || earliest > previous + maximum
            }) {
                return Err(unsupported("transport table repetition"));
            }
            if slot.is_none()
                && anchors
                    .first()
                    .is_some_and(|first| earliest > first.ticks + maximum)
            {
                return Err(unsupported("late initial transport table"));
            }
            *slot = Some(earliest);
            if channel < 2 {
                let packet_start = clock(&anchors, &mut interval, index * 188, false)?;
                if sys_end.is_some_and(|previous| packet_start < previous + SYS_DRAIN) {
                    return Err(unsupported("transport system-buffer spacing"));
                }
                sys_end = Some(end);
                if au_index == 0 && current.is_none() {
                    initial_tables_end = end + SYS_DRAIN;
                }
            }
            continue;
        }
        let mut payload = head.payload;
        if head.start {
            if current.is_some()
                || tables.first().is_none_or(Option::is_none)
                || tables.get(1).is_none_or(Option::is_none)
                || earliest < initial_tables_end
            {
                return Err(unsupported("PES before complete initial system tables"));
            }
            let au = units
                .get(au_index)
                .ok_or_else(|| unsupported("extra video PES"))?;
            if head.random_access != au.idr {
                return Err(unsupported("AVC transport random-access indication"));
            }
            let skip = pes(payload, au)?;
            let dts = au.dts90 * 300;
            if earliest > dts || dts - earliest > 10 * F {
                return Err(unsupported("AVC transport delay"));
            }
            while units
                .get(release)
                .is_some_and(|au| au.dts90 * 300 <= earliest)
                && release < au_index
            {
                let old = units
                    .get(release)
                    .ok_or_else(|| unsupported("AVC reservation identity"))?;
                reserved = reserved
                    .checked_sub(old.data.len() as u128 * 8)
                    .ok_or_else(|| unsupported("AVC reservation underflow"))?;
                release += 1;
            }
            reserved += au.data.len() as u128 * 8;
            if reserved + 8 > u128::from(bounds.eb) {
                return Err(unsupported("AVC elementary-buffer reservation"));
            }
            current = Some(au_index);
            copied = 0;
            payload = payload
                .get(skip..)
                .ok_or_else(|| unsupported("PES header extent"))?;
        } else if head.random_access {
            return Err(unsupported("random access on a PES continuation"));
        }
        let active = current.ok_or_else(|| unsupported("video continuation without PES"))?;
        let au = units
            .get(active)
            .ok_or_else(|| unsupported("video access-unit identity"))?;
        let next = copied
            .checked_add(payload.len())
            .ok_or_else(|| unsupported("PES byte count overflow"))?;
        if au.data.get(copied..next) != Some(payload) {
            return Err(unsupported(
                "serialized AVC payload differs from preparation",
            ));
        }
        leak = leak.append(
            earliest,
            end + bounds.transport_delay(),
            head.payload.len(),
            bounds,
        )?;
        copied = next;
        if copied == au.data.len() {
            if leak.finish >= u128::from(au.dts90 * 300) * u128::from(bounds.rb) {
                return Err(unsupported("AVC data misses its decode deadline"));
            }
            current = None;
            au_index += 1;
        }
    }
    if current.is_some() || au_index != units.len() || tables.iter().any(Option::is_none) {
        return Err(unsupported("incomplete serialized transport"));
    }
    let terminal = anchors
        .last()
        .ok_or_else(|| unsupported("terminal PCR"))?
        .ticks;
    for (index, previous) in tables.iter().enumerate() {
        let maximum = if index == 2 { 2 * F } else { F / 2 };
        if previous.is_none_or(|previous| terminal > previous + maximum) {
            return Err(unsupported("missing final transport table repetition"));
        }
    }
    Ok(())
}

fn pes(bytes: &[u8], au: &Au) -> Result<usize, ExtractError> {
    if bytes.get(..7) != Some(&[0, 0, 1, 0xe0, 0, 0, 0x80]) {
        return Err(unsupported("invalid qualified video PES header"));
    }
    let both = au.pts90 != au.dts90;
    if bytes.get(7) != Some(&(if both { 0xc0 } else { 0x80 }))
        || bytes.get(8) != Some(&(if both { 10 } else { 5 }))
        || timestamp(
            bytes.get(9..14).unwrap_or_default(),
            if both { 3 } else { 2 },
        ) != Some(au.pts90)
        || (both && timestamp(bytes.get(14..19).unwrap_or_default(), 1) != Some(au.dts90))
    {
        return Err(unsupported("PES timestamp mismatch"));
    }
    Ok(if both { 19 } else { 14 })
}

fn timestamp(bytes: &[u8], prefix: u8) -> Option<u64> {
    let [a, b, c, d, e] = *bytes else { return None };
    if a >> 4 != prefix || a & 1 == 0 || c & 1 == 0 || e & 1 == 0 {
        return None;
    }
    Some(
        (u64::from((a >> 1) & 7) << 30)
            | (u64::from(b) << 22)
            | (u64::from(c >> 1) << 15)
            | (u64::from(d) << 7)
            | u64::from(e >> 1),
    )
}

fn verify_table(channel: usize, start: bool, payload: &[u8]) -> Result<(), ExtractError> {
    if !start || payload.first() != Some(&0) {
        return Err(unsupported("PSI pointer or unit start"));
    }
    let bytes = payload.get(1..).ok_or_else(|| unsupported("missing PSI"))?;
    let length = bytes
        .get(1..3)
        .and_then(|bytes| <[u8; 2]>::try_from(bytes).ok())
        .map(u16::from_be_bytes)
        .map(|length| usize::from(length & 0x0fff) + 3)
        .ok_or_else(|| unsupported("short PSI length"))?;
    let section = bytes
        .get(..length)
        .ok_or_else(|| unsupported("PSI past transport packet"))?;
    if coreav_mpegts::crc32(section) != 0
        || bytes
            .get(length..)
            .is_none_or(|bytes| bytes.iter().any(|byte| *byte != 0xff))
        || section.get(3..8) != Some(&[0, 1, 0xc1, 0, 0])
    {
        return Err(unsupported("invalid PSI CRC or section header"));
    }
    let valid = match channel {
        0 => {
            length == 16
                && section.get(..3) == Some(&[0, 0xb0, 13])
                && section.get(8..12) == Some(&[0, 1, 0xf0, 0])
        }
        1 => {
            length == 21
                && section.get(..3) == Some(&[2, 0xb0, 18])
                && section.get(8..17) == Some(&[0xe1, 0, 0xf0, 0, 0x1b, 0xe1, 0, 0xf0, 0])
        }
        2 => {
            length == 40
                && section.get(..3) == Some(&[0x42, 0xf0, 37])
                && section.get(8..18) == Some(&[0xff, 1, 0xff, 0, 1, 0xfc, 0x80, 20, 0x48, 18])
                && section.get(18..21) == Some(&[1, 6, b'F'])
                && section.get(20..26) == Some(b"FFmpeg")
                && section.get(26) == Some(&9)
                && section.get(27..36) == Some(b"Service01")
        }
        _ => false,
    };
    if !valid {
        return Err(unsupported("unqualified transport program metadata"));
    }
    Ok(())
}
