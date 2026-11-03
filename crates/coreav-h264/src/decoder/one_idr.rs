use super::*;
use crate::error::OneIdrError;

const MEMORY_CEILING: u64 = 32 << 20;
const PACKET_CEILING: usize = 4 << 20;

/// Validated geometry of the SPS activated by the admitted IDR's PPS.
/// Dimensions and rectangle offsets are in luma samples, not container units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OneIdrGeometry {
    /// Full macroblock-padded width.
    pub coded_width: u32,
    /// Full macroblock-padded height.
    pub coded_height: u32,
    /// Left edge of the visible rectangle in the coded picture.
    pub visible_left: u32,
    /// Top edge of the visible rectangle in the coded picture.
    pub visible_top: u32,
    /// Width after applying SPS cropping.
    pub visible_width: u32,
    /// Height after applying SPS cropping.
    pub visible_height: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum OneIdrState {
    Fresh,
    Decoding,
    Finished,
    Failed,
}

impl<K: Kernels> Decoder<K> {
    /// Computes the one-picture bound from a complete SPS NAL, before
    /// allocating picture buffers. Includes one caller-held packed output,
    /// RBSP scratch, parameter sets and transient allocation overlap.
    /// Refuses unsupported SPS syntax, caller limits and bounds above 32 MiB.
    /// The caller's borrowed input packet is outside this codec-side bound.
    pub fn one_idr_memory_needed(sps_nal: &[u8], limits: &Limits) -> Result<u64> {
        if NalHeader::parse(sps_nal)?.kind != NalKind::Sps {
            return Err(Error::Invalid("an SPS NAL is required"));
        }
        if sps_nal.len() > limits.max_nal_bytes.min(PACKET_CEILING) {
            return Err(Error::Limit(LimitKind::NalBytes));
        }
        let mut buffer = Vec::new();
        let payload = sps_nal.get(1..).unwrap_or_default();
        bounded_rbsp(
            payload.get(..MAX_SPS_BYTES * 3 / 2).unwrap_or(payload),
            &mut buffer,
        )?;
        buffer.truncate(MAX_SPS_BYTES);
        let sps = parse_sps(&buffer)?;
        one_picture_bound(&sps, limits)
    }

    /// Decodes one complete supplied access unit. `length_size` selects
    /// AVC length prefixes; `None` selects Annex B. Requires `Mode::OneIdr`.
    /// All slice headers are admitted before `start_picture`; entropy
    /// decoding then requires exact macroblock coverage before any output.
    /// Failure poisons this request and cannot produce a concealed picture.
    pub fn decode_one_idr(&mut self, data: &[u8], length_size: Option<usize>) -> Result<()> {
        self.decode_one_idr_with(data, length_size, |_| Ok(()))
    }

    /// Runs `admit` after all parameter-set and slice-header checks and
    /// before any picture allocation. Geometry comes from the activated
    /// SPS/PPS, never container metadata. A caller's refusal is returned
    /// unchanged and poisons the request. Allocations inside `admit` belong
    /// to the caller, outside the codec's allocation bound.
    pub fn decode_one_idr_with(
        &mut self,
        data: &[u8],
        length_size: Option<usize>,
        admit: impl FnOnce(OneIdrGeometry) -> Result<()>,
    ) -> Result<()> {
        if self.mode != Mode::OneIdr {
            return Err(Error::OneIdr(OneIdrError::AccessUnitRequired));
        }
        if self.started || self.one_idr == OneIdrState::Finished {
            self.fail_one_idr();
            return Err(Error::OneIdr(OneIdrError::SecondPicture));
        }
        if self.one_idr != OneIdrState::Fresh {
            return Err(Error::OneIdr(OneIdrError::Failed));
        }
        let result = if data.len() > PACKET_CEILING {
            Err(Error::Limit(LimitKind::NalBytes))
        } else {
            match length_size {
                Some(size) => {
                    self.admit_one_idr(crate::nal::strict_length_prefixed(data, size), admit)
                }
                None => strict_annex_b(data)
                    .and_then(|()| self.admit_one_idr(crate::annex_b(data).map(Ok), admit)),
            }
        };
        if result.is_err() {
            self.fail_one_idr();
        }
        result
    }

