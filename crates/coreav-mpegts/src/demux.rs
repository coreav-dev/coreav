//! The demultiplexer state shared by the probe and the packet listing:
//! PAT and PMT sections, continuity counters, and PES packets reassembled
//! per PID.

use crate::codec::{Allowlist, Codec, codec, format_identifier};
use crate::error::{Warning, warn};
use crate::packet::{self, NULL_PID, TS_PACKET};
use crate::pes::{self, PesHeader};
use crate::section::{
    self, Assembler, Descriptor, EIT_FIRST, EIT_LAST, EIT_PID, MAX_EIT_SECTION,
    MAX_PRIVATE_SECTION, MAX_SECTION, PAT, PMT, SDT_ACTUAL, SDT_PID, SPLICE_INFO, Service,
};
use crate::{Error, LimitKind, Limits};
use std::collections::{BTreeMap, VecDeque};

/// Whether two packets of one PID are the same apart from a PCR, which a
/// duplicate may carry anew (ISO/IEC 13818-1 2.4.3.3).
fn same_but_pcr(a: &[u8; TS_PACKET], b: &[u8; TS_PACKET]) -> bool {
    let pcr = |p: &[u8; TS_PACKET]| {
        p.get(3).is_some_and(|c| c & 0x20 != 0)
            && p.get(4).is_some_and(|l| *l >= 7)
            && p.get(5).is_some_and(|f| f & 0x10 != 0)
    };
    if pcr(a) && pcr(b) {
        a.get(..6) == b.get(..6) && a.get(12..) == b.get(12..)
    } else {
        a == b
    }
}

/// A complete PES packet of a kept stream.
#[derive(Clone, Debug)]
pub(crate) struct Pes {
    pub pid: u16,
    /// Offset of the stored packet that starts it.
    pub offset: u64,
    pub header: PesHeader,
    pub payload: Vec<u8>,
    /// The adaptation field of its first packet set the random access
    /// indicator.
    pub random_access: bool,
    /// A packet of it was lost (a continuity counter skipped) or repeated
    /// while it was read.
    pub corrupt: bool,
}

/// Most payload bytes, and PES packets, read to learn the codec of a
/// stream the tables leave unnamed ([`Demux::sniffed`]): enough for the
/// three frames in a row [`crate::codec::sniff`] asks for of the longest
/// AC-3 frames (3,840 bytes).
const SNIFF_BYTES: usize = 16 << 10;
const SNIFF_PACKETS: u8 = 8;

/// `stream_id` of a padding stream (ISO/IEC 13818-1 table 2-22).
const PADDING_STREAM: u8 = 0xBE;

/// A stream as the PMTs declare it.
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub pid: u16,
    pub programs: Vec<u16>,
    pub stream_type: u8,
    pub descriptors: Vec<Descriptor>,
    /// The four bytes its descriptors give its format, if any
    /// ([`format_identifier`]).
    pub format_identifier: Option<[u8; 4]>,
    pub codec: Codec,
    /// Its place in the elementary stream loop of the first Program Map
    /// Table that declared it; `None` for a stream no table declares (the
    /// Event Information Table).
    pub pmt_index: Option<usize>,
    pub kept: bool,
    /// A stream whose type and descriptors name no codec: its first PES
    /// packets are read to learn its codec ([`crate::codec::sniff`]). A
    /// stream whose first unit is not a PES packet (sections), or whose
    /// packets are scrambled, has none to read and stays unnamed.
    pub sniff: bool,
    /// The payloads of its PES packets read so far for that, joined, and
    /// how many: a frame that a PES packet cuts goes on in the next.
    pub sniffed: (Vec<u8>, u8),
    /// Where the stream was found (a byte offset): the packet that
    /// completed the Program Map Table naming it, or that completed the
    /// first Event Information Table section; `u64::MAX` for a PID no
    /// table declares.
    pub found_at: u64,
}

/// A Program Map Table section read before the first PAT named its PID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EarlyPmt {
    /// The packet that completed it.
    pub at: u64,
    pub pmt_pid: u16,
    pub number: u16,
    /// Its elementary streams' PIDs, in its order.
    pub pids: Vec<u16>,
}

/// A program as the PAT and its PMT declare it.
#[derive(Clone, Debug)]
pub(crate) struct ProgramEntry {
    pub number: u16,
    pub pmt_pid: u16,
    pub pcr_pid: Option<u16>,
    pub pmt_version: Option<u8>,
    pub pids: Vec<u16>,
}

struct Builder {
    offset: u64,
    data: Vec<u8>,
    random_access: bool,
    corrupt: bool,
}

