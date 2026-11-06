use crate::{
    Error, Limits, NalHeader, Result, SliceHeader, params::Sps, reconstruction::RetainedPicture,
};

pub(crate) struct Emission<'a> {
    pub(crate) event: OutputEvent,
    pub(crate) picture: Option<&'a RetainedPicture>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ReferenceIdentity {
    pub index: u64,
    pub poc: i32,
    pub long_term: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct References {
    pub entries: [Option<ReferenceIdentity>; 16],
    pub before: u8,
    pub after: u8,
    pub long_term: u8,
}

/// An output-order event derived from headers, without reconstructed samples.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OutputEvent {
    /// Full signed picture order count within the coded video sequence.
    pub poc: i32,
    /// Zero-based coded-picture index, never reset at an EOS or IRAP.
    pub picture_index: u64,
    /// Visible luma width after conformance cropping.
    pub width: u32,
    /// Visible luma height after conformance cropping.
    pub height: u32,
    /// Equal luma and chroma sample depth.
    pub bit_depth: u32,
}

/// Picture-order state derived for the first slice of one coded picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PictureOrder {
    /// Full signed picture order count.
    pub poc: i32,
    /// Selected for output after temporal extraction and RASL suppression.
    pub output: bool,
    /// NoRaslOutputFlag of this picture's associated IRAP.
    pub no_rasl_output: bool,
    /// Included in the target decoding set, before any sample reconstruction.
    pub reconstruction_required: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reference {
    Unused,
    Short,
    Long,
}

struct Picture {
    event: OutputEvent,
    reference: Reference,
    output: bool,
    latency: u64,
    temporal_id: u8,
    unit_type: u8,
    generated: bool,
    samples: Option<RetainedPicture>,
}

struct Pending {
    index: u64,
    coded_size: [u32; 2],
    conformance_window: [u32; 4],
    reorder: usize,
    latency: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
struct Irap {
    poc: i32,
    index: u64,
    unit_type: u8,
    no_rasl: bool,
    trailing_seen: bool,
    min_radl: Option<i32>,
    max_rasl: Option<i32>,
    prior_irap_poc: Option<i32>,
    prior_output_max: Option<i32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Key {
    Poc(i32),
    Lsb(u32),
}

#[derive(Clone, Copy, Debug)]
struct Request {
    key: Key,
    used: bool,
    long: bool,
    found: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Reading,
    Finished,
    Failed,
}

pub(crate) struct Dpb {
    pub(crate) warnings: crate::Warnings,
    pictures: [Option<Picture>; 16],
    reconstruction: bool,
    pending: Option<Pending>,
    current_references: References,
    count: u64,
    highest_tid: Option<u32>,
    prev_tid0: Option<(u32, i32)>,
    irap: Option<Irap>,
    after_eos: bool,
    last_output: Option<i32>,
    stsa: [Option<u64>; 7],
    last_lower_tid: [Option<u64>; 7],
    state: State,
}

impl Dpb {
    pub(crate) fn new() -> Self {
        Self {
            warnings: crate::Warnings::default(),
            pictures: [const { None }; 16],
            reconstruction: false,
            pending: None,
            current_references: References::default(),
            count: 0,
            highest_tid: None,
            prev_tid0: None,
            irap: None,
            after_eos: true,
            last_output: None,
            stsa: [None; 7],
            last_lower_tid: [None; 7],
            state: State::Reading,
        }
    }

    pub(crate) fn new_reconstruction() -> Self {
        Self {
            reconstruction: true,
            ..Self::new()
        }
    }

    pub(crate) fn fail(&mut self) {
        self.state = State::Failed;
        self.pictures = [const { None }; 16];
        self.pending = None;
        self.current_references = References::default();
    }

    pub(crate) fn current_references(&self) -> References {
        self.current_references
    }

    pub(crate) fn reconstructs(&self, index: u64) -> bool {
        self.current_identity(index).is_ok()
    }

    pub(crate) fn current_identity(&self, index: u64) -> Result<ReferenceIdentity> {
        self.pictures
            .iter()
            .flatten()
            .find(|p| !p.generated && p.event.picture_index == index)
            .map(|p| ReferenceIdentity {
                index,
                poc: p.event.poc,
                long_term: p.reference == Reference::Long,
            })
            .ok_or(Error::InvalidData("unavailable DPB picture"))
    }

    pub(crate) fn reference(&self, identity: ReferenceIdentity) -> Result<&RetainedPicture> {
        if self.state != State::Reading
            || self
                .pending
                .as_ref()
                .is_some_and(|p| p.index == identity.index)
        {
            return Err(Error::InvalidData("unavailable reference samples"));
        }
        self.pictures
            .iter()
            .flatten()
            .find(|p| {
                !p.generated
                    && p.reference != Reference::Unused
                    && p.event.picture_index == identity.index
                    && p.event.poc == identity.poc
            })
            .and_then(|p| p.samples.as_ref())
            .ok_or(Error::InvalidData("unavailable reference samples"))
    }

    pub(crate) fn heap_bytes(&self) -> Result<usize> {
        self.pictures.iter().flatten().try_fold(0usize, |total, p| {
            let bytes = p
                .samples
                .as_ref()
                .map_or(Ok(0), RetainedPicture::heap_bytes)?;
            total
                .checked_add(bytes)
                .ok_or(Error::Limit("reconstruction_memory"))
        })
    }

    pub(crate) fn attach(&mut self, samples: RetainedPicture) -> Result<()> {
        if self.state != State::Reading {
            return Err(Error::InvalidData("DPB session ended"));
        }
        let result = self.attach_picture(samples);
        if result.is_err() {
            self.fail();
        }
        result
    }

    fn attach_picture(&mut self, samples: RetainedPicture) -> Result<()> {
        let pending = self
            .pending
            .as_ref()
            .ok_or(Error::InvalidData("no pending DPB picture"))?;
        let picture = self
            .pictures
            .iter_mut()
            .flatten()
            .find(|p| !p.generated && p.event.picture_index == pending.index)
            .ok_or(Error::InvalidData("pending DPB picture"))?;
        let info = samples.info();
        if samples.index() != pending.index
            || [info.width, info.height] != pending.coded_size
            || info.conformance_window != pending.conformance_window
            || info.visible_width != picture.event.width
            || info.visible_height != picture.event.height
            || info.bit_depth != picture.event.bit_depth
            || picture.samples.is_some()
        {
            return Err(Error::InvalidData("DPB picture attachment"));
        }
        picture.samples = Some(samples);
        Ok(())
    }

