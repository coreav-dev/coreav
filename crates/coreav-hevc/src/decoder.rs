use crate::{
    Error, InterStats, IntraPlane, IntraStats, Limits, PictureInfo, Result, SeiInfo, Summary,
    dpb::{Dpb, Emission},
    parser::{PictureObserver, SliceInput, StreamCore},
    reconstruction::{Frame, Memory},
};

/// Independent admission, live-memory and entropy-work limits for a complete stream.
#[derive(Clone, Copy, Debug)]
pub struct DecodeLimits {
    /// Header admission, including the stream-wide coded-picture limit.
    pub headers: Limits,
    /// Maximum simultaneous codec state, pictures and transient scratch bytes.
    pub max_memory_bytes: usize,
    /// Maximum regular, bypass and termination bins over the complete stream.
    pub max_stream_bins: u64,
    /// Maximum bins in any one reconstructed picture.
    pub max_picture_bins: u64,
    /// Maximum bytes of a packed visible output picture, checked before sample allocation.
    pub max_frame_bytes: u64,
    /// Optional absolute decoder-session deadline, including filtering and output work.
    pub deadline: Option<std::time::Instant>,
}

impl Default for DecodeLimits {
    fn default() -> Self {
        Self {
            headers: Limits::default(),
            max_memory_bytes: 512 * 1024 * 1024,
            max_stream_bins: 1 << 40,
            max_picture_bins: 268_435_456,
            max_frame_bytes: u64::MAX,
            deadline: None,
        }
    }
}

/// One complete filtered picture, borrowed in presentation order.
#[derive(Clone, Copy, Debug)]
pub struct DecodedPicture<'a> {
    /// Original zero-based coded-picture index, including non-output pictures.
    pub index: u64,
    /// Full signed picture order count from the shared DPB.
    pub poc: i32,
    /// Caller timestamp supplied with the first slice, in the caller's own clock.
    pub timestamp: Option<i64>,
    /// Opaque caller identity supplied with the picture's first slice.
    pub origin: Option<u64>,
    /// Activated geometry, crop and original display declarations.
    pub info: PictureInfo,
    /// Complete coded Y, Cb and Cr planes; cropping is not applied to these views.
    pub planes: [IntraPlane<'a>; 3],
    /// Reconstruction and serial-tool work through the picture's finalization.
    pub stats: IntraStats,
    /// Effective inter prediction and boundary-strength work.
    pub inter: InterStats,
}

/// A callback-scoped reservation in the decoder's combined live-memory account.
pub struct OutputMemory<'a> {
    codec_bytes: usize,
    reserved: &'a mut usize,
    limit: usize,
}

impl OutputMemory<'_> {
    /// Charges external capacity before allocating output or queue storage.
    pub fn reserve(&mut self, bytes: usize) -> Result<()> {
        let reserved = self
            .reserved
            .checked_add(bytes)
            .ok_or(Error::Limit("reconstruction_memory"))?;
        let mut memory = Memory::new(self.codec_bytes, self.limit)?;
        memory.charge(reserved)?;
        *self.reserved = reserved;
        Ok(())
    }

    /// Releases external capacity after it is freed or transferred to the caller.
    pub fn release(&mut self, bytes: usize) -> Result<()> {
        *self.reserved = self
            .reserved
            .checked_sub(bytes)
            .ok_or(Error::InvalidData("output_reservation_underflow"))?;
        Ok(())
    }
}

/// Successful complete-stream accounting, including pictures not selected for output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DecodeSummary {
    /// Header syntax and presentation-order output counts.
    pub headers: Summary,
    /// Pictures reconstructed and finalized, including non-output reference pictures.
    pub decoded_pictures: u64,
    /// Temporally extracted or suppressed RASL pictures with no reconstructed samples.
    pub skipped_pictures: u64,
    /// Defined hash messages attached to pictures outside the target decoding set.
    /// These messages have no reconstructed samples and receive no checked-hash credit.
    pub skipped_picture_hashes: u64,
    /// Reserved hash messages attached to pictures outside the target decoding set.
    pub skipped_reserved_hashes: u64,
    /// Work and checked hashes over all reconstructed pictures, not only outputs.
    pub reconstruction: IntraStats,
    /// Effective inter work over all reconstructed pictures.
    pub inter: InterStats,
    /// Peak accounted simultaneous codec bytes.
    pub memory_bytes: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Active,
    Finished,
    Failed,
}