    fn admit_one_idr<'a>(
        &mut self,
        units: impl Iterator<Item = Result<&'a [u8]>> + Clone,
        admit: impl FnOnce(OneIdrGeometry) -> Result<()>,
    ) -> Result<()> {
        let mut first: Option<(Boundary, Arc<Sps>, Arc<Pps>)> = None;
        let mut last = None;
        let mut slices = 0usize;
        let mut ended = false;
        for unit in units.clone() {
            let unit = unit?;
            let header = NalHeader::parse(unit)?;
            if !header.is_slice() {
                if first.is_some()
                    && matches!(
                        header.kind,
                        NalKind::AccessUnitDelimiter
                            | NalKind::Sei
                            | NalKind::EndOfSequence
                            | NalKind::EndOfStream
                    )
                {
                    ended = true;
                }
                self.one_idr_parameter_set(unit)?;
                if let Some((boundary, sps, pps)) = &first {
                    let (active_sps, active_pps) =
                        self.resolve_parameter_sets(boundary.pps_id as usize)?;
                    if **sps != *active_sps || **pps != *active_pps {
                        return Err(Error::OneIdr(OneIdrError::ConfigurationConflict));
                    }
                }
                continue;
            }
            if ended {
                return Err(Error::OneIdr(OneIdrError::SecondPicture));
            }
            if header.kind != NalKind::IdrSlice || header.nal_ref_idc == 0 {
                return Err(Error::OneIdr(OneIdrError::ExpectedIdr));
            }
            if unit.len() > self.limits.max_nal_bytes.min(PACKET_CEILING) {
                return Err(Error::Limit(LimitKind::NalBytes));
            }
            let mut buffer = std::mem::take(&mut self.rbsp);
            let result = (|| {
                bounded_rbsp(unit.get(1..).unwrap_or_default(), &mut buffer)?;
                let (sps, pps) = self.resolve_parameter_sets(peek_pps_id(&buffer)? as usize)?;
                check_scope(&sps, &pps)?;
                one_picture_bound(&sps, &self.limits)?;
                let hdr = parse_slice_header(
                    &mut BitReader::new(&buffer),
                    5,
                    header.nal_ref_idc,
                    &sps,
                    &pps,
                )?;
                if hdr.frame_num != 0 {
                    return Err(Error::OneIdr(OneIdrError::FrameNumGap));
                }
                if hdr.slice_type != SliceType::I || hdr.redundant_pic_cnt != 0 || hdr.field_pic {
                    return Err(Error::OneIdr(OneIdrError::PredictedSlice));
                }
                if let Some((boundary, old_sps, old_pps)) = &first {
                    if **old_sps != *sps || **old_pps != *pps || hdr.pps_id != boundary.pps_id {
                        return Err(Error::OneIdr(OneIdrError::ConfigurationConflict));
                    }
                    if boundary.starts_new_picture(&hdr, &sps)
                        || last.is_some_and(|mb| hdr.first_mb <= mb)
                    {
                        return Err(Error::OneIdr(OneIdrError::SecondPicture));
                    }
                } else {
                    if hdr.first_mb != 0 {
                        return Err(Error::OneIdr(OneIdrError::IncompletePicture));
                    }
                    first = Some((Boundary::of(&hdr), sps, pps));
                }
                last = Some(hdr.first_mb);
                Ok(())
            })();
            self.rbsp = buffer;
            result?;
            slices += 1;
            let mbs = first.as_ref().map_or(0, |(_, sps, _)| {
                sps.width_mbs as usize * sps.frame_height_mbs() as usize
            });
            if slices > self.limits.max_slices_per_picture.min(mbs) {
                return Err(Error::Limit(LimitKind::SlicesPerPicture));
            }
        }
        let (_, sps, _) = first.ok_or(Error::OneIdr(OneIdrError::ExpectedIdr))?;
        let coded_width = sps
            .width_mbs
            .checked_mul(16)
            .ok_or(Error::Limit(LimitKind::PictureSize))?;
        let coded_height = sps
            .frame_height_mbs()
            .checked_mul(16)
            .ok_or(Error::Limit(LimitKind::PictureSize))?;
        let [left, right, top, bottom] = sps.crop;
        let visible_width = coded_width
            .checked_sub(left)
            .and_then(|n| n.checked_sub(right))
            .filter(|&n| n > 0)
            .ok_or(Error::Invalid("frame cropping outside the picture"))?;
        let visible_height = coded_height
            .checked_sub(top)
            .and_then(|n| n.checked_sub(bottom))
            .filter(|&n| n > 0)
            .ok_or(Error::Invalid("frame cropping outside the picture"))?;
        admit(OneIdrGeometry {
            coded_width,
            coded_height,
            visible_left: left,
            visible_top: top,
            visible_width,
            visible_height,
        })?;
        self.one_idr = OneIdrState::Decoding;
        self.threads = 1;
        self.band_mbs = None;
        for unit in units {
            let unit = unit?;
            if NalHeader::parse(unit)?.is_slice() {
                let mut buffer = std::mem::take(&mut self.rbsp);
                let result = (|| {
                    bounded_rbsp(unit.get(1..).unwrap_or_default(), &mut buffer)?;
                    self.decode_slice(&mut buffer, true, NalHeader::parse(unit)?.nal_ref_idc)
                })();
                self.rbsp = buffer;
                result?;
            }
        }
        self.finish_picture()?;
        self.dpb.flush();
        self.release_output();
        self.one_idr = OneIdrState::Finished;
        self.end_of_access_unit();
        Ok(())
    }

    pub(super) fn one_idr_parameter_set(&mut self, nal: &[u8]) -> Result<()> {
        if self.one_idr != OneIdrState::Fresh {
            return Err(Error::OneIdr(if self.one_idr == OneIdrState::Failed {
                OneIdrError::Failed
            } else {
                OneIdrError::ConfigurationConflict
            }));
        }
        if nal.len() > self.limits.max_nal_bytes.min(PACKET_CEILING) {
            return Err(Error::Limit(LimitKind::NalBytes));
        }
        match NalHeader::parse(nal)?.kind {
            NalKind::Sps => {
                let payload = nal.get(1..).unwrap_or_default();
                let mut buffer = Vec::new();
                bounded_rbsp(
                    payload.get(..MAX_SPS_BYTES * 3 / 2).unwrap_or(payload),
                    &mut buffer,
                )?;
                buffer.truncate(MAX_SPS_BYTES);
                let sps = parse_sps(&buffer)?;
                one_picture_bound(&sps, &self.limits)?;
                if let Some(slot) = self.sps.get_mut(sps.id as usize) {
                    *slot = Some(Arc::new(sps));
                }
                Ok(())
            }
            NalKind::Pps => {
                let payload = nal.get(1..).unwrap_or_default();
                let mut buffer = Vec::new();
                bounded_rbsp(
                    payload.get(..MAX_PPS_BYTES * 3 / 2).unwrap_or(payload),
                    &mut buffer,
                )?;
                buffer.truncate(MAX_PPS_BYTES);
                let id = BitReader::new(&buffer).ue()? as usize;
                let slot = self
                    .pps
                    .get_mut(id)
                    .ok_or(Error::Invalid("pic_parameter_set_id"))?;
                let mut kept = Vec::new();
                kept.try_reserve_exact(buffer.len())
                    .map_err(|_| Error::Limit(LimitKind::Memory))?;
                kept.extend_from_slice(&buffer);
                *slot = Some(kept);
                if let Some(resolved) = self.resolved.get_mut(id) {
                    *resolved = None;
                }
                Ok(())
            }
            NalKind::Slice | NalKind::IdrSlice => {
                Err(Error::OneIdr(OneIdrError::AccessUnitRequired))
            }
            NalKind::Partition | NalKind::Other(_) => Err(Error::OneIdr(OneIdrError::ExpectedIdr)),
            _ => Ok(()),
        }
    }

    pub(super) fn one_idr_settled(&mut self) -> Result<()> {
        match self.one_idr {
            OneIdrState::Finished => Ok(()),
            OneIdrState::Failed => Err(Error::OneIdr(OneIdrError::Failed)),
            _ => {
                self.fail_one_idr();
                Err(Error::OneIdr(OneIdrError::AccessUnitRequired))
            }
        }
    }

    pub(super) fn fail_one_idr(&mut self) {
        self.one_idr = OneIdrState::Failed;
        self.current = None;
        self.dpb.clear();
        self.ready.clear();
    }

    pub(super) fn check_one_idr_start(&self, hdr: &SliceHeader, sps: &Sps) -> Result<()> {
        if self.started {
            return Err(Error::OneIdr(OneIdrError::SecondPicture));
        }
        if self.one_idr != OneIdrState::Decoding || !hdr.idr || hdr.slice_type != SliceType::I {
            return Err(Error::OneIdr(OneIdrError::AccessUnitRequired));
        }
        one_picture_bound(sps, &self.limits)?;
        Ok(())
    }
}