    pub(crate) fn resume(
        &mut self,
        mut emit: impl FnMut(Emission<'_>) -> Result<()>,
    ) -> Result<()> {
        if self.state != State::Reading {
            return Err(Error::InvalidData("DPB session ended"));
        }
        let result = self.resume_picture(&mut emit);
        if result.is_err() {
            self.fail();
        }
        result
    }

    fn resume_picture(&mut self, emit: &mut impl FnMut(Emission<'_>) -> Result<()>) -> Result<()> {
        let Some(pending) = self.pending.as_ref() else {
            return Ok(());
        };
        if !self
            .pictures
            .iter()
            .flatten()
            .any(|p| !p.generated && p.event.picture_index == pending.index && p.samples.is_some())
        {
            return Err(Error::InvalidData("unfinished DPB picture"));
        }
        let (reorder, latency) = (pending.reorder, pending.latency);
        self.pending = None;
        self.additional_bumps(reorder, latency, emit)
    }

    pub(crate) fn end_sequence(&mut self) {
        self.after_eos = true;
    }

    pub(crate) fn starts_cvs(&self, nal: NalHeader) -> bool {
        (16..=21).contains(&nal.unit_type) && (nal.unit_type != 21 || self.after_eos)
    }

    pub(crate) fn start_cvsg(
        &mut self,
        nal: NalHeader,
        header: &SliceHeader,
        mut emit: impl FnMut(Emission<'_>) -> Result<()>,
    ) -> Result<()> {
        if self.state != State::Reading {
            return Err(Error::InvalidData("DPB session ended"));
        }
        if !self.starts_cvs(nal) || nal.temporal_id != 0 || !header.first || header.dependent {
            self.fail();
            return Err(Error::InvalidData("CVSG requires CVS-start IRAP"));
        }
        let result = self
            .prior_output(nal, header, &mut emit)
            .and_then(|()| self.select_cvsg());
        if result.is_err() {
            self.fail();
        }
        result
    }

    fn select_cvsg(&mut self) -> Result<()> {
        if self.state == State::Failed {
            return Err(Error::InvalidData("DPB session ended"));
        }
        if self.fullness() != 0 {
            return Err(Error::InvalidData("CVSG requires drained DPB"));
        }
        *self = Self {
            count: self.count,
            reconstruction: self.reconstruction,
            ..Self::new()
        };
        Ok(())
    }

    pub(crate) fn picture(
        &mut self,
        nal: NalHeader,
        header: &SliceHeader,
        sps: &Sps,
        limits: &Limits,
        mut emit: impl FnMut(Emission<'_>) -> Result<()>,
    ) -> Result<PictureOrder> {
        if self.state != State::Reading {
            return Err(Error::InvalidData("DPB session ended"));
        }
        let result = self.read_picture(nal, header, sps, limits, &mut emit);
        if result.is_err() {
            self.fail();
        }
        result
    }

    pub(crate) fn finish(
        &mut self,
        mut emit: impl FnMut(Emission<'_>) -> Result<()>,
    ) -> Result<()> {
        if self.state != State::Reading {
            return Err(Error::InvalidData("DPB session ended"));
        }
        let result = self.drain(&mut emit);
        if result.is_ok() {
            self.state = State::Finished;
        } else {
            self.fail();
        }
        result
    }

    fn read_picture(
        &mut self,
        nal: NalHeader,
        h: &SliceHeader,
        s: &Sps,
        limits: &Limits,
        emit: &mut impl FnMut(Emission<'_>) -> Result<()>,
    ) -> Result<PictureOrder> {
        self.current_references = References::default();
        if self.pending.is_some() {
            return Err(Error::InvalidData("unfinished DPB picture"));
        }
        if nal.layer_id != 0 {
            return Err(Error::Unsupported("nuh_layer_id"));
        }
        if !matches!(nal.unit_type, 0..=9 | 16..=21) {
            return Err(Error::Unsupported("reserved VCL NAL type"));
        }
        if !h.first || h.dependent {
            return Err(Error::InvalidData("DPB first slice"));
        }
        if u32::from(nal.temporal_id) > s.sub_layers || s.sub_layers > 6 {
            return Err(Error::InvalidData("DPB temporal_id"));
        }
        if s.temporal_nesting && nal.temporal_id > 0 && !matches!(nal.unit_type, 2 | 3 | 6..=9) {
            self.warnings.temporal_nesting_nal_type = true;
        }
        if self.count >= limits.max_pictures {
            return Err(Error::Limit("max_pictures"));
        }
        let irap = (16..=21).contains(&nal.unit_type);
        let idr = matches!(nal.unit_type, 19 | 20);
        if self.after_eos && !irap {
            return Err(Error::InvalidData("missing initial IRAP"));
        }
        if irap && nal.temporal_id != 0 {
            return Err(Error::InvalidData("IRAP temporal_id"));
        }
        let highest = *self.highest_tid.get_or_insert(s.sub_layers);
        if highest > s.sub_layers {
            return Err(Error::InvalidData("HighestTid exceeds SPS sub-layers"));
        }
        let no_rasl = self.starts_cvs(nal);
        let modulus = 1u32
            .checked_shl(s.poc_bits)
            .filter(|_| (4..=16).contains(&s.poc_bits))
            .ok_or(Error::InvalidData("POC bit width"))?;
        let lsb = h.poc_lsb.ok_or(Error::InvalidData("missing POC"))?;
        if lsb >= modulus || idr && lsb != 0 {
            return Err(Error::InvalidData("POC lsb"));
        }
        let (poc, msb) = self.poc(lsb, modulus, no_rasl)?;
        let index = self.count;
        self.count = self
            .count
            .checked_add(1)
            .ok_or(Error::Limit("max_pictures"))?;
        if u32::from(nal.temporal_id) > highest {
            return Ok(PictureOrder {
                poc,
                output: false,
                no_rasl_output: self.irap.is_some_and(|i| i.no_rasl),
                reconstruction_required: false,
            });
        }
        let prior_output_max = if irap && !no_rasl {
            self.pictures
                .iter()
                .flatten()
                .filter(|p| p.output)
                .map(|p| p.event.poc)
                .chain(self.last_output)
                .max()
        } else {
            None
        };
        if prior_output_max.is_some_and(|previous| previous >= poc) {
            return Err(Error::InvalidData("output precedes IRAP"));
        }
        let leading = matches!(nal.unit_type, 6..=9);
        if !irap {
            self.leading(nal.unit_type, poc)?;
        }
        let suppressed = matches!(nal.unit_type, 8 | 9) && self.irap.is_some_and(|i| i.no_rasl);
        let output = h
            .output
            .ok_or(Error::InvalidData("missing pic_output_flag"))?
            && !suppressed;
        if suppressed {
            return Ok(PictureOrder {
                poc,
                output: false,
                no_rasl_output: true,
                reconstruction_required: false,
            });
        }
        let highest = highest as usize;
        let capacity = *s
            .max_dpb
            .get(highest)
            .ok_or(Error::InvalidData("DPB HighestTid"))? as usize;
        let reorder = *s
            .max_reorder
            .get(highest)
            .ok_or(Error::InvalidData("DPB HighestTid"))? as usize;
        let increase = *s
            .max_latency
            .get(highest)
            .ok_or(Error::InvalidData("DPB HighestTid"))?;
        if capacity == 0 || capacity > 16 || reorder >= capacity {
            return Err(Error::InvalidData("DPB ordering limits"));
        }
        let latency = if increase == 0 {
            None
        } else {
            Some(reorder as u64 + u64::from(increase) - 1)
        };
        let (mut requests, request_count) = Self::requests(h, poc, modulus, idr)?;
        let tid_capacity = *s
            .max_dpb
            .get(nal.temporal_id as usize)
            .ok_or(Error::InvalidData("DPB temporal_id"))?;
        if request_count >= tid_capacity as usize {
            return Err(Error::InvalidData("RPS exceeds temporal DPB"));
        }
        if irap && requests.iter().flatten().any(|r| r.used) {
            return Err(Error::InvalidData("IRAP current references"));
        }
        if h.slice_type.is_some_and(|kind| kind < 2) && !requests.iter().flatten().any(|r| r.used) {
            return Err(Error::InvalidData("empty current RPS"));
        }
        if no_rasl {
            for picture in self.pictures.iter_mut().flatten() {
                picture.reference = Reference::Unused;
            }
        }
        self.references(&mut requests, nal, s, poc, irap)?;
        if no_rasl {
            self.prior_output(nal, h, emit)?;
            self.last_output = None;
            self.stsa = [None; 7];
            self.last_lower_tid = [None; 7];
        } else {
            self.remove_unused();
            while self.fullness() >= capacity || self.must_bump(reorder, latency) {
                self.bump(emit)?;
            }
        }
        if matches!(nal.unit_type, 16..=18) || nal.unit_type == 21 && no_rasl {
            for request in requests.iter().flatten().filter(|r| r.found.is_none()) {
                let generated_poc = match request.key {
                    Key::Poc(p) => p,
                    Key::Lsb(p) => p as i32,
                };
                self.store(Picture {
                    event: OutputEvent {
                        poc: generated_poc,
                        picture_index: index,
                        width: s.visible_width,
                        height: s.visible_height,
                        bit_depth: s.bit_depth,
                    },
                    reference: if request.long {
                        Reference::Long
                    } else {
                        Reference::Short
                    },
                    output: false,
                    latency: 0,
                    temporal_id: nal.temporal_id,
                    unit_type: nal.unit_type,
                    generated: true,
                    samples: None,
                })?;
            }
        }
        if self.fullness() >= capacity {
            return Err(Error::InvalidData("DPB full with references"));
        }
        if self.pictures.iter().flatten().any(|p| p.event.poc == poc) {
            return Err(Error::InvalidData("duplicate DPB POC"));
        }
        if output {
            if self.last_output.is_some_and(|last| poc <= last) {
                return Err(Error::InvalidData("late output POC"));
            }
            for p in self
                .pictures
                .iter_mut()
                .flatten()
                .filter(|p| p.output && p.event.poc > poc)
            {
                p.latency = p
                    .latency
                    .checked_add(1)
                    .ok_or(Error::Limit("picture latency"))?;
            }
        }
        self.store(Picture {
            event: OutputEvent {
                poc,
                picture_index: index,
                width: s.visible_width,
                height: s.visible_height,
                bit_depth: s.bit_depth,
            },
            reference: Reference::Short,
            output,
            latency: 0,
            temporal_id: nal.temporal_id,
            unit_type: nal.unit_type,
            generated: false,
            samples: None,
        })?;
        if self.reconstruction {
            self.pending = Some(Pending {
                index,
                coded_size: [s.width, s.height],
                conformance_window: s.conformance_window,
                reorder,
                latency,
            });
        }
        self.additional_bumps(reorder, latency, emit)?;
        if irap {
            self.irap = Some(Irap {
                poc,
                index,
                unit_type: nal.unit_type,
                no_rasl,
                trailing_seen: false,
                min_radl: None,
                max_rasl: None,
                prior_irap_poc: if no_rasl {
                    None
                } else {
                    self.irap.map(|i| i.poc)
                },
                prior_output_max,
            });
        }
        if matches!(nal.unit_type, 4 | 5) {
            *self
                .stsa
                .get_mut(nal.temporal_id as usize)
                .ok_or(Error::InvalidData("STSA temporal_id"))? = Some(index);
        }
        for (tid, lower) in self.last_lower_tid.iter_mut().enumerate() {
            if usize::from(nal.temporal_id) < tid {
                *lower = Some(index);
            }
        }
        if nal.temporal_id == 0 && !leading && !(nal.unit_type <= 9 && nal.unit_type % 2 == 0) {
            self.prev_tid0 = Some((lsb, msb));
        }
        self.after_eos = false;
        Ok(PictureOrder {
            poc,
            output,
            reconstruction_required: true,
            no_rasl_output: if irap {
                no_rasl
            } else {
                self.irap.is_some_and(|i| i.no_rasl)
            },
        })
    }

    fn poc(&self, lsb: u32, modulus: u32, reset: bool) -> Result<(i32, i32)> {
        let msb = if reset {
            0
        } else {
            let (prev_lsb, prev_msb) = self
                .prev_tid0
                .ok_or(Error::InvalidData("missing prevTid0Pic"))?;
            let delta = if lsb < prev_lsb && prev_lsb - lsb >= modulus / 2 {
                i64::from(modulus)
            } else if lsb > prev_lsb && lsb - prev_lsb > modulus / 2 {
                -i64::from(modulus)
            } else {
                0
            };
            i64::from(prev_msb) + delta
        };
        let poc =
            i32::try_from(msb + i64::from(lsb)).map_err(|_| Error::InvalidData("POC range"))?;
        let msb = i32::try_from(msb).map_err(|_| Error::InvalidData("POC MSB range"))?;
        Ok((poc, msb))
    }

    fn leading(&mut self, unit_type: u8, poc: i32) -> Result<()> {
        let irap = self
            .irap
            .as_mut()
            .ok_or(Error::InvalidData("missing associated IRAP"))?;
        let radl = matches!(unit_type, 6 | 7);
        let rasl = matches!(unit_type, 8 | 9);
        if radl || rasl {
            if poc >= irap.poc || irap.trailing_seen {
                return Err(Error::InvalidData("leading picture order"));
            }
            if rasl && !matches!(irap.unit_type, 16 | 21)
                || radl && matches!(irap.unit_type, 18 | 20)
            {
                return Err(Error::InvalidData("IRAP leading picture type"));
            }
            if radl {
                irap.min_radl = Some(irap.min_radl.map_or(poc, |old| old.min(poc)));
            }
            if rasl {
                irap.max_rasl = Some(irap.max_rasl.map_or(poc, |old| old.max(poc)));
            }
            if irap
                .min_radl
                .zip(irap.max_rasl)
                .is_some_and(|(radl, rasl)| rasl >= radl)
            {
                return Err(Error::InvalidData("RASL RADL output order"));
            }
            if radl
                && irap
                    .prior_output_max
                    .is_some_and(|previous| poc <= previous)
                || rasl && irap.prior_irap_poc.is_some_and(|previous| poc <= previous)
            {
                return Err(Error::InvalidData(
                    "leading picture precedes earlier output",
                ));
            }
        } else {
            if poc <= irap.poc {
                return Err(Error::InvalidData("trailing picture POC"));
            }
            irap.trailing_seen = true;
        }
        Ok(())
    }

    fn requests(
        h: &SliceHeader,
        poc: i32,
        modulus: u32,
        idr: bool,
    ) -> Result<([Option<Request>; 16], usize)> {
        let mut requests = [None; 16];
        let rps = &h.short_term_rps;
        if rps.count > 16 || rps.negative_count > rps.count || h.long_term_count > 16 {
            return Err(Error::InvalidData("RPS count"));
        }
        let count = rps
            .count
            .checked_add(h.long_term_count)
            .filter(|n| *n <= 16)
            .ok_or(Error::InvalidData("RPS count"))?;
        if idr && count != 0 {
            return Err(Error::InvalidData("IDR RPS"));
        }
        for (i, (&delta, &used)) in rps.delta.iter().zip(&rps.used).take(rps.count).enumerate() {
            if delta == 0 || (i < rps.negative_count) != (delta < 0) {
                return Err(Error::InvalidData("RPS delta sign"));
            }
            let value = poc
                .checked_add(delta)
                .ok_or(Error::InvalidData("RPS POC range"))?;
            *requests.get_mut(i).ok_or(Error::InvalidData("RPS count"))? = Some(Request {
                key: Key::Poc(value),
                used,
                long: false,
                found: None,
            });
        }
        for (i, reference) in h.long_term.iter().take(h.long_term_count).enumerate() {
            if reference.poc_lsb >= modulus {
                return Err(Error::InvalidData("long-term POC lsb"));
            }
            let key = if let Some(cycle) = reference.delta_msb_cycle {
                let value = i64::from(reference.poc_lsb) + i64::from(poc)
                    - i64::from(cycle) * i64::from(modulus)
                    - i64::from(poc.rem_euclid(modulus as i32));
                Key::Poc(
                    i32::try_from(value).map_err(|_| Error::InvalidData("long-term POC range"))?,
                )
            } else {
                Key::Lsb(reference.poc_lsb)
            };
            *requests
                .get_mut(rps.count + i)
                .ok_or(Error::InvalidData("RPS count"))? = Some(Request {
                key,
                used: reference.used,
                long: true,
                found: None,
            });
        }
        for (i, a) in requests.iter().flatten().enumerate() {
            for b in requests.iter().flatten().skip(i + 1) {
                let overlap = match (a.key, b.key) {
                    (Key::Poc(x), Key::Poc(y)) => x == y,
                    (Key::Lsb(x), Key::Lsb(y)) => x == y,
                    (Key::Poc(x), Key::Lsb(y)) | (Key::Lsb(y), Key::Poc(x)) => {
                        x.rem_euclid(modulus as i32) as u32 == y
                    }
                };
                if overlap {
                    return Err(Error::InvalidData("duplicate RPS reference"));
                }
            }
        }
        Ok((requests, count))
    }

    fn references(
        &mut self,
        requests: &mut [Option<Request>; 16],
        nal: NalHeader,
        s: &Sps,
        poc: i32,
        irap: bool,
    ) -> Result<()> {
        for long in [true, false] {
            for request in requests.iter_mut().flatten().filter(|r| r.long == long) {
                for (i, picture) in self
                    .pictures
                    .iter()
                    .enumerate()
                    .filter_map(|(i, p)| p.as_ref().map(|p| (i, p)))
                {
                    if picture.reference == Reference::Unused
                        || !long && picture.reference != Reference::Short
                    {
                        continue;
                    }
                    let matches = match request.key {
                        Key::Poc(value) => picture.event.poc == value,
                        Key::Lsb(value) => {
                            picture.event.poc.rem_euclid(1i32 << s.poc_bits) as u32 == value
                        }
                    };
                    if matches {
                        if request.found.is_some() {
                            return Err(Error::InvalidData("ambiguous long-term POC"));
                        }
                        request.found = Some(i);
                    }
                }
                if let Some(i) = request.found {
                    let p = self
                        .pictures
                        .get_mut(i)
                        .and_then(Option::as_mut)
                        .ok_or(Error::InvalidData("RPS picture"))?;
                    if request.long {
                        p.reference = Reference::Long;
                    }
                    if request.long && i64::from(poc) - i64::from(p.event.poc) >= 1 << 24 {
                        return Err(Error::InvalidData("long-term POC distance"));
                    }
                    if matches!(nal.unit_type, 2 | 3) && p.temporal_id >= nal.temporal_id {
                        return Err(Error::InvalidData("TSA reference temporal_id"));
                    }
                    if request.used {
                        if p.temporal_id > nal.temporal_id
                            || p.temporal_id == nal.temporal_id
                                && p.unit_type <= 9
                                && p.unit_type % 2 == 0
                        {
                            return Err(Error::InvalidData("current reference temporal_id"));
                        }
                        if matches!(nal.unit_type, 4 | 5) && p.temporal_id == nal.temporal_id {
                            return Err(Error::InvalidData("STSA current reference"));
                        }
                        if p.temporal_id == nal.temporal_id
                            && self
                                .stsa
                                .get(nal.temporal_id as usize)
                                .copied()
                                .flatten()
                                .is_some_and(|index| p.event.picture_index < index)
                        {
                            return Err(Error::InvalidData("reference precedes STSA"));
                        }
                        if s.temporal_nesting
                            && self
                                .last_lower_tid
                                .get(p.temporal_id as usize)
                                .copied()
                                .flatten()
                                .is_some_and(|index| p.event.picture_index < index)
                        {
                            return Err(Error::InvalidData("temporal nesting reference"));
                        }
                        if !matches!(nal.unit_type, 6..=9) && !irap && p.generated {
                            return Err(Error::InvalidData("unavailable trailing reference"));
                        }
                        if matches!(nal.unit_type, 6 | 7)
                            && (p.generated || matches!(p.unit_type, 8 | 9))
                        {
                            return Err(Error::InvalidData("unavailable RADL reference"));
                        }
                    }
                    if let Some(associated) = self.irap {
                        if (!irap && !matches!(nal.unit_type, 6..=9) || nal.unit_type == 21)
                            && (p.event.picture_index < associated.index
                                || p.event.poc < associated.poc)
                            || request.used
                                && matches!(nal.unit_type, 6 | 7)
                                && p.event.picture_index < associated.index
                        {
                            return Err(Error::InvalidData("reference precedes IRAP"));
                        }
                    }
                } else if request.used {
                    return Err(Error::InvalidData("missing current reference"));
                }
            }
        }
        for (i, picture) in self.pictures.iter_mut().enumerate() {
            if let Some(picture) = picture {
                if !requests.iter().flatten().any(|r| r.found == Some(i)) {
                    picture.reference = Reference::Unused;
                }
            }
        }
        let mut current = References::default();
        let mut count = 0;
        for class in 0..3 {
            for request in requests.iter().flatten().filter(|r| r.used) {
                let picture = request
                    .found
                    .and_then(|index| self.pictures.get(index))
                    .and_then(Option::as_ref)
                    .ok_or(Error::InvalidData("RPS picture"))?;
                let picture_class = if request.long {
                    2
                } else {
                    usize::from(picture.event.poc > poc)
                };
                if picture_class != class {
                    continue;
                }
                *current
                    .entries
                    .get_mut(count)
                    .ok_or(Error::InvalidData("RPS count"))? = Some(ReferenceIdentity {
                    index: picture.event.picture_index,
                    poc: picture.event.poc,
                    long_term: request.long,
                });
                count += 1;
                match class {
                    0 => current.before += 1,
                    1 => current.after += 1,
                    _ => current.long_term += 1,
                }
            }
        }
        self.current_references = current;
        Ok(())
    }

    fn fullness(&self) -> usize {
        self.pictures.iter().flatten().count()
    }

    fn must_bump(&self, reorder: usize, latency: Option<u64>) -> bool {
        self.pictures.iter().flatten().filter(|p| p.output).count() > reorder
            || latency.is_some_and(|limit| {
                self.pictures
                    .iter()
                    .flatten()
                    .any(|p| p.output && p.latency >= limit)
            })
    }

    fn remove_unused(&mut self) {
        for p in &mut self.pictures {
            if p.as_ref()
                .is_some_and(|p| !p.output && p.reference == Reference::Unused)
            {
                *p = None;
            }
        }
    }

    fn store(&mut self, picture: Picture) -> Result<()> {
        let empty = self
            .pictures
            .iter_mut()
            .find(|p| p.is_none())
            .ok_or(Error::InvalidData("DPB capacity"))?;
        *empty = Some(picture);
        Ok(())
    }

    fn next_output(&self) -> Result<usize> {
        self.pictures
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.as_ref().filter(|p| p.output).map(|p| (i, p.event.poc)))
            .min_by_key(|(_, poc)| *poc)
            .map(|(i, _)| i)
            .ok_or(Error::InvalidData("DPB cannot bump references"))
    }

    fn additional_bumps(
        &mut self,
        reorder: usize,
        latency: Option<u64>,
        emit: &mut impl FnMut(Emission<'_>) -> Result<()>,
    ) -> Result<()> {
        while self.must_bump(reorder, latency) {
            let picture = self
                .pictures
                .get(self.next_output()?)
                .and_then(Option::as_ref)
                .ok_or(Error::InvalidData("DPB output picture"))?;
            if self
                .pending
                .as_ref()
                .is_some_and(|p| p.index == picture.event.picture_index)
            {
                break;
            }
            self.bump(emit)?;
        }
        Ok(())
    }

    fn bump(&mut self, emit: &mut impl FnMut(Emission<'_>) -> Result<()>) -> Result<()> {
        let index = self.next_output()?;
        let picture = self
            .pictures
            .get_mut(index)
            .and_then(Option::as_mut)
            .ok_or(Error::InvalidData("DPB output picture"))?;
        if self
            .pending
            .as_ref()
            .is_some_and(|p| p.index == picture.event.picture_index)
            || self.reconstruction && picture.samples.is_none()
        {
            return Err(Error::InvalidData("unfinished DPB picture"));
        }
        if self.last_output.is_some_and(|poc| picture.event.poc <= poc) {
            return Err(Error::InvalidData("output POC order"));
        }
        emit(Emission {
            event: picture.event,
            picture: picture.samples.as_ref(),
        })?;
        self.last_output = Some(picture.event.poc);
        picture.output = false;
        self.remove_unused();
        Ok(())
    }

    fn prior_output(
        &mut self,
        nal: NalHeader,
        header: &SliceHeader,
        emit: &mut impl FnMut(Emission<'_>) -> Result<()>,
    ) -> Result<()> {
        if self.pending.is_some() {
            return Err(Error::InvalidData("unfinished DPB picture"));
        }
        if nal.unit_type == 21 || header.no_output_of_prior_pics {
            self.pictures = [const { None }; 16];
            Ok(())
        } else {
            self.drain(emit)
        }
    }

    fn drain(&mut self, emit: &mut impl FnMut(Emission<'_>) -> Result<()>) -> Result<()> {
        if self.pending.is_some() {
            return Err(Error::InvalidData("unfinished DPB picture"));
        }
        while self.pictures.iter().flatten().any(|p| p.output) {
            self.bump(emit)?;
        }
        self.pictures = [const { None }; 16];
        self.current_references = References::default();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LongTermRef, rps::Rps, vui::Vui};

    fn sps() -> Sps {
        Sps {
            id: 0,
            vps_id: 0,
            width: 64,
            height: 64,
            visible_width: 60,
            visible_height: 62,
            conformance_window: [0, 2, 0, 1],
            bit_depth: 8,
            profile_idc: 1,
            profile_bit_depth_warning: false,
            ctb_log2: 6,
            ctu_count: 1,
            poc_bits: 4,
            rps_count: 0,
            rps: [Rps::default(); 64],
            long_term_present: true,
            long_term_count: 0,
            long_term_used: [false; 32],
            long_term_poc: [0; 32],
            temporal_mvp: false,
            sao: false,
            sub_layers: 0,
            temporal_nesting: false,
            max_dpb: [4; 7],
            max_reorder: [2; 7],
            max_latency: [0; 7],
            min_cb_log2: 3,
            min_tb_log2: 2,
            max_tb_log2: 5,
            max_inter_transform_depth: 0,
            max_intra_transform_depth: 0,
            amp: false,
            strong_smoothing: false,
            pcm: None,
            scaling_lists: None,
            vui: Vui::default(),
        }
    }

    fn header(poc: u32, entries: &[(i32, bool)]) -> SliceHeader {
        let mut rps = Rps {
            count: entries.len(),
            negative_count: entries.iter().filter(|(d, _)| *d < 0).count(),
            ..Rps::default()
        };
        for ((delta, used), &(d, u)) in rps.delta.iter_mut().zip(&mut rps.used).zip(entries) {
            *delta = d;
            *used = u;
        }
        SliceHeader {
            first: true,
            dependent: false,
            pps_id: 0,
            address: 0,
            slice_type: Some(2),
            poc_lsb: Some(poc),
            output: Some(true),
            slice_qp_y: Some(26),
            chroma_qp_offsets: Some([0; 2]),
            cabac_init_flag: Some(false),
            sao: Some([false; 2]),
            deblock_disabled: Some(true),
            deblock_offsets: Some([0; 2]),
            loop_filter_slices: Some(true),
            entry_points: 0,
            rbsp_header_bytes: 0,
            no_output_of_prior_pics: false,
            short_term_rps: rps,
            long_term: [LongTermRef::default(); 16],
            long_term_count: 0,
            reference_syntax: None,
            inter: None,
        }
    }

    fn nal(unit_type: u8) -> NalHeader {
        NalHeader {
            unit_type,
            layer_id: 0,
            temporal_id: 0,
        }
    }

    fn push(dpb: &mut Dpb, kind: u8, h: &SliceHeader, s: &Sps) -> Result<Vec<OutputEvent>> {
        let mut events = Vec::new();
        dpb.picture(nal(kind), h, s, &Limits::default(), |event| {
            events.push(event.event);
            Ok(())
        })?;
        Ok(events)
    }

    fn finish(dpb: &mut Dpb) -> Result<Vec<OutputEvent>> {
        let mut events = Vec::new();
        dpb.finish(|event| {
            events.push(event.event);
            Ok(())
        })?;
        Ok(events)
    }

    fn pocs(events: &[OutputEvent]) -> Vec<i32> {
        events.iter().map(|e| e.poc).collect()
    }

    fn info(s: &Sps) -> crate::PictureInfo {
        crate::PictureInfo {
            width: s.width,
            height: s.height,
            visible_width: s.visible_width,
            visible_height: s.visible_height,
            bit_depth: s.bit_depth,
            profile_idc: s.profile_idc,
            ctb_log2: s.ctb_log2,
            tiles: false,
            wpp: false,
            conformance_window: s.conformance_window,
            vui: s.vui,
        }
    }

    fn complete(dpb: &mut Dpb, index: u64, s: &Sps) -> Result<Vec<OutputEvent>> {
        dpb.attach(RetainedPicture::for_test(
            index,
            info(s),
            Some(index as i64),
        ))?;
        let mut events = Vec::new();
        dpb.resume(|emission| {
            let picture = emission.picture.ok_or(Error::InvalidData("test samples"))?;
            assert_eq!(picture.index(), emission.event.picture_index);
            assert_eq!(picture.timestamp(), Some(picture.index() as i64));
            events.push(emission.event);
            Ok(())
        })?;
        Ok(events)
    }

    #[test]
    fn reconstructed_samples_wait_for_attachment_and_suffix_boundary() -> Result<()> {
        let mut s = sps();
        s.max_reorder = [0; 7];
        let mut dpb = Dpb::new_reconstruction();
        assert_eq!(dpb.heap_bytes()?, 0);
        assert!(push(&mut dpb, 19, &header(0, &[]), &s)?.is_empty());
        assert!(dpb.reconstructs(0));
        assert_eq!(dpb.heap_bytes()?, 0);
        let identity = ReferenceIdentity {
            index: 0,
            poc: 0,
            long_term: false,
        };
        assert_eq!(dpb.current_identity(0)?, identity);
        assert!(dpb.reference(identity).is_err());
        let samples = RetainedPicture::for_test(0, info(&s), Some(123));
        let bytes = samples.heap_bytes()?;
        assert!(bytes > 0);
        dpb.attach(samples)?;
        assert_eq!(dpb.heap_bytes()?, bytes);
        assert!(dpb.reference(identity).is_err());
        assert_eq!(dpb.last_output, None);
        let mut events = Vec::new();
        dpb.resume(|emission| {
            let picture = emission.picture.ok_or(Error::InvalidData("test samples"))?;
            assert_eq!(picture.index(), 0);
            assert_eq!(picture.timestamp(), Some(123));
            assert_eq!(picture.info(), info(&s));
            assert_eq!(
                picture
                    .view()
                    .planes
                    .first()
                    .and_then(|p| p.samples.first()),
                Some(&42)
            );
            events.push(emission.event);
            Ok(())
        })?;
        assert_eq!(pocs(&events), [0]);
        assert_eq!(dpb.reference(identity)?.index(), 0);
        assert_eq!(dpb.heap_bytes()?, bytes);
        dpb.resume(|_| Err(Error::InvalidData("duplicate output")))?;
        assert!(finish(&mut dpb)?.is_empty());
        assert_eq!(dpb.heap_bytes()?, 0);
        assert!(!dpb.reconstructs(0));
        assert!(dpb.reference(identity).is_err());
        Ok(())
    }

    #[test]
    fn eager_bumps_emit_ready_predecessors_but_stop_at_pending_samples() -> Result<()> {
        let mut s = sps();
        s.max_reorder = [1; 7];
        let mut dpb = Dpb::new_reconstruction();
        assert!(push(&mut dpb, 19, &header(0, &[]), &s)?.is_empty());
        assert!(complete(&mut dpb, 0, &s)?.is_empty());
        let bytes = dpb.heap_bytes()?;
        assert_eq!(
            pocs(&push(&mut dpb, 1, &header(3, &[(-3, false)]), &s)?),
            [0]
        );
        assert_eq!(dpb.heap_bytes()?, bytes);
        assert!(complete(&mut dpb, 1, &s)?.is_empty());
        assert_eq!(dpb.heap_bytes()?, 2 * bytes);
        assert!(push(&mut dpb, 1, &header(1, &[(2, true)]), &s)?.is_empty());
        assert!(!dpb.reconstructs(0));
        assert_eq!(dpb.heap_bytes()?, bytes);
        assert_eq!(pocs(&complete(&mut dpb, 2, &s)?), [1]);
        assert_eq!(pocs(&finish(&mut dpb)?), [3]);
        assert_eq!(dpb.heap_bytes()?, 0);
        Ok(())
    }

    #[test]
    fn resumed_reorder_and_latency_bumps_match_header_mode() -> Result<()> {
        let mut s = sps();
        s.max_latency = [1; 7];
        let mut headers = Dpb::new();
        let mut decoded = Dpb::new_reconstruction();
        for (index, (kind, poc)) in [(19, 0), (1, 5), (1, 3), (1, 1)].into_iter().enumerate() {
            let h = header(poc, &[]);
            let expected = push(&mut headers, kind, &h, &s)?;
            let mut actual = push(&mut decoded, kind, &h, &s)?;
            actual.extend(complete(&mut decoded, index as u64, &s)?);
            assert_eq!(actual, expected);
            if index == 3 {
                assert_eq!(pocs(&actual), [1, 3, 5]);
            }
        }
        assert_eq!(finish(&mut decoded)?, finish(&mut headers)?);
        assert_eq!(decoded.heap_bytes()?, 0);
        Ok(())
    }

    #[test]
    fn unfinished_pictures_cannot_be_skipped_by_first_slice_drain_or_cvsg() -> Result<()> {
        let s = sps();
        for output in [false, true] {
            for attached in [false, true] {
                for operation in 0..4 {
                    let mut dpb = Dpb::new_reconstruction();
                    let mut h = header(0, &[]);
                    h.output = Some(output);
                    push(&mut dpb, 19, &h, &s)?;
                    if attached {
                        dpb.attach(RetainedPicture::for_test(0, info(&s), None))?;
                    }
                    let mut calls = 0;
                    let mut emit = |_: Emission<'_>| {
                        calls += 1;
                        Ok(())
                    };
                    let result = match operation {
                        0 => dpb
                            .picture(nal(1), &header(1, &[]), &s, &Limits::default(), &mut emit)
                            .map(|_| ()),
                        1 => dpb.finish(&mut emit),
                        2 => dpb.start_cvsg(nal(19), &header(0, &[]), &mut emit),
                        _ => {
                            let mut h = header(0, &[]);
                            h.no_output_of_prior_pics = true;
                            dpb.start_cvsg(nal(19), &h, &mut emit)
                        }
                    };
                    assert_eq!(result, Err(Error::InvalidData("unfinished DPB picture")));
                    assert_eq!(calls, 0);
                    assert_eq!(dpb.heap_bytes()?, 0);
                    assert!(!dpb.reconstructs(0));
                    assert_eq!(
                        dpb.finish(|_| Ok(())),
                        Err(Error::InvalidData("DPB session ended"))
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn attachments_validate_identity_geometry_uniqueness_and_completion() -> Result<()> {
        let s = sps();
        for field in 0..8 {
            let mut dpb = Dpb::new_reconstruction();
            push(&mut dpb, 19, &header(0, &[]), &s)?;
            let mut format = info(&s);
            let index = u64::from(field == 0);
            match field {
                1 => format.width = 32,
                2 => format.height = 32,
                3 => format.visible_width -= 2,
                4 => format.visible_height -= 2,
                5 => format.bit_depth = 10,
                6 => format.conformance_window = [1, 1, 0, 1],
                7 => dpb.attach(RetainedPicture::for_test(0, format, None))?,
                _ => {}
            }
            assert_eq!(
                dpb.attach(RetainedPicture::for_test(index, format, None)),
                Err(Error::InvalidData("DPB picture attachment"))
            );
            assert_eq!(dpb.heap_bytes()?, 0);
            assert_eq!(dpb.fullness(), 0);
            assert_eq!(
                dpb.resume(|_| Ok(())),
                Err(Error::InvalidData("DPB session ended"))
            );
        }
        let mut dpb = Dpb::new_reconstruction();
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        assert_eq!(
            dpb.resume(|_| Ok(())),
            Err(Error::InvalidData("unfinished DPB picture"))
        );
        assert_eq!(dpb.heap_bytes()?, 0);
        let mut headers = Dpb::new();
        push(&mut headers, 19, &header(0, &[]), &s)?;
        assert_eq!(headers.heap_bytes()?, 0);
        assert_eq!(
            headers.attach(RetainedPicture::for_test(0, info(&s), None)),
            Err(Error::InvalidData("no pending DPB picture"))
        );
        Ok(())
    }

    #[test]
    fn callback_failures_and_explicit_failure_release_all_retained_samples() -> Result<()> {
        let mut s = sps();
        for operation in 0..5 {
            let mut dpb = Dpb::new_reconstruction();
            s.max_reorder = [if operation == 0 { 0 } else { 1 }; 7];
            push(&mut dpb, 19, &header(0, &[]), &s)?;
            dpb.attach(RetainedPicture::for_test(0, info(&s), None))?;
            if operation != 0 {
                dpb.resume(|_| Ok(()))?;
            }
            assert!(dpb.heap_bytes()? > 0);
            let mut calls = 0;
            let mut fail = |emission: Emission<'_>| {
                assert!(emission.picture.is_some());
                calls += 1;
                Err(Error::Limit("test callback"))
            };
            let result = match operation {
                0 => dpb.resume(&mut fail),
                1 => dpb
                    .picture(nal(1), &header(1, &[]), &s, &Limits::default(), &mut fail)
                    .map(|_| ()),
                2 => dpb.finish(&mut fail),
                3 => dpb.start_cvsg(nal(19), &header(0, &[]), &mut fail),
                _ => {
                    dpb.fail();
                    Err(Error::Limit("test callback"))
                }
            };
            assert_eq!(result, Err(Error::Limit("test callback")));
            assert_eq!(calls, usize::from(operation != 4));
            assert_eq!(dpb.heap_bytes()?, 0);
            assert_eq!(dpb.fullness(), 0);
            assert_eq!(dpb.current_references(), References::default());
            assert_eq!(
                dpb.finish(|_| Ok(())),
                Err(Error::InvalidData("DPB session ended"))
            );
        }
        Ok(())
    }

    #[test]
    fn prior_output_rules_release_samples_and_preserve_reconstruction_mode() -> Result<()> {
        let s = sps();
        for (kind, discard) in [(19, false), (19, true), (21, false)] {
            let mut dpb = Dpb::new_reconstruction();
            push(&mut dpb, 19, &header(0, &[]), &s)?;
            complete(&mut dpb, 0, &s)?;
            assert!(dpb.heap_bytes()? > 0);
            let mut next = header(0, &[]);
            next.no_output_of_prior_pics = discard;
            if kind == 21 {
                dpb.end_sequence();
            }
            let mut events = Vec::new();
            dpb.start_cvsg(nal(kind), &next, |emission| {
                assert_eq!(emission.picture.map(RetainedPicture::index), Some(0));
                events.push(emission.event);
                Ok(())
            })?;
            assert_eq!(events.len(), usize::from(kind != 21 && !discard));
            assert_eq!(dpb.heap_bytes()?, 0);
            assert_eq!(dpb.fullness(), 0);
            assert_eq!(dpb.count, 1);
            assert!(dpb.reconstruction);
            assert_eq!(dpb.current_references(), References::default());
            assert!(push(&mut dpb, kind, &next, &s)?.is_empty());
            assert!(dpb.reconstructs(1));
            complete(&mut dpb, 1, &s)?;
            assert_eq!(pocs(&finish(&mut dpb)?), [0]);
            assert_eq!(dpb.heap_bytes()?, 0);
        }
        Ok(())
    }

    #[test]
    fn current_reference_snapshot_follows_normative_classes_and_request_order() -> Result<()> {
        let mut s = sps();
        s.poc_bits = 8;
        s.max_dpb = [16; 7];
        s.max_reorder = [15; 7];
        let mut dpb = Dpb::new_reconstruction();
        let previous = [0, 2, 4, 8, 10, 12, 14];
        for (index, &poc) in previous.iter().enumerate() {
            let entries: Vec<_> = previous
                .iter()
                .take(index)
                .rev()
                .map(|&p| (p - poc, false))
                .collect();
            let mut h = header(poc as u32, &entries);
            h.output = Some(false);
            assert!(push(&mut dpb, if index == 0 { 19 } else { 1 }, &h, &s)?.is_empty());
            complete(&mut dpb, index as u64, &s)?;
        }
        let mut h = header(
            6,
            &[(-2, true), (-4, false), (-6, true), (2, true), (6, true)],
        );
        h.output = Some(false);
        h.long_term_count = 2;
        for (entry, lsb) in h.long_term.iter_mut().zip([14, 10]) {
            *entry = LongTermRef {
                poc_lsb: lsb,
                used: true,
                delta_msb_cycle: None,
            };
        }
        push(&mut dpb, 1, &h, &s)?;
        let refs = dpb.current_references();
        assert_eq!((refs.before, refs.after, refs.long_term), (2, 2, 2));
        let expected = [
            (2, 4, false),
            (0, 0, false),
            (3, 8, false),
            (5, 12, false),
            (6, 14, true),
            (4, 10, true),
        ];
        for (actual, (index, poc, long_term)) in refs.entries.iter().zip(expected) {
            let identity = ReferenceIdentity {
                index,
                poc,
                long_term,
            };
            assert_eq!(*actual, Some(identity));
            assert_eq!(dpb.reference(identity)?.index(), index);
        }
        assert!(refs.entries.iter().skip(6).all(Option::is_none));
        assert!(dpb.reconstructs(1));
        complete(&mut dpb, 7, &s)?;
        let mut next = header(7, &[(-7, true)]);
        next.output = Some(false);
        next.long_term_count = 1;
        if let Some(entry) = next.long_term.first_mut() {
            *entry = LongTermRef {
                poc_lsb: 4,
                used: true,
                delta_msb_cycle: None,
            };
        }
        push(&mut dpb, 1, &next, &s)?;
        let new = dpb.current_references();
        assert_eq!((new.before, new.after, new.long_term), (1, 0, 1));
        assert_eq!(
            new.entries.get(1),
            Some(&Some(ReferenceIdentity {
                index: 2,
                poc: 4,
                long_term: true
            }))
        );
        assert!(dpb.current_identity(2)?.long_term);
        assert_eq!(
            refs.entries.first(),
            Some(&Some(ReferenceIdentity {
                index: 2,
                poc: 4,
                long_term: false
            }))
        );
        complete(&mut dpb, 8, &s)?;
        finish(&mut dpb)?;
        assert_eq!(dpb.heap_bytes()?, 0);
        Ok(())
    }

    #[test]
    fn reference_identity_never_aliases_a_recycled_slot_or_repeated_poc() -> Result<()> {
        let mut s = sps();
        s.max_reorder = [0; 7];
        let mut dpb = Dpb::new_reconstruction();
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        complete(&mut dpb, 0, &s)?;
        let old = ReferenceIdentity {
            index: 0,
            poc: 0,
            long_term: false,
        };
        assert_eq!(dpb.reference(old)?.index(), 0);
        assert!(dpb.reference(ReferenceIdentity { poc: 1, ..old }).is_err());
        assert!(
            dpb.reference(ReferenceIdentity { index: 1, ..old })
                .is_err()
        );
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        assert!(dpb.reference(old).is_err());
        assert!(!dpb.reconstructs(0));
        complete(&mut dpb, 1, &s)?;
        assert!(dpb.reference(old).is_err());
        assert_eq!(
            dpb.reference(ReferenceIdentity { index: 1, ..old })?
                .index(),
            1
        );
        dpb.fail();
        assert!(
            dpb.reference(ReferenceIdentity { index: 1, ..old })
                .is_err()
        );
        assert_eq!(dpb.heap_bytes()?, 0);
        Ok(())
    }

    #[test]
    fn generated_unavailable_references_never_acquire_or_alias_pixels() -> Result<()> {
        let mut s = sps();
        s.max_reorder = [0; 7];
        let mut dpb = Dpb::new_reconstruction();
        push(&mut dpb, 21, &header(8, &[(-3, false)]), &s)?;
        assert_eq!(dpb.heap_bytes()?, 0);
        assert_eq!(dpb.fullness(), 2);
        complete(&mut dpb, 0, &s)?;
        let unavailable = ReferenceIdentity {
            index: 0,
            poc: 5,
            long_term: false,
        };
        assert!(dpb.reference(unavailable).is_err());
        assert_eq!(dpb.current_references(), References::default());
        let mut radl = header(7, &[(-2, false)]);
        radl.output = Some(false);
        push(&mut dpb, 7, &radl, &s)?;
        assert!(!dpb.reconstructs(0));
        assert!(dpb.reconstructs(1));
        assert!(dpb.current_identity(0).is_err());
        assert_eq!(dpb.heap_bytes()?, 0);
        assert!(dpb.reference(unavailable).is_err());
        complete(&mut dpb, 1, &s)?;
        assert!(dpb.reference(unavailable).is_err());
        assert!(
            dpb.pictures
                .iter()
                .flatten()
                .filter(|p| p.generated)
                .all(|p| p.samples.is_none())
        );
        finish(&mut dpb)?;
        assert_eq!(dpb.heap_bytes()?, 0);
        Ok(())
    }

    #[test]
    fn extracted_and_suppressed_pictures_reset_current_reference_snapshot() -> Result<()> {
        let mut s = sps();
        let mut dpb = Dpb::new();
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        push(&mut dpb, 1, &header(1, &[(-1, true)]), &s)?;
        assert_eq!(dpb.current_references().before, 1);
        s.sub_layers = 1;
        let mut n = nal(1);
        n.temporal_id = 1;
        dpb.picture(n, &header(2, &[]), &s, &Limits::default(), |_| Ok(()))?;
        assert!(!dpb.reconstructs(2));
        assert_eq!(dpb.current_references(), References::default());
        assert_eq!(dpb.heap_bytes()?, 0);
        let mut dpb = Dpb::new();
        push(&mut dpb, 21, &header(4, &[]), &s)?;
        push(&mut dpb, 7, &header(3, &[(1, true)]), &s)?;
        assert_eq!(dpb.current_references().after, 1);
        push(&mut dpb, 9, &header(2, &[]), &s)?;
        assert!(!dpb.reconstructs(2));
        assert_eq!(dpb.current_references(), References::default());
        Ok(())
    }

    #[test]
    fn poc_wrap_uses_asymmetric_half_range_thresholds() -> Result<()> {
        let mut dpb = Dpb::new();
        dpb.prev_tid0 = Some((8, 0));
        assert_eq!(dpb.poc(0, 16, false)?, (16, 16));
        dpb.prev_tid0 = Some((0, 16));
        assert_eq!(dpb.poc(8, 16, false)?, (24, 16));
        assert_eq!(dpb.poc(9, 16, false)?, (9, 0));
        assert_eq!(dpb.poc(15, 16, true)?, (15, 0));
        dpb.prev_tid0 = Some((15, i32::MAX - 15));
        assert_eq!(dpb.poc(0, 16, false), Err(Error::InvalidData("POC range")));
        Ok(())
    }

    #[test]
    fn prev_tid0_excludes_higher_temporal_and_slnr_pictures() -> Result<()> {
        let mut dpb = Dpb::new();
        let mut s = sps();
        s.sub_layers = 1;
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        let mut n = nal(1);
        n.temporal_id = 1;
        dpb.picture(n, &header(7, &[]), &s, &Limits::default(), |_| Ok(()))?;
        push(&mut dpb, 0, &header(6, &[]), &s)?;
        assert_eq!(dpb.poc(9, 16, false)?, (-7, -16));
        Ok(())
    }

    #[test]
    fn output_is_sorted_and_bound_to_the_original_picture_index() -> Result<()> {
        let mut dpb = Dpb::new();
        let s = sps();
        let mut events = push(&mut dpb, 19, &header(0, &[]), &s)?;
        events.extend(push(&mut dpb, 1, &header(3, &[]), &s)?);
        events.extend(push(&mut dpb, 1, &header(1, &[]), &s)?);
        events.extend(push(&mut dpb, 1, &header(2, &[]), &s)?);
        events.extend(finish(&mut dpb)?);
        assert_eq!(pocs(&events), [0, 1, 2, 3]);
        assert_eq!(
            events.iter().map(|e| e.picture_index).collect::<Vec<_>>(),
            [0, 2, 3, 1]
        );
        Ok(())
    }

    #[test]
    fn pic_output_flag_does_not_remove_a_usable_reference() -> Result<()> {
        let mut dpb = Dpb::new();
        let s = sps();
        let mut hidden = header(0, &[]);
        hidden.output = Some(false);
        assert!(push(&mut dpb, 19, &hidden, &s)?.is_empty());
        let mut predicted = header(1, &[(-1, true)]);
        predicted.slice_type = Some(1);
        push(&mut dpb, 1, &predicted, &s)?;
        assert_eq!(pocs(&finish(&mut dpb)?), [1]);
        Ok(())
    }

    #[test]
    fn missing_current_reference_fails_and_ends_the_session() -> Result<()> {
        let mut dpb = Dpb::new();
        let s = sps();
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        assert_eq!(
            push(&mut dpb, 1, &header(2, &[(-1, true)]), &s),
            Err(Error::InvalidData("missing current reference"))
        );
        assert_eq!(
            finish(&mut dpb),
            Err(Error::InvalidData("DPB session ended"))
        );
        Ok(())
    }

    #[test]
    fn missing_foll_is_allowed_and_present_foll_is_retained() -> Result<()> {
        let mut dpb = Dpb::new();
        let s = sps();
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        push(&mut dpb, 1, &header(2, &[(-1, false), (-2, false)]), &s)?;
        push(&mut dpb, 1, &header(3, &[(-3, true)]), &s)?;
        assert_eq!(pocs(&finish(&mut dpb)?), [2, 3]);
        Ok(())
    }

    #[test]
    fn long_term_cycle_resolves_a_wrapped_short_term_picture() -> Result<()> {
        let mut dpb = Dpb::new();
        let s = sps();
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        push(&mut dpb, 1, &header(7, &[(-7, false)]), &s)?;
        push(&mut dpb, 1, &header(14, &[(-14, false)]), &s)?;
        let mut h = header(1, &[]);
        h.long_term_count = 1;
        if let Some(r) = h.long_term.first_mut() {
            *r = LongTermRef {
                poc_lsb: 0,
                used: true,
                delta_msb_cycle: Some(1),
            };
        }
        let order = dpb.picture(nal(1), &h, &s, &Limits::default(), |_| Ok(()))?;
        assert_eq!(order.poc, 17);
        assert!(
            dpb.pictures
                .iter()
                .flatten()
                .any(|p| p.event.picture_index == 0 && p.reference == Reference::Long)
        );
        Ok(())
    }

    #[test]
    fn ambiguous_long_term_lsb_is_invalid() -> Result<()> {
        let mut dpb = Dpb::new();
        let s = sps();
        for (kind, poc, delta) in [
            (19, 0, None),
            (1, 7, Some(-7)),
            (1, 14, Some(-14)),
            (1, 0, Some(-16)),
        ] {
            let entries = delta.map(|d| [(d, false)]);
            let mut h = header(poc, entries.as_ref().map_or(&[], |a| a.as_slice()));
            h.output = Some(false);
            push(&mut dpb, kind, &h, &s)?;
        }
        let mut h = header(1, &[]);
        h.long_term_count = 1;
        if let Some(r) = h.long_term.first_mut() {
            *r = LongTermRef {
                poc_lsb: 0,
                used: true,
                delta_msb_cycle: None,
            };
        }
        assert_eq!(
            push(&mut dpb, 1, &h, &s),
            Err(Error::InvalidData("ambiguous long-term POC"))
        );
        Ok(())
    }

    #[test]
    fn initial_cra_generates_foll_and_suppresses_rasl() -> Result<()> {
        let mut dpb = Dpb::new();
        let s = sps();
        let cra = dpb.picture(
            nal(21),
            &header(8, &[(-3, false)]),
            &s,
            &Limits::default(),
            |_| Ok(()),
        )?;
        assert!(cra.no_rasl_output);
        assert!(
            dpb.pictures
                .iter()
                .flatten()
                .any(|p| p.generated && p.event.poc == 5)
        );
        let rasl = dpb.picture(
            nal(9),
            &header(5, &[(-1, true)]),
            &s,
            &Limits::default(),
            |_| Ok(()),
        )?;
        assert_eq!(
            rasl,
            PictureOrder {
                poc: 5,
                output: false,
                no_rasl_output: true,
                reconstruction_required: false,
            }
        );
        push(&mut dpb, 1, &header(9, &[(-1, true)]), &s)?;
        let output = finish(&mut dpb)?;
        assert_eq!(pocs(&output), [8, 9]);
        assert_eq!(
            output.iter().map(|e| e.picture_index).collect::<Vec<_>>(),
            [0, 2]
        );
        Ok(())
    }

    #[test]
    fn generated_references_are_not_available_to_trailing_or_radl() -> Result<()> {
        for kind in [1, 7] {
            let mut dpb = Dpb::new();
            let s = sps();
            push(&mut dpb, 16, &header(8, &[(-3, false)]), &s)?;
            let (poc, delta, error) = if kind == 1 {
                (9, -4, "unavailable trailing reference")
            } else {
                (6, -1, "unavailable RADL reference")
            };
            assert_eq!(
                push(&mut dpb, kind, &header(poc, &[(delta, true)]), &s),
                Err(Error::InvalidData(error))
            );
        }
        Ok(())
    }

    #[test]
    fn no_output_prior_discards_but_false_flushes_in_order() -> Result<()> {
        for discard in [false, true] {
            let mut dpb = Dpb::new();
            let s = sps();
            push(&mut dpb, 19, &header(0, &[]), &s)?;
            push(&mut dpb, 1, &header(2, &[]), &s)?;
            let mut h = header(0, &[]);
            h.no_output_of_prior_pics = discard;
            let output = push(&mut dpb, 19, &h, &s)?;
            assert_eq!(pocs(&output), if discard { vec![] } else { vec![0, 2] });
            let rest = finish(&mut dpb)?;
            assert_eq!(pocs(&rest), [0]);
            assert_eq!(rest.first().map(|e| e.picture_index), Some(2));
        }
        Ok(())
    }

    #[test]
    fn eos_resets_random_access_but_not_picture_indices() -> Result<()> {
        let mut dpb = Dpb::new();
        let s = sps();
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        dpb.end_sequence();
        let cra = dpb.picture(nal(21), &header(4, &[]), &s, &Limits::default(), |_| Ok(()))?;
        assert!(cra.no_rasl_output);
        let output = finish(&mut dpb)?;
        assert_eq!(pocs(&output), [4]);
        assert_eq!(output.first().map(|e| e.picture_index), Some(1));
        Ok(())
    }

    #[test]
    fn ordinary_cra_does_not_discard_the_previous_cvs_output() -> Result<()> {
        let mut dpb = Dpb::new();
        let s = sps();
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        let mut h = header(4, &[]);
        h.no_output_of_prior_pics = true;
        let cra = dpb.picture(nal(21), &h, &s, &Limits::default(), |_| Ok(()))?;
        assert!(!cra.no_rasl_output);
        assert_eq!(pocs(&finish(&mut dpb)?), [0, 4]);
        Ok(())
    }

    #[test]
    fn latency_bumps_a_later_picture_without_waiting_for_the_reorder_limit() -> Result<()> {
        for increase in [0, 1] {
            let mut dpb = Dpb::new();
            let mut s = sps();
            s.max_reorder = [3; 7];
            s.max_latency = [increase; 7];
            push(&mut dpb, 19, &header(0, &[]), &s)?;
            push(&mut dpb, 1, &header(7, &[]), &s)?;
            push(&mut dpb, 1, &header(3, &[]), &s)?;
            push(&mut dpb, 1, &header(4, &[]), &s)?;
            let output = push(&mut dpb, 1, &header(5, &[]), &s)?;
            assert_eq!(
                pocs(&output),
                if increase == 0 {
                    vec![3]
                } else {
                    vec![3, 4, 5, 7]
                }
            );
        }
        Ok(())
    }

    #[test]
    fn highest_tid_controls_bumping_instead_of_the_current_tid() -> Result<()> {
        let mut dpb = Dpb::new();
        let mut s = sps();
        s.sub_layers = 1;
        if let Some(reorder) = s.max_reorder.first_mut() {
            *reorder = 0;
        }
        assert!(push(&mut dpb, 19, &header(0, &[]), &s)?.is_empty());
        assert!(push(&mut dpb, 1, &header(2, &[]), &s)?.is_empty());
        assert_eq!(pocs(&finish(&mut dpb)?), [0, 2]);
        Ok(())
    }

    #[test]
    fn later_sps_does_not_silently_change_highest_tid() -> Result<()> {
        let mut dpb = Dpb::new();
        let mut s = sps();
        if let Some(reorder) = s.max_reorder.get_mut(1) {
            *reorder = 0;
        }
        assert!(push(&mut dpb, 19, &header(0, &[]), &s)?.is_empty());
        s.sub_layers = 1;
        let output = push(&mut dpb, 19, &header(0, &[]), &s)?;
        assert_eq!(
            output.iter().map(|e| e.picture_index).collect::<Vec<_>>(),
            [0]
        );
        assert_eq!(finish(&mut dpb)?.first().map(|e| e.picture_index), Some(1));
        Ok(())
    }

    #[test]
    fn eos_preserves_selection_and_extracted_pictures_do_not_mutate_references_or_leading_state()
    -> Result<()> {
        let mut dpb = Dpb::new();
        let mut s = sps();
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        dpb.end_sequence();
        s.sub_layers = 1;
        push(&mut dpb, 21, &header(4, &[]), &s)?;
        let mut n = nal(1);
        n.temporal_id = 1;
        let skipped = dpb.picture(n, &header(6, &[(-1, true)]), &s, &Limits::default(), |_| {
            Err(Error::InvalidData("unexpected output"))
        })?;
        assert!(!skipped.output);
        push(&mut dpb, 7, &header(3, &[(1, true)]), &s)?;
        let output = finish(&mut dpb)?;
        assert_eq!(pocs(&output), [3, 4]);
        assert_eq!(
            output.iter().map(|e| e.picture_index).collect::<Vec<_>>(),
            [3, 1]
        );
        Ok(())
    }

    #[test]
    fn new_cvsg_selects_the_new_first_sps_highest_tid() -> Result<()> {
        let mut dpb = Dpb::new();
        let mut s = sps();
        if let Some(reorder) = s.max_reorder.get_mut(1) {
            *reorder = 0;
        }
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        let mut output = finish(&mut dpb)?;
        dpb.select_cvsg()?;
        s.sub_layers = 1;
        output.extend(push(&mut dpb, 19, &header(0, &[]), &s)?);
        assert_eq!(
            output.iter().map(|e| e.picture_index).collect::<Vec<_>>(),
            [0, 1]
        );
        let mut n = nal(1);
        n.temporal_id = 1;
        let mut output = Vec::new();
        let order = dpb.picture(n, &header(1, &[]), &s, &Limits::default(), |event| {
            output.push(event.event);
            Ok(())
        })?;
        assert!(order.output);
        assert_eq!(output.first().map(|e| e.picture_index), Some(2));
        Ok(())
    }

    #[test]
    fn cvsg_selection_rejects_buffered_state_without_dropping_output() -> Result<()> {
        let mut dpb = Dpb::new();
        let s = sps();
        dpb.select_cvsg()?;
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        push(&mut dpb, 1, &header(3, &[]), &s)?;
        assert_eq!(
            dpb.select_cvsg(),
            Err(Error::InvalidData("CVSG requires drained DPB"))
        );
        let output = finish(&mut dpb)?;
        assert_eq!(pocs(&output), [0, 3]);
        assert_eq!(
            output.iter().map(|e| e.picture_index).collect::<Vec<_>>(),
            [0, 1]
        );
        dpb.select_cvsg()?;
        push(&mut dpb, 21, &header(1, &[]), &s)?;
        let output = finish(&mut dpb)?;
        assert_eq!(pocs(&output), [1]);
        assert_eq!(output.first().map(|e| e.picture_index), Some(2));
        Ok(())
    }

    #[test]
    fn cvsg_start_callback_failure_and_finished_state_are_terminal() -> Result<()> {
        let mut dpb = Dpb::new();
        let s = sps();
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        assert_eq!(
            dpb.start_cvsg(nal(19), &header(0, &[]), |_| Err(Error::Limit("callback"))),
            Err(Error::Limit("callback"))
        );
        assert_eq!(
            dpb.finish(|_| Ok(())),
            Err(Error::InvalidData("DPB session ended"))
        );
        assert_eq!(
            dpb.start_cvsg(nal(19), &header(0, &[]), |_| Ok(())),
            Err(Error::InvalidData("DPB session ended"))
        );
        let mut finished = Dpb::new();
        finished.finish(|_| Ok(()))?;
        assert_eq!(
            finished.start_cvsg(nal(19), &header(0, &[]), |_| Ok(())),
            Err(Error::InvalidData("DPB session ended"))
        );
        Ok(())
    }

    #[test]
    fn cvsg_selection_cannot_reopen_a_failed_session() -> Result<()> {
        let mut dpb = Dpb::new();
        let s = sps();
        assert_eq!(
            push(&mut dpb, 1, &header(0, &[]), &s),
            Err(Error::InvalidData("missing initial IRAP"))
        );
        assert_eq!(
            dpb.select_cvsg(),
            Err(Error::InvalidData("DPB session ended"))
        );
        Ok(())
    }

    #[test]
    fn missing_selected_sps_sublayer_parameters_are_not_invented() -> Result<()> {
        let mut dpb = Dpb::new();
        let mut s = sps();
        s.sub_layers = 1;
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        s.sub_layers = 0;
        assert_eq!(
            push(&mut dpb, 19, &header(0, &[]), &s),
            Err(Error::InvalidData("HighestTid exceeds SPS sub-layers"))
        );
        Ok(())
    }

    #[test]
    fn extracted_pictures_still_consume_the_picture_budget() -> Result<()> {
        let mut dpb = Dpb::new();
        let mut s = sps();
        let limits = Limits {
            max_pictures: 2,
            ..Limits::default()
        };
        dpb.picture(nal(19), &header(0, &[]), &s, &limits, |_| Ok(()))?;
        s.sub_layers = 1;
        let mut n = nal(1);
        n.temporal_id = 1;
        assert!(
            !dpb.picture(n, &header(2, &[]), &s, &limits, |_| Ok(()))?
                .output
        );
        assert_eq!(
            dpb.picture(n, &header(3, &[]), &s, &limits, |_| Ok(())),
            Err(Error::Limit("max_pictures"))
        );
        Ok(())
    }

    #[test]
    fn output_preserves_each_pictures_geometry_across_sps_changes() -> Result<()> {
        let mut dpb = Dpb::new();
        let mut s = sps();
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        s.visible_width = 128;
        s.visible_height = 120;
        s.bit_depth = 10;
        let output = push(&mut dpb, 19, &header(0, &[]), &s)?;
        assert_eq!(
            output.first().map(|e| (e.width, e.height, e.bit_depth)),
            Some((60, 62, 8))
        );
        let output = finish(&mut dpb)?;
        assert_eq!(
            output.first().map(|e| (e.width, e.height, e.bit_depth)),
            Some((128, 120, 10))
        );
        Ok(())
    }

    #[test]
    fn resource_limit_and_callback_error_are_terminal() -> Result<()> {
        let mut dpb = Dpb::new();
        let mut s = sps();
        let limits = Limits {
            max_pictures: 0,
            ..Limits::default()
        };
        assert_eq!(
            dpb.picture(nal(19), &header(0, &[]), &s, &limits, |_| Ok(())),
            Err(Error::Limit("max_pictures"))
        );
        assert_eq!(
            finish(&mut dpb),
            Err(Error::InvalidData("DPB session ended"))
        );
        let mut dpb = Dpb::new();
        s.max_reorder = [0; 7];
        assert_eq!(
            dpb.picture(nal(19), &header(0, &[]), &s, &Limits::default(), |_| Err(
                Error::Limit("sink")
            )),
            Err(Error::Limit("sink"))
        );
        assert_eq!(
            finish(&mut dpb),
            Err(Error::InvalidData("DPB session ended"))
        );
        Ok(())
    }

    #[test]
    fn malformed_rps_and_unsupported_tools_have_distinct_errors() -> Result<()> {
        let s = sps();
        let mut dpb = Dpb::new();
        assert_eq!(
            push(&mut dpb, 22, &header(0, &[]), &s),
            Err(Error::Unsupported("reserved VCL NAL type"))
        );
        let mut dpb = Dpb::new();
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        assert_eq!(
            push(&mut dpb, 1, &header(2, &[(-2, true), (-2, false)]), &s),
            Err(Error::InvalidData("duplicate RPS reference"))
        );
        Ok(())
    }

    #[test]
    fn current_reference_respects_slnr_and_tsa_temporal_constraints() -> Result<()> {
        for (kind, first_kind, error) in [
            (1, 0, "current reference temporal_id"),
            (3, 1, "TSA reference temporal_id"),
        ] {
            let mut dpb = Dpb::new();
            let mut s = sps();
            s.sub_layers = 1;
            push(&mut dpb, 19, &header(0, &[]), &s)?;
            let tid = if kind == 3 { 1 } else { 0 };
            let mut n = nal(first_kind);
            n.temporal_id = tid;
            dpb.picture(n, &header(2, &[]), &s, &Limits::default(), |_| Ok(()))?;
            n.unit_type = kind;
            assert_eq!(
                dpb.picture(
                    n,
                    &header(3, &[(-1, true)]),
                    &s,
                    &Limits::default(),
                    |_| Ok(())
                ),
                Err(Error::InvalidData(error))
            );
        }
        Ok(())
    }

    #[test]
    fn pictures_after_stsa_cannot_reuse_earlier_same_tid_references() -> Result<()> {
        let mut dpb = Dpb::new();
        let mut s = sps();
        s.sub_layers = 1;
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        let mut n = nal(1);
        n.temporal_id = 1;
        dpb.picture(
            n,
            &header(2, &[(-2, false)]),
            &s,
            &Limits::default(),
            |_| Ok(()),
        )?;
        n.unit_type = 5;
        dpb.picture(
            n,
            &header(3, &[(-1, false), (-3, true)]),
            &s,
            &Limits::default(),
            |_| Ok(()),
        )?;
        n.unit_type = 1;
        assert_eq!(
            dpb.picture(
                n,
                &header(4, &[(-2, true)]),
                &s,
                &Limits::default(),
                |_| Ok(())
            ),
            Err(Error::InvalidData("reference precedes STSA"))
        );
        Ok(())
    }

    #[test]
    fn complete_layers_warn_on_a_temporal_nesting_nal_type() -> Result<()> {
        let mut dpb = Dpb::new();
        let mut s = sps();
        s.sub_layers = 1;
        s.temporal_nesting = true;
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        let mut n = nal(1);
        n.temporal_id = 1;
        let order = dpb.picture(n, &header(1, &[(-1, true)]), &s, &Limits::default(), |_| {
            Ok(())
        })?;
        assert_eq!(order.poc, 1);
        assert!(dpb.warnings.temporal_nesting_nal_type);
        n.temporal_id = 2;
        assert_eq!(
            dpb.picture(n, &header(2, &[]), &s, &Limits::default(), |_| Ok(())),
            Err(Error::InvalidData("DPB temporal_id"))
        );
        Ok(())
    }

    #[test]
    fn temporal_nesting_excludes_references_before_a_lower_tid_picture() -> Result<()> {
        let mut dpb = Dpb::new();
        let mut s = sps();
        s.sub_layers = 2;
        s.temporal_nesting = true;
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        let mut n = nal(3);
        n.temporal_id = 1;
        dpb.picture(n, &header(2, &[(-2, true)]), &s, &Limits::default(), |_| {
            Ok(())
        })?;
        push(&mut dpb, 1, &header(1, &[(1, false)]), &s)?;
        n.temporal_id = 2;
        assert_eq!(
            dpb.picture(
                n,
                &header(3, &[(-1, true)]),
                &s,
                &Limits::default(),
                |_| Ok(())
            ),
            Err(Error::InvalidData("temporal nesting reference"))
        );
        Ok(())
    }

    #[test]
    fn leading_picture_type_and_decoding_order_are_checked() -> Result<()> {
        let s = sps();
        let mut dpb = Dpb::new();
        push(&mut dpb, 20, &header(0, &[]), &s)?;
        assert_eq!(
            push(&mut dpb, 7, &header(15, &[]), &s),
            Err(Error::InvalidData("IRAP leading picture type"))
        );
        let mut dpb = Dpb::new();
        push(&mut dpb, 21, &header(4, &[]), &s)?;
        push(&mut dpb, 1, &header(5, &[]), &s)?;
        assert_eq!(
            push(&mut dpb, 9, &header(3, &[]), &s),
            Err(Error::InvalidData("leading picture order"))
        );
        Ok(())
    }

    #[test]
    fn sixteen_entry_limit_never_grows_and_irap_flush_is_callback_bounded() -> Result<()> {
        let mut dpb = Dpb::new();
        let mut s = sps();
        s.poc_bits = 8;
        s.max_dpb = [16; 7];
        s.max_reorder = [15; 7];
        push(&mut dpb, 19, &header(0, &[]), &s)?;
        for poc in 1..15 {
            assert!(push(&mut dpb, 1, &header(poc, &[]), &s)?.is_empty());
        }
        assert_eq!(dpb.fullness(), 15);
        s.max_reorder = [0; 7];
        let output = push(&mut dpb, 19, &header(0, &[]), &s)?;
        assert_eq!(output.len(), 16);
        assert_eq!(output.last().map(|e| e.picture_index), Some(15));
        assert_eq!(dpb.fullness(), 1);
        Ok(())
    }
}