struct Pictures {
    limits: DecodeLimits,
    frame: Option<Frame>,
    timestamp: Option<i64>,
    origin: Option<u64>,
    external_memory: usize,
    skipped: Option<u64>,
    stream_remaining: u64,
    picture_remaining: u64,
    summary: DecodeSummary,
}

struct Session {
    core: StreamCore,
    pictures: Pictures,
}

/// Sans-IO Main/Main10 scalar decoder with one DPB and borrowed output callbacks.
pub struct PictureDecoder {
    limits: DecodeLimits,
    session: Vec<Session>,
    state: State,
}

impl PictureDecoder {
    /// Creates an opt-in decoder without allocating picture samples.
    pub fn new(limits: DecodeLimits) -> Self {
        Self {
            limits,
            session: Vec::new(),
            state: State::Active,
        }
    }

    fn fixed_memory(&self) -> Result<usize> {
        core::mem::size_of::<Self>()
            .checked_add(
                core::mem::size_of::<Session>()
                    .checked_mul(self.session.capacity().max(1) + 1)
                    .ok_or(Error::Limit("reconstruction_memory"))?,
            )
            .and_then(|n| n.checked_add(coreav_kernels_hevc::SCRATCH_BYTES))
            .and_then(|n| n.checked_add(4 * core::mem::size_of::<crate::params::Sps>()))
            .and_then(|n| n.checked_add(2 * core::mem::size_of::<crate::params::Pps>()))
            .ok_or(Error::Limit("reconstruction_memory"))
    }

    fn fail(&mut self) {
        self.state = State::Failed;
        self.session = Vec::new();
    }

    fn initialize(&mut self) -> Result<()> {
        if !self.session.is_empty() {
            return Ok(());
        }
        Memory::new(self.fixed_memory()?, self.limits.max_memory_bytes)?;
        self.session
            .try_reserve_exact(1)
            .map_err(|_| Error::Limit("decoder_state_allocation"))?;
        Memory::new(self.fixed_memory()?, self.limits.max_memory_bytes)?;
        let limits = self.limits;
        self.session.push(Session {
            core: StreamCore::new_reconstruction(limits.headers),
            pictures: Pictures {
                limits,
                frame: None,
                timestamp: None,
                origin: None,
                external_memory: 0,
                skipped: None,
                stream_remaining: limits.max_stream_bins,
                picture_remaining: limits.max_picture_bins,
                summary: DecodeSummary::default(),
            },
        });
        Ok(())
    }