pub(crate) struct Demux {
    allowlist: Allowlist,
    max_packet: usize,
    max_buffered: usize,
    /// Bytes held by `builders`.
    buffered: usize,
    /// The last packet with a payload of each PID, to tell a duplicate.
    last: BTreeMap<u16, [u8; TS_PACKET]>,
    /// The offset and the first timestamp (PTS, else DTS) of each PID's
    /// PES packets, whether or not a Program Map Table had named it yet:
    /// a stream's PES packets commonly start before the tables that name
    /// it, and those packets are the stream's too.
    pub first_pes: BTreeMap<u16, (u64, u64)>,
    /// Where each PID's first PES packet is, whether or not it states a
    /// time: a PID no table declares is a stream all the same. A padding
    /// stream's PES packets (`stream_id` 0xBE, ISO/IEC 13818-1 table 2-22)
    /// carry no data and do not count: `ffprobe` lists no stream for a PID
    /// that carries only those, and does not place a stream at one
    /// (crafted files).
    pub pes_pids: BTreeMap<u16, u64>,
    /// Where the first Program Map Table was read.
    pub first_pmt_at: Option<u64>,
    /// The Program Map Table sections that came before the first PAT, at
    /// most one for each PMT PID and program number: where each was
    /// complete, and its PIDs in its order ([`EarlyPmt`]).
    pub early_pmts: Vec<EarlyPmt>,
    /// The sections being reassembled on PIDs that may carry a Program
    /// Map Table, before the first PAT says which do.
    early_sections: BTreeMap<u16, Assembler>,
    /// The offset of the packet being read.
    current: u64,
    max_programs: usize,
    max_streams: usize,
    pat_version: Option<u8>,
    /// The sections of the current PAT version seen so far, and its
    /// `last_section_number`.
    pat_sections: (u64, u8),
    /// `transport_stream_id` of the current PAT.
    pub transport_stream_id: Option<u16>,
    /// The services of the current Service Description Table of this
    /// transport stream, at most `max_programs`.
    pub services: Vec<Service>,
    /// Whether the first Service Description Table read came before the
    /// first Program Association Table; `None` until either is read.
    pub sdt_before_pat: Option<bool>,
    pub programs: Vec<ProgramEntry>,
    pub streams: Vec<Entry>,
    /// The file has no Program Association Table: its streams are the PIDs
    /// that carry PES packets ([`Demux::without_tables`]).
    pub tableless: bool,
    /// Once the tables are read, a PID that carries PES packets and that
    /// no table declares is taken as a stream as soon as one of its PES
    /// packets starts, to be named by its payload (the probe's head; a
    /// later read takes the probe's streams).
    pub name_undeclared: bool,
    by_pid: BTreeMap<u16, usize>,
    sections: BTreeMap<u16, Assembler>,
    builders: BTreeMap<u16, Builder>,
    /// Per PID, the header of the PES packet in progress once a piece of
    /// it was delivered ([`Demux::pieces`]), its times cleared: what the
    /// later pieces carry. Kept apart from the builders, so that reading
    /// whole PES packets carries none of it.
    continued: BTreeMap<u16, PesHeader>,
    counters: BTreeMap<u16, u8>,
    pub ready: VecDeque<Pes>,
    /// Complete sections that are packets of a stream, header and CRC
    /// included, as `ffprobe` lists them, each with its PID and its time:
    /// those of the Event Information Table, while it is listed as a
    /// stream ([`Demux::keep_epg`]), with no time; the splice information
    /// sections of the SCTE 35 streams kept, while they are read
    /// ([`Demux::keep_splices`]), with the base (90 kHz) of the last
    /// program clock reference the PCR PID of their program carried
    /// before they completed.
    pub sections_listed: VecDeque<(u16, Vec<u8>, Option<u64>)>,
    /// Whether to reassemble PES packets (the tables are always read).
    pub collect: bool,
    /// Whether to keep the Event Information Table's sections: only the
    /// packet listing reads them.
    pub keep_epg: bool,
    /// Whether to read the sections of the SCTE 35 streams kept
    /// ([`Demux::sections_listed`]): only the listing as `ffprobe` gives
    /// it does.
    pub keep_splices: bool,
    /// The base of the last program clock reference of each PID, while
    /// splice sections are kept; bounded by `max_streams`.
    pcrs: BTreeMap<u16, u64>,
    /// PIDs whose PES packets are reassembled whatever the tables say
    /// (the streams a probe kept), besides those the tables keep.
    pub always: std::collections::BTreeSet<u16>,
    /// When set, the PES packets of H.264, HEVC, MPEG-1 and MPEG-2 video
    /// streams are delivered in pieces of this many payload bytes as they
    /// arrive, the later pieces with no times, rather than whole once the
    /// next one starts: the packet listing reads them as `ffprobe` does.
    /// What a piece leaves behind is bounded by `max_buffered_bytes`, which
    /// the listing checks, rather than by `max_packet_bytes`.
    pub pieces: Option<usize>,
}