fn one_picture_bound(sps: &Sps, limits: &Limits) -> Result<u64> {
    check_sps_scope(sps)?;
    if sps.max_num_ref_frames as usize > limits.max_dpb_frames {
        return Err(Error::Limit(LimitKind::DpbFrames));
    }
    let a = u64::from(sps.width_mbs);
    let b = u64::from(sps.frame_height_mbs());
    let mbs = a
        .checked_mul(b)
        .filter(|&n| n > 0 && n <= limits.max_mbs as u64)
        .ok_or(Error::Limit(LimitKind::PictureSize))?;
    let memory = || {
        let w = a.checked_mul(16)?;
        let h = b.checked_mul(16)?;
        let stored = w
            .checked_mul(h.checked_add(8)?)?
            .checked_add(w.checked_mul((h / 2).checked_add(4)?)?)?
            .checked_add(mbs.checked_mul(crate::picture::MOTION_BYTES_PER_MB as u64)?)?;
        stored
            .checked_add(mbs.checked_mul(384 + THREAD_BYTES_PER_MB)?)?
            .checked_add(THREAD_BYTES)?
            .checked_add(PACKET_CEILING as u64)?
            .checked_add(DECODER_BYTES)
    };
    memory()
        .filter(|&n| n <= MEMORY_CEILING.min(limits.max_memory_bytes))
        .ok_or(Error::Limit(LimitKind::Memory))
}