    /// Feeds one original complete NAL and emits completed pictures in presentation order.
    ///
    /// Supply a timestamp on the first slice of a picture. Any supplied later-slice
    /// timestamp must agree. Its time base and container edit mapping remain the caller's.
    /// An error closes the session and releases its pictures; no concealment is attempted.
    pub fn read_nal(
        &mut self,
        bytes: &[u8],
        timestamp: Option<i64>,
        mut emit: impl FnMut(DecodedPicture<'_>) -> Result<()>,
    ) -> Result<()> {
        self.read(bytes, timestamp, None, |picture, _| emit(picture))
    }

    /// Feeds an original NAL with an opaque picture identity, independently of PTS.
    /// Later slices with an identity must match the first slice's identity.
    pub fn read_nal_with_origin(
        &mut self,
        bytes: &[u8],
        timestamp: Option<i64>,
        origin: u64,
        emit: impl FnMut(DecodedPicture<'_>, &mut OutputMemory<'_>) -> Result<()>,
    ) -> Result<()> {
        self.read(bytes, timestamp, Some(origin), emit)
    }

    fn read(
        &mut self,
        bytes: &[u8],
        timestamp: Option<i64>,
        origin: Option<u64>,
        mut emit: impl FnMut(DecodedPicture<'_>, &mut OutputMemory<'_>) -> Result<()>,
    ) -> Result<()> {
        if self.state != State::Active {
            self.fail();
            return Err(Error::InvalidData("picture_decoder_closed"));
        }
        let result = (|| {
            self.initialize()?;
            let fixed = self.fixed_memory()?;
            let session = self
                .session
                .first_mut()
                .ok_or(Error::InvalidData("decoder_state"))?;
            let frame_bytes = session
                .pictures
                .frame
                .as_ref()
                .map_or(Ok(0), Frame::heap_bytes)?;
            let live = fixed
                .checked_add(session.core.heap_bytes()?)
                .and_then(|n| n.checked_add(frame_bytes))
                .and_then(|n| n.checked_add(session.pictures.external_memory))
                .ok_or(Error::Limit("reconstruction_memory"))?;
            let mut memory = Memory::new(live, session.pictures.limits.max_memory_bytes)?;
            memory.charge(bytes.len().saturating_sub(2))?;
            session.pictures.summary.memory_bytes =
                session.pictures.summary.memory_bytes.max(memory.used);
            let available = session
                .pictures
                .limits
                .max_memory_bytes
                .checked_sub(live)
                .ok_or(Error::Limit("reconstruction_memory"))?;
            session.core.read_reconstructed(
                bytes,
                available,
                &mut Observer {
                    pictures: &mut session.pictures,
                    fixed,
                    incoming_timestamp: timestamp,
                    incoming_origin: origin,
                    emit: &mut emit,
                },
            )?;
            Ok(())
        })();
        if result.is_err() {
            self.fail();
        }
        result
    }

    /// Finalizes the last picture and drains output; success requires this call.
    /// Earlier callbacks remain valid if a later NAL, hash or callback fails.
    pub fn finish(
        &mut self,
        mut emit: impl FnMut(DecodedPicture<'_>) -> Result<()>,
    ) -> Result<DecodeSummary> {
        self.finish_with_output(|picture, _| emit(picture))
    }

    /// Drains pictures with callback-scoped output reservations in the same account.
    pub fn finish_with_output(
        &mut self,
        mut emit: impl FnMut(DecodedPicture<'_>, &mut OutputMemory<'_>) -> Result<()>,
    ) -> Result<DecodeSummary> {
        if self.state == State::Failed {
            return Err(Error::InvalidData("picture_decoder_failed"));
        }
        let result = (|| {
            let fixed = self.fixed_memory()?;
            let session = self
                .session
                .first_mut()
                .ok_or(Error::InvalidData("no_coded_pictures"))?;
            let headers = session.core.finish_reconstructed(&mut Observer {
                pictures: &mut session.pictures,
                fixed,
                incoming_timestamp: None,
                incoming_origin: None,
                emit: &mut emit,
            })?;
            session.pictures.summary.headers = headers;
            Ok(session.pictures.summary)
        })();
        if result.is_err() {
            self.fail();
        } else {
            self.state = State::Finished;
        }
        result
    }

    /// Sets retained external capacity between operations, charging live codec state too.
    /// Reservation failure is terminal, like a decode or output-callback failure.
    pub fn set_external_memory(&mut self, bytes: usize) -> Result<()> {
        if self.state == State::Failed {
            return Err(Error::InvalidData("picture_decoder_failed"));
        }
        let result = (|| {
            self.initialize()?;
            let fixed = self.fixed_memory()?;
            let session = self
                .session
                .first_mut()
                .ok_or(Error::InvalidData("decoder_state"))?;
            let frame = session
                .pictures
                .frame
                .as_ref()
                .map_or(Ok(0), Frame::heap_bytes)?;
            let live = fixed
                .checked_add(session.core.heap_bytes()?)
                .and_then(|n| n.checked_add(frame))
                .ok_or(Error::Limit("reconstruction_memory"))?;
            let mut memory = Memory::new(live, self.limits.max_memory_bytes)?;
            memory.charge(bytes)?;
            session.pictures.external_memory = bytes;
            session.pictures.summary.memory_bytes =
                session.pictures.summary.memory_bytes.max(memory.used);
            Ok(())
        })();
        if result.is_err() {
            self.fail();
        }
        result
    }
}

struct Observer<'a, F> {
    pictures: &'a mut Pictures,
    fixed: usize,
    incoming_timestamp: Option<i64>,
    incoming_origin: Option<u64>,
    emit: &'a mut F,
}

impl<F: FnMut(DecodedPicture<'_>, &mut OutputMemory<'_>) -> Result<()>> PictureObserver
    for Observer<'_, F>
{
    fn boundary(&mut self, dpb: &mut Dpb, scratch_bytes: usize) -> Result<()> {
        self.pictures.skipped = None;
        let Some(mut frame) = self.pictures.frame.take() else {
            return Ok(());
        };
        let fixed = self
            .fixed
            .checked_add(dpb.heap_bytes()?)
            .and_then(|n| n.checked_add(scratch_bytes))
            .and_then(|n| n.checked_add(self.pictures.external_memory))
            .ok_or(Error::Limit("reconstruction_memory"))?;
        let peak = frame.finalize(fixed, self.pictures.limits.max_memory_bytes)?;
        self.pictures.summary.memory_bytes = self.pictures.summary.memory_bytes.max(peak);
        frame.stats.bins = self.pictures.limits.max_picture_bins - self.pictures.picture_remaining;
        frame.stats.memory_bytes = self.pictures.summary.memory_bytes;
        self.pictures
            .summary
            .add(frame.stats, frame.inter_stats())?;
        dpb.attach(frame.retain(self.pictures.timestamp, self.pictures.origin)?)
    }

    fn slice(&mut self, input: SliceInput<'_>, dpb: &Dpb) -> Result<()> {
        if input.header.first {
            if self.incoming_origin.is_some() && self.incoming_origin == self.pictures.origin {
                return Err(Error::InvalidData("multiple_pictures_in_access_unit"));
            }
            self.pictures.timestamp = self.incoming_timestamp;
            self.pictures.origin = self.incoming_origin;
        } else {
            if self.incoming_timestamp.is_some()
                && self.incoming_timestamp != self.pictures.timestamp
            {
                return Err(Error::InvalidData("picture_timestamp_changed"));
            }
            if self.incoming_origin.is_some() && self.incoming_origin != self.pictures.origin {
                return Err(Error::InvalidData("picture_origin_changed"));
            }
        }
        if !dpb.reconstructs(input.index) {
            if self.pictures.skipped != Some(input.index) {
                self.pictures.summary.skipped_pictures = self
                    .pictures
                    .summary
                    .skipped_pictures
                    .checked_add(1)
                    .ok_or(Error::Limit("decode_work_count"))?;
                self.pictures.skipped = Some(input.index);
            }
            return Ok(());
        }
        let fixed = self
            .fixed
            .checked_add(dpb.heap_bytes()?)
            .and_then(|n| n.checked_add(self.pictures.external_memory))
            .ok_or(Error::Limit("reconstruction_memory"))?;
        if self.pictures.frame.is_none() {
            let bytes = (u64::from(input.picture.visible_width)
                * u64::from(input.picture.visible_height))
            .checked_mul(3)
            .and_then(|n| n.checked_mul(if input.picture.bit_depth == 10 { 2 } else { 1 }))
            .and_then(|n| n.checked_div(2))
            .ok_or(Error::Limit("output_frame_bytes"))?;
            if bytes > self.pictures.limits.max_frame_bytes {
                return Err(Error::Limit("output_frame_bytes"));
            }
            let identity = dpb.current_identity(input.index)?;
            let mut frame = Frame::new_full(
                &input,
                identity.poc,
                dpb.current_references(),
                fixed,
                self.pictures.limits.max_memory_bytes,
            )?;
            frame.deadline = self.pictures.limits.deadline;
            self.pictures.frame = Some(frame);
            self.pictures.picture_remaining = self.pictures.limits.max_picture_bins;
        }
        let frame = self
            .pictures
            .frame
            .as_mut()
            .ok_or(Error::InvalidData("missing_working_picture"))?;
        let allowance = self
            .pictures
            .stream_remaining
            .min(self.pictures.picture_remaining);
        let mut budget = allowance;
        let result = frame.decode_full(
            input,
            dpb,
            &mut budget,
            fixed,
            self.pictures.limits.max_memory_bytes,
        );
        let consumed = allowance - budget;
        self.pictures.stream_remaining -= consumed;
        self.pictures.picture_remaining -= consumed;
        match result {
            Ok(peak) => {
                self.pictures.summary.memory_bytes = self.pictures.summary.memory_bytes.max(peak);
                Ok(())
            }
            Err(Error::Limit("cabac_bin_budget")) if self.pictures.stream_remaining == 0 => {
                Err(Error::Limit("stream_cabac_bin_budget"))
            }
            Err(error) => Err(error),
        }
    }

    fn sei(&mut self, info: SeiInfo, dpb: &Dpb, scratch_bytes: usize) -> Result<()> {
        if let Some(frame) = self.pictures.frame.as_mut() {
            let fixed = self
                .fixed
                .checked_add(dpb.heap_bytes()?)
                .and_then(|n| n.checked_add(scratch_bytes))
                .and_then(|n| n.checked_add(self.pictures.external_memory))
                .ok_or(Error::Limit("reconstruction_memory"))?;
            let peak = frame.hash(info, fixed, self.pictures.limits.max_memory_bytes)?;
            self.pictures.summary.memory_bytes = self.pictures.summary.memory_bytes.max(peak);
        } else if self.pictures.skipped.is_some() {
            self.pictures.summary.skipped_picture_hashes = self
                .pictures
                .summary
                .skipped_picture_hashes
                .checked_add(u64::from(info.picture_hash.is_some()))
                .ok_or(Error::Limit("decode_work_count"))?;
            self.pictures.summary.skipped_reserved_hashes = self
                .pictures
                .summary
                .skipped_reserved_hashes
                .checked_add(u64::from(info.reserved_hashes))
                .ok_or(Error::Limit("decode_work_count"))?;
        } else if info.picture_hash.is_some() || info.reserved_hashes != 0 {
            return Err(Error::InvalidData("picture_hash_without_samples"));
        }
        Ok(())
    }

    fn output(&mut self, emission: Emission<'_>, scratch_bytes: usize) -> Result<()> {
        let samples = emission
            .picture
            .ok_or(Error::InvalidData("missing_output_samples"))?;
        let picture = samples.view();
        let frame = self
            .pictures
            .frame
            .as_ref()
            .map_or(Ok(0), Frame::heap_bytes)?;
        let codec_bytes = self
            .fixed
            .checked_add(emission.retained_bytes)
            .and_then(|n| n.checked_add(frame))
            .and_then(|n| n.checked_add(scratch_bytes))
            .ok_or(Error::Limit("reconstruction_memory"))?;
        let mut memory = OutputMemory {
            codec_bytes,
            reserved: &mut self.pictures.external_memory,
            limit: self.pictures.limits.max_memory_bytes,
        };
        (self.emit)(
            DecodedPicture {
                index: picture.index,
                poc: emission.event.poc,
                timestamp: samples.timestamp(),
                origin: samples.origin(),
                info: picture.info,
                planes: picture.planes,
                stats: picture.stats,
                inter: samples.inter_stats(),
            },
            &mut memory,
        )?;
        let mut used = Memory::new(codec_bytes, self.pictures.limits.max_memory_bytes)?;
        used.charge(self.pictures.external_memory)?;
        self.pictures.summary.memory_bytes = self.pictures.summary.memory_bytes.max(used.used);
        Ok(())
    }
}

impl DecodeSummary {
    fn add(&mut self, reconstruction: IntraStats, inter: InterStats) -> Result<()> {
        self.decoded_pictures = self
            .decoded_pictures
            .checked_add(1)
            .ok_or(Error::Limit("decode_work_count"))?;
        macro_rules! sum {
            ($target:ident, $source:ident, $($field:ident),+ $(,)?) => {
                $(self.$target.$field = self.$target.$field.checked_add($source.$field)
                    .ok_or(Error::Limit("decode_work_count"))?;)+
            };
        }
        sum!(
            reconstruction,
            reconstruction,
            ctus,
            coding_units,
            transform_units,
            nonzero_transform_units,
            transform_skips,
            transquant_bypass_units,
            pcm_units,
            dependent_segments,
            tile_restarts,
            wpp_restarts,
            qp_deltas,
            nonzero_qp_deltas,
            scaling_list_blocks,
            sign_hidden_groups,
            dst_blocks,
            bins,
            picture_hashes,
            reserved_hashes,
            sao_merges
        );
        sum!(
            inter,
            inter,
            skip_units,
            prediction_units,
            p_units,
            b_units,
            bi_units,
            merge_units,
            amvp_lists,
            temporal_units,
            weighted_units,
            fractional_luma_units,
            fractional_chroma_units,
            negative_motion_units,
            long_term_units,
            list_modification_units
        );
        for (target, source) in [
            (
                &mut self.reconstruction.deblock_edges,
                reconstruction.deblock_edges,
            ),
            (
                &mut self.reconstruction.deblock_changed_samples,
                reconstruction.deblock_changed_samples,
            ),
            (
                &mut self.reconstruction.sao_blocks,
                reconstruction.sao_blocks,
            ),
            (
                &mut self.reconstruction.sao_changed_samples,
                reconstruction.sao_changed_samples,
            ),
            (
                &mut self.inter.weighted_changed_samples,
                inter.weighted_changed_samples,
            ),
            (&mut self.inter.boundary_strengths, inter.boundary_strengths),
        ] {
            add_counts(target, source)?;
        }
        add_counts(
            &mut self.reconstruction.luma_modes,
            reconstruction.luma_modes,
        )?;
        add_counts(
            &mut self.reconstruction.chroma_modes,
            reconstruction.chroma_modes,
        )?;
        add_counts(
            &mut self.reconstruction.dct_blocks,
            reconstruction.dct_blocks,
        )?;
        add_counts(&mut self.inter.partitions, inter.partitions)?;
        for (target, source) in self
            .inter
            .boundary_changed_samples
            .iter_mut()
            .zip(inter.boundary_changed_samples)
        {
            add_counts(target, source)?;
        }
        self.reconstruction.memory_bytes = self
            .reconstruction
            .memory_bytes
            .max(reconstruction.memory_bytes);
        Ok(())
    }
}

fn add_counts<const N: usize>(target: &mut [u64; N], source: [u64; N]) -> Result<()> {
    for (target, source) in target.iter_mut().zip(source) {
        *target = target
            .checked_add(source)
            .ok_or(Error::Limit("decode_work_count"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skipped_hash_messages_never_receive_checked_sample_credit() -> Result<()> {
        let mut decoder = PictureDecoder::new(DecodeLimits::default());
        decoder.initialize()?;
        let fixed = decoder.fixed_memory()?;
        let session = decoder
            .session
            .first_mut()
            .ok_or(Error::InvalidData("test_session"))?;
        session.pictures.skipped = Some(7);
        let mut emit = |_: DecodedPicture<'_>, _: &mut OutputMemory<'_>| {
            Err(Error::InvalidData("unexpected_output"))
        };
        let mut observer = Observer {
            pictures: &mut session.pictures,
            fixed,
            incoming_timestamp: None,
            incoming_origin: None,
            emit: &mut emit,
        };
        let mut dpb = Dpb::new_reconstruction();
        let defined = SeiInfo {
            messages: 1,
            picture_hash: Some(crate::PictureHash::Crc([[0; 2]; 3])),
            reserved_hashes: 0,
        };
        observer.sei(defined, &dpb, 0)?;
        observer.sei(
            SeiInfo {
                messages: 1,
                reserved_hashes: 1,
                ..SeiInfo::default()
            },
            &dpb,
            0,
        )?;
        assert_eq!(observer.pictures.summary.skipped_picture_hashes, 1);
        assert_eq!(observer.pictures.summary.skipped_reserved_hashes, 1);
        assert_eq!(observer.pictures.summary.reconstruction.picture_hashes, 0);
        assert_eq!(observer.pictures.summary.reconstruction.reserved_hashes, 0);
        observer.boundary(&mut dpb, 0)?;
        assert_eq!(
            observer.sei(defined, &dpb, 0),
            Err(Error::InvalidData("picture_hash_without_samples"))
        );
        observer.pictures.skipped = Some(8);
        observer.pictures.summary.skipped_picture_hashes = u64::MAX;
        assert_eq!(
            observer.sei(defined, &dpb, 0),
            Err(Error::Limit("decode_work_count"))
        );
        Ok(())
    }

    #[test]
    fn construction_keeps_parameter_slots_off_the_callers_stack() {
        assert!(core::mem::size_of::<super::PictureDecoder>() <= 128);
        let decoder = super::PictureDecoder::new(super::DecodeLimits::default());
        assert!(decoder.session.is_empty());
        assert_eq!(decoder.session.capacity(), 0);
    }
}