impl Demux {
    pub(crate) fn new(allowlist: &Allowlist, limits: &Limits) -> Self {
        Self {
            allowlist: allowlist.clone(),
            max_packet: usize::try_from(limits.max_packet_bytes).unwrap_or(usize::MAX),
            max_buffered: usize::try_from(limits.max_buffered_bytes).unwrap_or(usize::MAX),
            buffered: 0,
            last: BTreeMap::new(),
            first_pes: BTreeMap::new(),
            pes_pids: BTreeMap::new(),
            first_pmt_at: None,
            early_pmts: Vec::new(),
            early_sections: BTreeMap::new(),
            current: 0,
            max_programs: usize::try_from(limits.max_programs).unwrap_or(usize::MAX),
            max_streams: usize::try_from(limits.max_streams).unwrap_or(usize::MAX),
            pat_version: None,
            pat_sections: (0, 0),
            transport_stream_id: None,
            services: Vec::new(),
            sdt_before_pat: None,
            programs: Vec::new(),
            streams: Vec::new(),
            tableless: false,
            name_undeclared: false,
            by_pid: BTreeMap::new(),
            sections: BTreeMap::new(),
            builders: BTreeMap::new(),
            continued: BTreeMap::new(),
            counters: BTreeMap::new(),
            ready: VecDeque::new(),
            sections_listed: VecDeque::new(),
            collect: true,
            keep_epg: false,
            keep_splices: false,
            pcrs: BTreeMap::new(),
            always: std::collections::BTreeSet::new(),
            pieces: None,
        }
    }

    /// A demultiplexer that already knows the tables (for a read that
    /// starts later in the file).
    pub(crate) fn resume(&self) -> Self {
        Self {
            allowlist: self.allowlist.clone(),
            max_packet: self.max_packet,
            max_buffered: self.max_buffered,
            buffered: 0,
            last: BTreeMap::new(),
            first_pes: BTreeMap::new(),
            pes_pids: BTreeMap::new(),
            first_pmt_at: self.first_pmt_at,
            early_pmts: Vec::new(),
            early_sections: BTreeMap::new(),
            current: 0,
            max_programs: self.max_programs,
            max_streams: self.max_streams,
            pat_version: self.pat_version,
            pat_sections: self.pat_sections,
            transport_stream_id: self.transport_stream_id,
            services: self.services.clone(),
            sdt_before_pat: self.sdt_before_pat,
            programs: self.programs.clone(),
            streams: self.streams.clone(),
            tableless: self.tableless,
            name_undeclared: false,
            by_pid: self.by_pid.clone(),
            sections: BTreeMap::new(),
            builders: BTreeMap::new(),
            continued: BTreeMap::new(),
            counters: BTreeMap::new(),
            ready: VecDeque::new(),
            sections_listed: VecDeque::new(),
            collect: true,
            keep_epg: false,
            keep_splices: false,
            pcrs: BTreeMap::new(),
            always: self.always.clone(),
            pieces: None,
        }
    }

    /// Bytes held by the PES packets being reassembled.
    pub(crate) fn buffered(&self) -> usize {
        self.buffered
    }

    /// The stream on `pid` shows no payload to name it by: it is not
    /// waited for.
    fn stays_unnamed(&mut self, pid: u16) {
        if let Some(entry) = self
            .by_pid
            .get(&pid)
            .and_then(|&index| self.streams.get_mut(index))
        {
            entry.sniff = false;
        }
    }

    /// Whether a Program Association Table was read.
    pub(crate) fn pat_read(&self) -> bool {
        self.pat_version.is_some()
    }

    /// Takes the PIDs that carried PES packets in a file without a Program
    /// Association Table as its streams, in the order of their first PES
    /// packets (`pids` gives where each is), each named by its payload
    /// ([`crate::codec::sniff`]).
    pub(crate) fn without_tables(&mut self, pids: &BTreeMap<u16, u64>) -> Result<(), Error> {
        self.tableless = true;
        let mut order: Vec<(u64, u16)> = pids.iter().map(|(pid, at)| (*at, *pid)).collect();
        order.sort_unstable();
        for (at, pid) in order {
            if self.by_pid.contains_key(&pid) {
                continue;
            }
            self.undeclared(pid)?;
            if let Some(entry) = self.streams.last_mut() {
                entry.sniff = true;
                entry.found_at = at;
            }
        }
        Ok(())
    }

    /// Whether the PAT and the PMT of every program it names were read.
    pub(crate) fn tables_complete(&self) -> bool {
        let (seen, last) = self.pat_sections;
        let all = (0..=u32::from(last.min(63))).all(|n| seen & (1 << n) != 0);
        self.pat_version.is_some() && all && self.programs.iter().all(|p| p.pmt_version.is_some())
    }