fn bounded_rbsp(payload: &[u8], buffer: &mut Vec<u8>) -> Result<()> {
    buffer.clear();
    if buffer.capacity() < payload.len() {
        *buffer = Vec::new();
        buffer
            .try_reserve_exact(payload.len())
            .map_err(|_| Error::Limit(LimitKind::Memory))?;
    }
    rbsp(payload, buffer);
    Ok(())
}

fn strict_annex_b(data: &[u8]) -> Result<()> {
    let mut first = true;
    for (at, _) in data
        .windows(3)
        .enumerate()
        .filter(|(_, bytes)| *bytes == [0, 0, 1])
    {
        if first
            && !data
                .get(..at)
                .is_some_and(|bytes| bytes.iter().all(|&b| b == 0))
        {
            return Err(Error::Invalid("bytes before the Annex B start code"));
        }
        if data.get(at + 3).is_none_or(|&b| b == 0) {
            return Err(Error::Truncated("the NAL unit header"));
        }
        first = false;
    }
    if first {
        return Err(Error::Truncated("the Annex B start code"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_streams::*;

    #[test]
    fn target_layouts_fit_the_fixed_allowances() {
        let thread = size_of::<crate::mb::MbData>()
            + size_of::<crate::inter::Scratch>()
            + size_of::<crate::recon::CoeffTables>()
            + size_of::<crate::deblock::ThresholdTable>()
            + size_of::<crate::params::LevelScale>()
            + size_of::<PictureWork<Auto>>()
            + size_of::<SliceJob>()
            + 4096;
        assert!(
            thread as u64 <= THREAD_BYTES,
            "thread fixed bytes: {thread}"
        );
        let arc = 2 * size_of::<usize>();
        let sps = size_of::<Sps>() + 255 * size_of::<i32>() + arc;
        let fixed = MAX_SPS * sps
            + MAX_PPS * (MAX_PPS_BYTES + sps + size_of::<Pps>() + arc)
            + size_of::<Decoder<Auto>>()
            + MAX_SPS * size_of::<Option<Arc<Sps>>>()
            + MAX_PPS * (size_of::<Option<Vec<u8>>>() + size_of::<Option<(Arc<Sps>, Arc<Pps>)>>())
            + MAX_WARNINGS * size_of::<Warning>()
            + 2 * MAX_SPS_BYTES * 3 / 2
            + 2 * MAX_PPS_BYTES * 3 / 2
            + 128 * 1024;
        assert!(
            fixed as u64 <= DECODER_BYTES,
            "decoder fixed bytes: {fixed}"
        );
    }

    #[test]
    fn the_second_picture_is_refused_before_starting_the_first() {
        let mut data = nal(3, 7, sps_with_max_refs(2, 2, 16));
        data.extend(nal(3, 8, pps()));
        data.extend(nal(3, 5, idr_pcm_slice(4, pcm_samples)));
        data.extend(nal(3, 5, idr_pcm_slice(4, pcm_samples)));
        let mut decoder = Decoder::new(Limits::default()).with_mode(Mode::OneIdr);
        assert_eq!(
            decoder.decode_one_idr(&data, None),
            Err(Error::OneIdr(OneIdrError::SecondPicture))
        );
        assert!(!decoder.started);
        assert_eq!(decoder.seq, 0);
        assert!(decoder.pool.is_empty());
        assert!(decoder.take_pictures().is_empty());
    }
}