    /// Learns the codec of a stream from its first PES packets, if the
    /// PMT left it unnamed: their payloads are joined, up to
    /// [`SNIFF_BYTES`] and [`SNIFF_PACKETS`], until they name one.
    pub(crate) fn sniffed(&mut self, index: usize, pes: &Pes) {
        let allowlist = &self.allowlist;
        if let Some(entry) = self.streams.get_mut(index) {
            if entry.sniff {
                let (data, packets) = &mut entry.sniffed;
                let room = SNIFF_BYTES.saturating_sub(data.len());
                data.extend_from_slice(pes.payload.get(..room).unwrap_or(&pes.payload));
                *packets = packets.saturating_add(1);
                let codec = crate::codec::sniff(pes.header.stream_id, data);
                if codec != Codec::Other || data.len() >= SNIFF_BYTES || *packets >= SNIFF_PACKETS {
                    entry.sniff = false;
                    entry.sniffed = (Vec::new(), 0);
                    entry.codec = codec;
                    entry.kept = allowlist.reads(codec);
                }
            }
        }
    }

    /// The index of the stream on `pid`.
    pub(crate) fn stream_of(&self, pid: u16) -> Option<usize> {
        self.by_pid.get(&pid).copied()
    }

    /// Takes one transport packet stored at `offset`.
    pub(crate) fn feed(
        &mut self,
        offset: u64,
        bytes: &[u8; TS_PACKET],
        warnings: &mut Vec<Warning>,
    ) -> Result<(), Error> {
        self.current = offset;
        let header = match packet::parse(bytes) {
            Ok(header) => header,
            Err(reason) => {
                warn(
                    warnings,
                    Warning::BadPes {
                        pid: 0,
                        offset,
                        reason,
                    },
                );
                return Ok(());
            }
        };
        if header.error {
            // The packet may be damaged; its payload is read all the same,
            // as `ffprobe` reads it.
            warn(warnings, Warning::TransportError { offset });
        }
        let pid = header.pid;
        if pid == NULL_PID {
            return Ok(());
        }
        let discontinuity = match header.adaptation {
            Some(adaptation) => {
                if self.keep_splices {
                    // Bounded by the stream limit, as every other per-PID
                    // fact is: a PID already recorded keeps its clock.
                    if let Some(pcr) = adaptation.pcr {
                        if self.pcrs.len() < self.max_streams || self.pcrs.contains_key(&pid) {
                            self.pcrs.insert(pid, pcr / 300);
                        }
                    }
                }
                adaptation.discontinuity
            }
            None => false,
        };
        let Some(start) = header.payload else {
            return Ok(());
        };
        // Continuity: the counter steps by one for each packet with a
        // payload. A packet that repeats the one before it (the same counter
        // and the same bytes, its PCR aside: 2.4.3.3) is a duplicate, and
        // is dropped; the same counter on other bytes is a lost packet.
        let previous = self.counters.insert(pid, header.counter);
        let repeated = self.last.insert(pid, *bytes);
        if let Some(previous) = previous {
            if previous == header.counter && !discontinuity {
                if repeated.is_some_and(|before| same_but_pcr(&before, bytes)) {
                    warn(warnings, Warning::Duplicate { pid, offset });
                    self.mark_corrupt(pid);
                    return Ok(());
                }
                warn(warnings, Warning::Discontinuity { pid, offset });
                self.mark_corrupt(pid);
            } else if (previous + 1) & 0x0F != header.counter && !discontinuity {
                warn(warnings, Warning::Discontinuity { pid, offset });
                self.mark_corrupt(pid);
            }
        }
        let payload = bytes.get(start..).unwrap_or_default();
        if pid == 0
            || pid == EIT_PID
            || pid == SDT_PID
            || self.programs.iter().any(|p| p.pmt_pid == pid)
        {
            return self.section_payload(pid, payload, header.unit_start, warnings);
        }
        if !self.pat_read() {
            self.early_section(pid, payload, header.unit_start);
        }
        if self.keep_splices && self.splice_pid(pid) {
            return self.splice_payload(pid, payload, header.unit_start, warnings);
        }
        // The first timestamp of the PID, recorded before any table names
        // it: the stream starts where its first PES packet does, not where
        // the reader learnt that the PID carries one. Bounded by the
        // stream limit, as every other per-PID fact is.
        if header.unit_start
            && !self.first_pes.contains_key(&pid)
            && self.pes_pids.len() < self.max_streams
        {
            let header = match pes::parse(payload) {
                pes::Parsed::Done(first) if first.stream_id != PADDING_STREAM => Some(first),
                _ => None,
            };
            if let Some(first) = header {
                self.pes_pids.entry(pid).or_insert(offset);
                // A PES packet of the stream may state no time; the first
                // that does gives the stream its start.
                if let Some(time) = first.pts.or(first.dts) {
                    self.first_pes.insert(pid, (offset, time));
                }
            }
        }
        // A PID no table declares, once the tables are read: `ffprobe`
        // names it by its payload, as it names a stream whose type names
        // no codec (crafted files: AC-3 and DTS on a PID the PMT leaves
        // out). The PIDs that started PES packets before the tables were
        // read come first, in the order of their first ones.
        if self.name_undeclared
            && header.unit_start
            && !self.by_pid.contains_key(&pid)
            && self.pes_pids.contains_key(&pid)
            && self.tables_complete()
        {
            let before = self.streams.len();
            self.add_undeclared(&BTreeMap::new())?;
            for entry in self.streams.iter_mut().skip(before) {
                entry.sniff = true;
            }
        }
        if !self.collect {
            return Ok(());
        }
        let stream = self
            .by_pid
            .get(&pid)
            .and_then(|&index| self.streams.get(index));
        let sniffing = stream.is_some_and(|s| s.sniff);
        let wanted = self.always.contains(&pid) || sniffing || stream.is_some_and(|s| s.kept);
        if !wanted {
            return Ok(());
        }
        if header.scrambled {
            warn(warnings, Warning::Scrambled { pid });
            self.stays_unnamed(pid);
            return Ok(());
        }
        if header.unit_start && sniffing && matches!(pes::parse(payload), pes::Parsed::Invalid(_)) {
            self.stays_unnamed(pid);
        }
        if header.unit_start {
            self.complete(pid, warnings);
            self.builders.insert(
                pid,
                Builder {
                    offset,
                    data: Vec::new(),
                    random_access: header.adaptation.is_some_and(|a| a.random_access),
                    corrupt: false,
                },
            );
        }
        let Some(builder) = self.builders.get_mut(&pid) else {
            // No unit start seen yet on this PID.
            return Ok(());
        };
        builder.data.extend_from_slice(payload);
        if header.error {
            builder.corrupt = true;
        }
        self.buffered = self.buffered.saturating_add(payload.len());
        if builder.data.len() > self.max_packet {
            return Err(Error::LimitExceeded {
                limit: LimitKind::PacketBytes,
                offset,
            });
        }
        if self.buffered > self.max_buffered {
            return Err(Error::LimitExceeded {
                limit: LimitKind::BufferedBytes,
                offset,
            });
        }
        // A PES packet with a length is complete once that many bytes are
        // there; the rest of the packet is stuffing. (Only a packet of no
        // stated length is long enough to be delivered in pieces.)
        let declared = match *builder.data.as_slice() {
            [0, 0, 1, _, l0, l1, ..]
                if self.pieces.is_none() || !self.continued.contains_key(&pid) =>
            {
                usize::from(u16::from_be_bytes([l0, l1]))
            }
            _ => 0,
        };
        if declared > 0 && builder.data.len() >= declared + 6 {
            builder.data.truncate(declared + 6);
            self.complete(pid, warnings);
        } else if let Some(size) = self.pieces {
            self.deliver_pieces(pid, size);
        }
        Ok(())
    }

    /// Delivers the whole pieces of `size` payload bytes the PES packet in
    /// progress on `pid` holds, when it is one of coded video. Kept out of
    /// line: only the listing as `ffprobe` gives it reads in pieces, and
    /// reading whole PES packets should not pay for it.
    #[inline(never)]
    fn deliver_pieces(&mut self, pid: u16, size: usize) {
        let coded = self
            .stream_of(pid)
            .and_then(|i| self.streams.get(i))
            .is_some_and(|s| {
                matches!(
                    s.codec,
                    Codec::H264 | Codec::Hevc | Codec::Mpeg1Video | Codec::Mpeg2Video
                )
            });
        if !coded || size == 0 {
            return;
        }
        let Some(builder) = self.builders.get_mut(&pid) else {
            return;
        };
        loop {
            let continued = self.continued.get(&pid).copied();
            let (header, skip) = match continued {
                Some(header) => (header, 0),
                None => match pes::parse(&builder.data) {
                    pes::Parsed::Done(header) => (header, header.header_len),
                    _ => return,
                },
            };
            let end = skip.saturating_add(size);
            let Some(payload) = builder.data.get(skip..end) else {
                return;
            };
            self.ready.push_back(Pes {
                pid,
                offset: builder.offset,
                header,
                payload: payload.to_vec(),
                random_access: builder.random_access && continued.is_none(),
                corrupt: builder.corrupt,
            });
            builder.data.drain(..end);
            self.buffered = self.buffered.saturating_sub(end);
            self.continued.insert(
                pid,
                PesHeader {
                    pts: None,
                    dts: None,
                    header_len: 0,
                    ..header
                },
            );
        }
    }

    /// Marks the PES packet in progress on `pid` as damaged.
    fn mark_corrupt(&mut self, pid: u16) {
        if let Some(builder) = self.builders.get_mut(&pid) {
            builder.corrupt = true;
        }
    }

    /// Ends every PES packet in progress (the end of the file, or of the
    /// part read).
    pub(crate) fn finish(&mut self, warnings: &mut Vec<Warning>) {
        let pids: Vec<u16> = self.builders.keys().copied().collect();
        for pid in pids {
            self.complete(pid, warnings);
        }
    }

    /// Ends the PES packet in progress on `pid`.
    fn complete(&mut self, pid: u16, warnings: &mut Vec<Warning>) {
        let Some(builder) = self.builders.remove(&pid) else {
            return;
        };
        self.buffered = self.buffered.saturating_sub(builder.data.len());
        // The last piece of a PES packet delivered in pieces.
        if let Some(header) = self.pieces.and_then(|_| self.continued.remove(&pid)) {
            self.ready.push_back(Pes {
                pid,
                offset: builder.offset,
                header,
                payload: builder.data,
                random_access: false,
                corrupt: builder.corrupt,
            });
            return;
        }
        match pes::parse(&builder.data) {
            pes::Parsed::Done(header) => {
                let declared = usize::from(header.packet_length);
                let truncated = declared > 0 && builder.data.len() < declared + 6;
                if truncated {
                    warn(warnings, Warning::TruncatedPes { pid });
                }
                let payload = builder
                    .data
                    .get(header.header_len..)
                    .unwrap_or_default()
                    .to_vec();
                self.ready.push_back(Pes {
                    pid,
                    offset: builder.offset,
                    header,
                    payload,
                    random_access: builder.random_access,
                    corrupt: builder.corrupt || truncated,
                });
            }
            pes::Parsed::NeedMore(_) => warn(
                warnings,
                Warning::BadPes {
                    pid,
                    offset: builder.offset,
                    reason: "a PES packet shorter than its header",
                },
            ),
            pes::Parsed::Invalid(reason) => warn(
                warnings,
                Warning::BadPes {
                    pid,
                    offset: builder.offset,
                    reason,
                },
            ),
        }
    }

    /// Reads a Program Map Table section on a PID before the first PAT
    /// says which PIDs carry one: `ffprobe` reads the file again from its
    /// start once it has the PAT, and a stream such a section names is
    /// found there ([`EarlyPmt`]). Only PIDs whose unit starts open a PMT
    /// section are reassembled, at most `max_programs` of them at a time.
    fn early_section(&mut self, pid: u16, payload: &[u8], unit_start: bool) {
        let opens_pmt = unit_start
            && payload
                .first()
                .and_then(|&pointer| payload.get(usize::from(pointer).saturating_add(1)))
                == Some(&PMT);
        if opens_pmt
            && !self.early_sections.contains_key(&pid)
            && self.early_sections.len() < self.max_programs
        {
            self.early_sections.insert(pid, Assembler::new(MAX_SECTION));
        }
        let Some(assembler) = self.early_sections.get_mut(&pid) else {
            return;
        };
        let mut sections = Vec::new();
        let pushed = assembler.push(payload, unit_start, &mut sections);
        // A unit that opens no PMT section (a PES packet) ends the PID's
        // reassembly, once it has completed the section before it.
        if pushed.is_err() || (unit_start && !opens_pmt) {
            self.early_sections.remove(&pid);
        }
        for data in sections {
            let Ok((long, body)) = section::long(&data) else {
                continue;
            };
            if long.table_id != PMT || !long.current {
                continue;
            }
            let Ok(parsed) = section::pmt(body) else {
                continue;
            };
            let known = self
                .early_pmts
                .iter()
                .any(|e| e.pmt_pid == pid && e.number == long.id);
            if !known && self.early_pmts.len() < self.max_programs {
                self.early_pmts.push(EarlyPmt {
                    at: self.current,
                    pmt_pid: pid,
                    number: long.id,
                    pids: parsed.streams.iter().map(|es| es.pid).collect(),
                });
            }
        }
    }

    /// Whether `pid` carries a kept SCTE 35 stream.
    #[inline(never)]
    fn splice_pid(&self, pid: u16) -> bool {
        self.stream_of(pid)
            .and_then(|index| self.streams.get(index))
            .is_some_and(|entry| entry.kept && entry.codec == Codec::Scte35)
    }

    /// Reassembles the splice information sections of an SCTE 35 stream
    /// (ANSI/SCTE 35 9.2, carried as private sections), each to be listed
    /// as a packet of the stream with the time of the last program clock
    /// reference of its program, as `ffprobe` lists them: on a capture of
    /// the corpus, each section is timed by the PCR of the packet before
    /// it on the PCR PID, and on streams crafted with sections before the
    /// first PCR (no time), between PCRs, and across packets, by the last
    /// PCR read on its program's PCR PID, a PCR on a PID no program names
    /// left aside.
    #[inline(never)]
    fn splice_payload(
        &mut self,
        pid: u16,
        payload: &[u8],
        unit_start: bool,
        warnings: &mut Vec<Warning>,
    ) -> Result<(), Error> {
        let mut sections = Vec::new();
        let assembler = self
            .sections
            .entry(pid)
            .or_insert_with(|| Assembler::new(MAX_PRIVATE_SECTION));
        if let Err(reason) = assembler.push(payload, unit_start, &mut sections) {
            warn(warnings, Warning::BadSection { pid, reason });
        }
        let clock = self
            .stream_of(pid)
            .and_then(|index| self.streams.get(index))
            .and_then(|entry| entry.programs.first())
            .and_then(|number| self.programs.iter().find(|p| p.number == *number))
            .and_then(|program| program.pcr_pid)
            .and_then(|pcr_pid| self.pcrs.get(&pcr_pid).copied());
        // A section whose CRC-32 fails is not listed, as `ffprobe` does
        // not list it (a stream crafted with one).
        for data in sections {
            if data.first() == Some(&SPLICE_INFO) && section::crc32(&data) == 0 {
                self.sections_listed.push_back((pid, data, clock));
            }
        }
        Ok(())
    }

    fn section_payload(
        &mut self,
        pid: u16,
        payload: &[u8],
        unit_start: bool,
        warnings: &mut Vec<Warning>,
    ) -> Result<(), Error> {
        let mut sections = Vec::new();
        let assembler = self.sections.entry(pid).or_insert_with(|| {
            Assembler::new(if pid == EIT_PID {
                MAX_EIT_SECTION
            } else {
                MAX_SECTION
            })
        });
        if let Err(reason) = assembler.push(payload, unit_start, &mut sections) {
            warn(warnings, Warning::BadSection { pid, reason });
        }
        for data in sections {
            let (long, body) = match section::long(&data) {
                Ok(parsed) => parsed,
                Err(reason) => {
                    // Sections of other tables (and short-form private
                    // sections) on these PIDs are not ours to read.
                    if matches!(data.first(), Some(&PAT) | Some(&PMT)) {
                        warn(warnings, Warning::BadSection { pid, reason });
                    }
                    continue;
                }
            };
            if !long.current {
                continue;
            }
            match (pid, long.table_id) {
                // The Event Information Table is a table, not an
                // elementary stream, but it is the one piece of service
                // information `ffprobe` lists as a stream; listing it
                // keeps the two readers on the same stream set for a
                // broadcast capture. Its place is where it is first read,
                // as `ffprobe` places it.
                (EIT_PID, EIT_FIRST..=EIT_LAST) => {
                    self.epg()?;
                    let listed = self
                        .by_pid
                        .get(&EIT_PID)
                        .and_then(|&index| self.streams.get(index))
                        .is_some_and(|entry| entry.kept);
                    if self.keep_epg && listed {
                        self.sections_listed
                            .push_back((EIT_PID, data.clone(), None));
                    }
                }
                (SDT_PID, SDT_ACTUAL) => {
                    if self.sdt_before_pat.is_none() {
                        self.sdt_before_pat = Some(true);
                    }
                    // A table of several sections is kept whole: each
                    // section adds its services, a newer one replacing
                    // what an older one said of the same service.
                    for service in section::sdt_services(body) {
                        if let Some(known) = self.services.iter_mut().find(|s| s.id == service.id) {
                            *known = service;
                        } else if self.services.len() < self.max_programs {
                            self.services.push(service);
                        }
                    }
                }
                (0, PAT) => {
                    if self.sdt_before_pat.is_none() {
                        self.sdt_before_pat = Some(false);
                    }
                    self.transport_stream_id = Some(long.id);
                    if self.pat_version != Some(long.version) {
                        self.pat_sections = (0, long.last);
                    }
                    if long.number < 64 {
                        self.pat_sections.0 |= 1 << long.number;
                    }
                    self.pat(long.version, body)?;
                }
                (_, PMT) => self.pmt(pid, long.id, long.version, body, warnings)?,
                _ => {}
            }
        }
        Ok(())
    }

    /// Lists every PID that carried PES packets and that no table
    /// declares, after the tables have been read: its packets are the
    /// file's too, and `ffprobe` lists them, after the declared streams.
    /// `earlier` holds the first PES packet of each PID of an earlier
    /// pass over the file.
    pub(crate) fn add_undeclared(&mut self, earlier: &BTreeMap<u16, u64>) -> Result<(), Error> {
        let mut found: BTreeMap<u16, u64> = BTreeMap::new();
        for (pid, offset) in earlier.iter().chain(self.pes_pids.iter()) {
            if self.by_pid.contains_key(pid)
                || *pid == 0
                || *pid == EIT_PID
                || self.programs.iter().any(|p| p.pmt_pid == *pid)
            {
                continue;
            }
            let at = found.entry(*pid).or_insert(*offset);
            *at = (*at).min(*offset);
        }
        let mut order: Vec<(u64, u16)> = found.into_iter().map(|(pid, at)| (at, pid)).collect();
        order.sort_unstable();
        for (_, pid) in order {
            self.undeclared(pid)?;
        }
        Ok(())
    }

    /// Lists a PID that carries PES packets and that no Program Map Table
    /// declares, with no stream type of its own.
    fn undeclared(&mut self, pid: u16) -> Result<(), Error> {
        if self.streams.len() >= self.max_streams {
            return Err(Error::LimitExceeded {
                limit: LimitKind::Streams,
                offset: 0,
            });
        }
        self.by_pid.insert(pid, self.streams.len());
        self.streams.push(Entry {
            pid,
            programs: Vec::new(),
            stream_type: 0,
            descriptors: Vec::new(),
            format_identifier: None,
            codec: Codec::Other,
            pmt_index: None,
            kept: self.allowlist.reads(Codec::Other),
            sniff: false,
            sniffed: (Vec::new(), 0),
            found_at: u64::MAX,
        });
        Ok(())
    }

    /// Lists the Event Information Table as a data stream, once.
    fn epg(&mut self) -> Result<(), Error> {
        if self.by_pid.contains_key(&EIT_PID) {
            return Ok(());
        }
        if self.streams.len() >= self.max_streams {
            return Err(Error::LimitExceeded {
                limit: LimitKind::Streams,
                offset: 0,
            });
        }
        self.by_pid.insert(EIT_PID, self.streams.len());
        self.streams.push(Entry {
            pid: EIT_PID,
            programs: Vec::new(),
            // No Program Map Table declares it, so it has no stream type;
            // `ffprobe` gives it the codec tag 0 that this stands for.
            stream_type: 0,
            descriptors: Vec::new(),
            format_identifier: None,
            codec: Codec::Epg,
            pmt_index: None,
            kept: self.allowlist.allows(Codec::Epg),
            sniff: false,
            sniffed: (Vec::new(), 0),
            found_at: self.current,
        });
        Ok(())
    }

    fn pat(&mut self, version: u8, body: &[u8]) -> Result<(), Error> {
        self.pat_version = Some(version);
        for (number, pmt_pid) in section::pat_entries(body) {
            if let Some(program) = self.programs.iter_mut().find(|p| p.number == number) {
                // A program on another PMT PID, in whichever section of the
                // PAT names it: its PMT is read again from the new PID.
                if program.pmt_pid != pmt_pid {
                    program.pmt_pid = pmt_pid;
                    program.pmt_version = None;
                }
                continue;
            }
            if self.programs.len() >= self.max_programs {
                return Err(Error::LimitExceeded {
                    limit: LimitKind::Programs,
                    offset: 0,
                });
            }
            self.programs.push(ProgramEntry {
                number,
                pmt_pid,
                pcr_pid: None,
                pmt_version: None,
                pids: Vec::new(),
            });
        }
        Ok(())
    }

    fn pmt(
        &mut self,
        pid: u16,
        number: u16,
        version: u8,
        body: &[u8],
        warnings: &mut Vec<Warning>,
    ) -> Result<(), Error> {
        let Some(program) = self
            .programs
            .iter()
            .position(|p| p.number == number && p.pmt_pid == pid)
        else {
            return Ok(());
        };
        let previous = self.programs.get(program).and_then(|p| p.pmt_version);
        if previous == Some(version) {
            return Ok(());
        }
        let parsed = match section::pmt(body) {
            Ok(parsed) => parsed,
            Err(reason) => {
                warn(warnings, Warning::BadSection { pid, reason });
                return Ok(());
            }
        };
        if previous.is_some() {
            warn(warnings, Warning::PmtChanged { program: number });
        }
        for (place, es) in parsed.streams.iter().enumerate() {
            if let Some(&index) = self.by_pid.get(&es.pid) {
                if let Some(entry) = self.streams.get_mut(index) {
                    if !entry.programs.contains(&number) {
                        entry.programs.push(number);
                    }
                }
                continue;
            }
            if self.streams.len() >= self.max_streams {
                return Err(Error::LimitExceeded {
                    limit: LimitKind::Streams,
                    offset: 0,
                });
            }
            let codec = codec(es.stream_type, &es.descriptors, &parsed.descriptors);
            self.by_pid.insert(es.pid, self.streams.len());
            self.streams.push(Entry {
                pid: es.pid,
                programs: vec![number],
                stream_type: es.stream_type,
                descriptors: es.descriptors.clone(),
                format_identifier: format_identifier(&es.descriptors),
                codec,
                pmt_index: Some(place),
                kept: self.allowlist.reads(codec),
                // `ffprobe` identifies a stream from its payload whatever
                // its type when the type names no codec: a stream of type
                // 0x82 without the Blu-ray registration that carries DTS
                // frames is DTS (crafted files of twenty-three types, and
                // two of the corpus).
                sniff: codec == Codec::Other && crate::codec::sniffed_type(es.stream_type),
                sniffed: (Vec::new(), 0),
                found_at: self.current,
            });
        }
        if let Some(entry) = self.programs.get_mut(program) {
            entry.pmt_version = Some(version);
            if self.first_pmt_at.is_none() {
                self.first_pmt_at = Some(self.current);
            }
            entry.pcr_pid = Some(parsed.pcr_pid);
            for es in parsed.streams {
                if !entry.pids.contains(&es.pid) {
                    entry.pids.push(es.pid);
                }
            }
        }
        Ok(())
    }
}
