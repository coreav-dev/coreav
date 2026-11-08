use super::{
    Cell, Frame, InterStats, Memory, ReferenceState, RetainedPicture, allocation_bytes,
    rows::SampleRows,
};
use crate::{Error, PictureInfo, Result, intra::IntraStats, parser::SliceInput};
use core::ops::Range;

pub(crate) trait Sink {
    fn motion(&mut self, band: MotionBand<'_>) -> Result<()>;
    fn samples(&mut self, band: SampleBand<'_>) -> Result<()>;
}

pub(crate) struct MotionBand<'a> {
    pub(super) rows: Range<usize>,
    pub(super) cells: &'a [Cell],
}

pub(crate) struct SampleBand<'a> {
    pub(super) origin: usize,
    pub(super) dirty: Range<usize>,
    pub(super) filtered_through: usize,
    pub(super) planes: &'a [Vec<u16>; 3],
}

pub(crate) struct Producer {
    frame: Frame,
    window: Window,
    failed: bool,
}

struct Window {
    origin: usize,
    reconstructed: usize,
    filtered: usize,
    intra: [Vec<u16>; 3],
    pre_sao: [Vec<u16>; 3],
    source: Vec<u16>,
}

pub(crate) struct Completion {
    pub(super) index: u64,
    pub(super) info: PictureInfo,
    pub(crate) stats: IntraStats,
    pub(crate) inter_stats: InterStats,
    pub(super) reference: Option<ReferenceState>,
}

impl Completion {
    pub(super) fn retain(
        self,
        planes: [Vec<u16>; 3],
        cells: Vec<Cell>,
        timestamp: Option<i64>,
        origin: Option<u64>,
    ) -> Result<RetainedPicture> {
        let width = self.info.width as usize;
        let height = self.info.height as usize;
        if planes[0].len() != width * height
            || planes[1].len() != width / 2 * (height / 2)
            || planes[2].len() != planes[1].len()
            || cells.len() != width / 4 * (height / 4)
        {
            return Err(Error::InvalidData("selected_picture_incomplete"));
        }
        Ok(RetainedPicture {
            index: self.index,
            info: self.info,
            planes,
            cells,
            timestamp,
            origin,
            stats: self.stats,
            inter_stats: self.inter_stats,
            reference: self.reference,
        })
    }
}

impl Producer {
    #[expect(
        clippy::result_large_err,
        reason = "return the untouched fallback owner without allocating"
    )]
    pub(crate) fn try_from_full(
        mut frame: Frame,
        fixed: usize,
        limit: usize,
    ) -> core::result::Result<(Self, RetainedPicture), (Frame, Error)> {
        let prepared = (|| {
            if frame.info.tiles || frame.info.wpp {
                return Err(Error::Unsupported("raster_picture_layout"));
            }
            let width = frame.info.width as usize;
            let height = frame.info.height as usize;
            if frame.next_ctu != 0
                || frame.segment_closed
                || frame.filtered
                || frame.hash_seen
                || frame.wpp.is_some()
                || frame.reference.is_none()
                || frame.stats != IntraStats::default()
                || frame.inter_stats != InterStats::default()
                || frame.filter_ctus.iter().any(|ctu| ctu.seen)
                || frame.cells.iter().any(|cell| cell.reconstructed != 0)
                || frame.planes[0].len() != width * height
                || frame.planes[1].len() != width / 2 * (height / 2)
                || frame.planes[2].len() != frame.planes[1].len()
                || frame.cells.len() != width / 4 * (height / 4)
            {
                return Err(Error::InvalidData("raster_picture_state"));
            }
            let mut memory = Memory::new(fixed, limit)?;
            memory.charge(frame.heap_bytes()?)?;
            let ctb = 1usize << frame.info.ctb_log2;
            let capacity = (2 * ctb + 4).min(height);
            let mut planes = [
                memory.allocate(capacity * width, 0)?,
                memory.allocate(capacity / 2 * (width / 2), 0)?,
                memory.allocate(capacity / 2 * (width / 2), 0)?,
            ];
            let mut cells = memory.allocate(capacity / 4 * (width / 4), Cell::empty())?;
            let window = Window::allocate(frame.info, &mut memory)?;
            let end = (2 * ctb).min(height);
            for (component, plane) in planes.iter_mut().enumerate() {
                let scale = if component == 0 { 1 } else { 2 };
                plane.truncate(end / scale * (width / scale));
            }
            cells.truncate(end / 4 * (width / 4));
            Ok((planes, cells, window))
        })();
        let (planes, cells, window) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => return Err((frame, error)),
        };
        let backing = RetainedPicture {
            index: frame.index,
            info: frame.info,
            planes: core::mem::replace(&mut frame.planes, planes),
            cells: core::mem::replace(&mut frame.cells, cells),
            timestamp: None,
            origin: None,
            stats: frame.stats,
            inter_stats: frame.inter_stats,
            reference: frame.reference,
        };
        Ok((
            Self {
                frame,
                window,
                failed: false,
            },
            backing,
        ))
    }

    pub(crate) fn heap_bytes(&self) -> Result<usize> {
        self.window
            .extra_bytes()?
            .checked_add(self.frame.heap_bytes()?)
            .ok_or(Error::Limit("reconstruction_memory"))
    }

    pub(crate) fn decode(
        &mut self,
        input: SliceInput<'_>,
        references: &dyn crate::dpb::ReferenceProvider,
        remaining: &mut u64,
        fixed: usize,
        limit: usize,
        sink: &mut impl Sink,
    ) -> Result<usize> {
        if self.failed {
            return Err(Error::InvalidData("selected_segment_state"));
        }
        let result = (|| {
            if input.syntax.kind > 2 || input.syntax.kind < 2 && input.syntax.inter.is_none() {
                return Err(Error::InvalidData("inter_segment_state"));
            }
            let fixed = fixed
                .checked_add(self.window.extra_bytes()?)
                .ok_or(Error::Limit("reconstruction_memory"))?;
            let (ranges, _, peak) = self.frame.prepare_slice(&input, fixed, limit, None)?;
            if input.pps.tiles || input.pps.wpp {
                return Err(Error::Unsupported("raster_picture_layout"));
            }
            let window = &mut self.window;
            self.frame
                .traverse_slice(&input, &ranges, remaining, |frame, slice, cabac| {
                    let location = frame.decode_ctu_rows(
                        &input,
                        Some(references),
                        slice,
                        cabac,
                        SampleRows {
                            origin: window.origin,
                            above: (window.reconstructed != 0).then_some(&window.intra),
                            above_y: window.reconstructed,
                        },
                    )?;
                    if location.x + 1 == input.layout.width {
                        window.row_complete(frame, sink)?;
                    }
                    Ok(location)
                })?;
            if self.frame.complete() {
                self.window.filter_band(&mut self.frame, sink)?;
                self.frame.filtered = true;
            }
            Ok(peak)
        })();
        self.failed = result.is_err();
        result
    }

    pub(crate) fn finish(self) -> Result<Completion> {
        if self.failed
            || !self.frame.complete()
            || !self.frame.filtered
            || self.window.filtered != self.frame.info.height as usize
            || self.frame.filter_ctus.iter().any(|ctu| !ctu.seen)
        {
            return Err(Error::InvalidData("selected_picture_incomplete"));
        }
        Ok(Completion {
            index: self.frame.index,
            info: self.frame.info,
            stats: self.frame.stats,
            inter_stats: self.frame.inter_stats,
            reference: self.frame.reference,
        })
    }

    pub(crate) fn hash(&mut self, sei: crate::SeiInfo, planes: [&[u16]; 3]) -> Result<()> {
        if sei.picture_hash.is_none() && sei.reserved_hashes == 0 {
            return Ok(());
        }
        if !self.frame.complete() || self.frame.filter_ctus.iter().any(|ctu| !ctu.seen) {
            return Err(Error::InvalidData("selected_picture_incomplete"));
        }
        if !self.frame.filtered || self.frame.hash_seen {
            return Err(Error::InvalidData("selected_picture_hash_placement"));
        }
        self.frame.hash_seen = true;
        if let Some(expected) = sei.picture_hash {
            crate::picture_hash::verify(
                &expected,
                planes,
                self.frame.info.width as usize,
                self.frame.info.height as usize,
                self.frame.info.bit_depth,
            )?;
            self.frame.stats.picture_hashes += 1;
        }
        self.frame.stats.reserved_hashes += u64::from(sei.reserved_hashes);
        Ok(())
    }
}

impl Window {
    fn allocate(info: PictureInfo, memory: &mut Memory<'_>) -> Result<Self> {
        let width = info.width as usize;
        Ok(Self {
            origin: 0,
            reconstructed: 0,
            filtered: 0,
            intra: [
                memory.allocate(width, 0)?,
                memory.allocate(width / 2, 0)?,
                memory.allocate(width / 2, 0)?,
            ],
            pre_sao: [
                memory.allocate(width, 0)?,
                memory.allocate(width / 2, 0)?,
                memory.allocate(width / 2, 0)?,
            ],
            source: memory.allocate(
                (2 * (1usize << info.ctb_log2) + 4).min(info.height as usize) * width,
                0,
            )?,
        })
    }

    fn extra_bytes(&self) -> Result<usize> {
        self.intra.iter().chain(&self.pre_sao).try_fold(
            allocation_bytes(&self.source)?,
            |total, plane| {
                total
                    .checked_add(allocation_bytes(plane)?)
                    .ok_or(Error::Limit("reconstruction_memory"))
            },
        )
    }

    fn row_complete(&mut self, frame: &mut Frame, sink: &mut impl Sink) -> Result<()> {
        let width = frame.info.width as usize;
        let ctb = 1usize << frame.info.ctb_log2;
        let start = self.reconstructed;
        let end = (start + ctb).min(frame.info.height as usize);
        for (component, (border, plane)) in self.intra.iter_mut().zip(&frame.planes).enumerate() {
            let scale = if component == 0 { 1 } else { 2 };
            let width = width / scale;
            let last = (end / scale - self.origin / scale - 1) * width;
            border.copy_from_slice(
                plane
                    .get(last..last + width)
                    .ok_or(Error::Internal("raster_intra_border"))?,
            );
        }
        let begin = (start - self.origin) / 4 * (width / 4);
        let last = (end - self.origin) / 4 * (width / 4);
        sink.motion(MotionBand {
            rows: start..end,
            cells: frame
                .cells
                .get(begin..last)
                .ok_or(Error::Internal("raster_motion_band"))?,
        })?;
        let storage = SampleRows {
            origin: self.origin,
            above: None,
            above_y: 0,
        };
        for component in 0..2 {
            frame.deblock_rows(component, false, start..end, storage)?;
        }
        self.reconstructed = end;
        if start != 0 {
            self.filter_band(frame, sink)?;
            if end != frame.info.height as usize {
                self.advance(frame);
            }
        }
        Ok(())
    }

    fn filter_band(&mut self, frame: &mut Frame, sink: &mut impl Sink) -> Result<()> {
        let start = self.filtered;
        let end = (start + (1usize << frame.info.ctb_log2)).min(frame.info.height as usize);
        let storage = SampleRows {
            origin: self.origin,
            above: None,
            above_y: 0,
        };
        for component in 0..2 {
            frame.deblock_rows(component, true, start..end, storage)?;
        }
        for (component, previous) in self.pre_sao.iter_mut().enumerate() {
            let scale = if component == 0 { 1 } else { 2 };
            let width = frame.info.width as usize / scale;
            let mut output = core::mem::take(
                frame
                    .planes
                    .get_mut(component)
                    .ok_or(Error::Internal("raster_component"))?,
            );
            let result = (|| {
                let source = self
                    .source
                    .get_mut(..output.len())
                    .ok_or(Error::Internal("raster_source"))?;
                source.copy_from_slice(&output);
                if start != 0 {
                    let above = (start / scale - self.origin / scale - 1) * width;
                    source
                        .get_mut(above..above + width)
                        .ok_or(Error::Internal("raster_sao_border"))?
                        .copy_from_slice(previous);
                }
                let last = (end / scale - self.origin / scale - 1) * width;
                previous.copy_from_slice(
                    source
                        .get(last..last + width)
                        .ok_or(Error::Internal("raster_sao_border"))?,
                );
                frame.sao_rows(component, Some(source), &mut output, start..end, storage)
            })();
            *frame
                .planes
                .get_mut(component)
                .ok_or(Error::Internal("raster_component"))? = output;
            result?;
        }
        sink.samples(SampleBand {
            origin: self.origin,
            dirty: start..self.reconstructed,
            filtered_through: end,
            planes: &frame.planes,
        })?;
        self.filtered = end;
        Ok(())
    }

    fn advance(&mut self, frame: &mut Frame) {
        let origin = self.filtered.saturating_sub(4);
        let end =
            (self.filtered + 2 * (1usize << frame.info.ctb_log2)).min(frame.info.height as usize);
        for (component, plane) in frame.planes.iter_mut().enumerate() {
            let scale = if component == 0 { 1 } else { 2 };
            let width = frame.info.width as usize / scale;
            let begin = (origin - self.origin) / scale * width;
            let retained = plane.len() - begin;
            plane.copy_within(begin.., 0);
            plane.truncate(retained);
            plane.resize((end - origin) / scale * width, 0);
        }
        let begin = (origin - self.origin) / 4 * (frame.info.width as usize / 4);
        let retained = frame.cells.len() - begin;
        frame.cells.copy_within(begin.., 0);
        frame.cells.truncate(retained);
        frame.cells.resize(
            (end - origin) / 4 * (frame.info.width as usize / 4),
            Cell::empty(),
        );
        self.origin = origin;
    }
}

#[cfg(test)]
pub(in crate::reconstruction) mod tests {
    use super::*;
    use crate::dpb::References;

    pub(in crate::reconstruction) fn set_deadline(
        producer: &mut Producer,
        deadline: Option<std::time::Instant>,
    ) {
        producer.frame.deadline = deadline;
    }

    pub(in crate::reconstruction) fn conversion(input: SliceInput<'_>) -> Result<()> {
        let frame = Frame::new_full(&input, 0, References::default(), 0, usize::MAX, None)?;
        let original_bytes = frame.heap_bytes()?;
        let (producer, backing) =
            Producer::try_from_full(frame, 0, usize::MAX).map_err(|(_, error)| error)?;
        let bytes = producer.heap_bytes()? + backing.heap_bytes()?;
        #[cfg(feature = "native-hevc-rows-private")]
        {
            let capacity =
                super::super::continuation_capacity(input.picture, input.sps.ctu_count as usize)?;
            assert_eq!(backing.heap_bytes()?, capacity.retained);
            assert_eq!(original_bytes, capacity.retained + capacity.base);
            assert_eq!(producer.heap_bytes()?, capacity.base + capacity.row_extra);
        }
        let fixed = 17 + input.scratch_bytes;
        let dpb = crate::dpb::Dpb::new_reconstruction();
        for limit in [
            fixed + original_bytes - 1,
            fixed + original_bytes,
            fixed + bytes - 1,
            fixed + bytes,
        ] {
            let frame = Frame::new_full(&input, 0, References::default(), 0, usize::MAX, None)?;
            let planes = frame.planes.each_ref().map(|plane| plane.as_ptr());
            let cells = frame.cells.as_ptr();
            let metadata = frame.filter_ctus.as_ptr();
            let scratch = frame.scratch.coefficients.as_ptr();
            let contexts = frame.contexts;
            let qp = frame.qp;
            match Producer::try_from_full(frame, fixed, limit) {
                Ok((producer, backing)) => {
                    assert_eq!(limit, fixed + bytes);
                    assert_eq!(
                        backing.planes.each_ref().map(|plane| plane.as_ptr()),
                        planes
                    );
                    assert_eq!(backing.cells.as_ptr(), cells);
                    assert_eq!(producer.frame.filter_ctus.as_ptr(), metadata);
                    assert_eq!(producer.frame.scratch.coefficients.as_ptr(), scratch);
                    assert_eq!(producer.frame.contexts, contexts);
                    assert_eq!(producer.frame.qp, qp);
                    assert_eq!(producer.heap_bytes()? + backing.heap_bytes()?, bytes);
                }
                Err((mut frame, error)) => {
                    assert!(limit < fixed + bytes);
                    assert_eq!(error, Error::Limit("reconstruction_memory"));
                    assert_eq!(frame.planes.each_ref().map(|plane| plane.as_ptr()), planes);
                    assert_eq!(frame.cells.as_ptr(), cells);
                    assert_eq!(frame.filter_ctus.as_ptr(), metadata);
                    assert_eq!(frame.scratch.coefficients.as_ptr(), scratch);
                    assert_eq!(frame.contexts, contexts);
                    assert_eq!(frame.qp, qp);
                    assert_eq!(frame.heap_bytes()?, original_bytes);
                    assert_eq!(frame.next_ctu, 0);
                    assert!(!frame.segment_closed);
                    assert_eq!(frame.stats, IntraStats::default());
                    frame.decode_full(input, &dpb, &mut 1000, 0, usize::MAX, 1, None)?;
                    assert!(frame.complete());
                }
            }
        }
        let mut frame = Frame::new_full(&input, 0, References::default(), 0, usize::MAX, None)?;
        frame.next_ctu = 1;
        match Producer::try_from_full(frame, 0, usize::MAX) {
            Ok(_) => panic!("partially reconstructed frame accepted"),
            Err((frame, error)) => {
                assert_eq!(frame.next_ctu, 1);
                assert_eq!(error, Error::InvalidData("raster_picture_state"));
            }
        }
        Ok(())
    }

    pub(in crate::reconstruction) fn producer(
        input: &SliceInput<'_>,
        poc: i32,
        references: References,
    ) -> Result<(Producer, TestSink)> {
        let frame = Frame::new_full(input, poc, references, 0, usize::MAX, None)?;
        let (producer, backing) = Producer::try_from_full(frame, input.scratch_bytes, usize::MAX)
            .map_err(|(_, error)| error)?;
        Ok((producer, TestSink::from_backing(backing)))
    }

    pub(in crate::reconstruction) fn assert_entropy(producer: &Producer, serial: &Frame) {
        assert_eq!(producer.frame.contexts, serial.contexts);
        assert_eq!(producer.frame.qp, serial.qp);
        assert_eq!(producer.frame.next_ctu, serial.next_ctu);
        assert_eq!(producer.frame.segment_closed, serial.segment_closed);
        assert_eq!(producer.frame.stats, serial.stats);
    }

    pub(in crate::reconstruction) fn intra_border(input: SliceInput<'_>) -> Result<()> {
        let dpb = crate::dpb::Dpb::new_reconstruction();
        let initial = SliceInput {
            header: crate::SliceHeader {
                first: true,
                address: 0,
                ..input.header
            },
            ..input
        };
        let mut serial = Frame::new_full(&initial, 0, References::default(), 0, usize::MAX, None)?;
        let (mut producer, mut sink) = producer(&initial, 0, References::default())?;
        let width = input.picture.width as usize;
        let ctb = 1usize << input.picture.ctb_log2;
        for frame in [&mut serial, &mut producer.frame] {
            for (component, plane) in frame.planes.iter_mut().enumerate() {
                let scale = if component == 0 { 1 } else { 2 };
                for (index, sample) in plane
                    .iter_mut()
                    .take(width / scale * (ctb / scale))
                    .enumerate()
                {
                    *sample = ((80 + (index % (width / scale) / (8 / scale)) % 2 * 6)
                        << (input.picture.bit_depth - 8)) as u16;
                }
            }
            for (index, cell) in frame
                .cells
                .iter_mut()
                .take(width / 4 * (ctb / 4))
                .enumerate()
            {
                let (x, y) = (index % (width / 4) * 4, index / (width / 4) * 4);
                cell.mode = Some(super::super::IntraMode::new(26)?);
                cell.reconstructed = 7;
                cell.qp = 30;
                cell.slice = 1;
                cell.edges = u8::from(x % 8 == 0) | (u8::from(y % 8 == 0) << 1);
                cell.transform_edges = cell.edges;
            }
            frame.filter_ctus[0] = super::super::filter::Ctu {
                seen: true,
                order: 0,
                slice: 1,
                across_slices: true,
                ..Default::default()
            };
            frame.next_ctu = 1;
            frame.segment_closed = true;
        }
        let unfiltered = serial.planes[0][(ctb - 1) * width..ctb * width].to_vec();
        producer
            .window
            .row_complete(&mut producer.frame, &mut sink)?;
        assert_eq!(producer.window.intra[0], unfiltered);
        assert_ne!(
            &producer.frame.planes[0][(ctb - 1) * width..ctb * width],
            &unfiltered
        );
        assert!(producer.frame.stats.deblock_changed_samples[0] > 0);
        assert!(producer.frame.inter_stats.boundary_strengths[2] > 0);
        let mut serial_bins = 1000;
        let mut raster_bins = 1000;
        serial.decode_full(input, &dpb, &mut serial_bins, 0, usize::MAX, 1, None)?;
        producer.decode(input, &dpb, &mut raster_bins, 0, usize::MAX, &mut sink)?;
        serial.finalize(0, usize::MAX)?;
        let actual = sink.retain(producer.finish()?)?;
        assert_eq!(actual.stats.pcm_units, 0);
        assert!(actual.cells[ctb / 4 * (width / 4)].mode.is_some());
        assert_eq!(raster_bins, serial_bins);
        assert_eq!(actual.planes, serial.planes);
        assert_eq!(actual.stats, serial.stats);
        assert_eq!(actual.inter_stats, serial.inter_stats);
        Ok(())
    }

    pub(in crate::reconstruction) fn qp_continuation(
        first: SliceInput<'_>,
        second: SliceInput<'_>,
    ) -> Result<()> {
        let dpb = crate::dpb::Dpb::new_reconstruction();
        let mut serial = Frame::new_full(&first, 0, References::default(), 0, usize::MAX, None)?;
        let (mut producer, mut sink) = producer(&first, 0, References::default())?;
        let mut serial_bins = 1000;
        let mut producer_bins = 1000;
        serial.decode_full(first, &dpb, &mut serial_bins, 0, usize::MAX, 1, None)?;
        producer.decode(first, &dpb, &mut producer_bins, 0, usize::MAX, &mut sink)?;
        assert_eq!(sink.motion, 0);
        assert_eq!(sink.filtered, 0);
        serial.qp = super::super::syntax::Qp::new(29);
        producer.frame.qp = serial.qp;
        serial.decode_full(second, &dpb, &mut serial_bins, 0, usize::MAX, 1, None)?;
        producer.decode(second, &dpb, &mut producer_bins, 0, usize::MAX, &mut sink)?;
        assert_entropy(&producer, &serial);
        assert_eq!(serial_bins, producer_bins);
        let actual = sink.retain(producer.finish()?)?;
        assert_eq!(
            actual.cells.iter().map(|cell| cell.qp).collect::<Vec<_>>(),
            serial.cells.iter().map(|cell| cell.qp).collect::<Vec<_>>()
        );
        assert!(actual.cells.iter().any(|cell| cell.qp == 29));
        Ok(())
    }

    pub(in crate::reconstruction) struct TestSink {
        pub(in crate::reconstruction) picture: RetainedPicture,
        pub(in crate::reconstruction) motion: usize,
        pub(in crate::reconstruction) filtered: usize,
        pub(in crate::reconstruction) commits: usize,
        prefix: [Vec<u16>; 3],
    }

    impl TestSink {
        pub(in crate::reconstruction) fn new(info: PictureInfo) -> Self {
            let mut picture = RetainedPicture::for_test(
                0,
                PictureInfo {
                    width: 8,
                    height: 8,
                    ..info
                },
                None,
            );
            picture.info = info;
            let width = info.width as usize;
            let height = info.height as usize;
            picture.planes = [
                vec![0; width * height],
                vec![0; width / 2 * (height / 2)],
                vec![0; width / 2 * (height / 2)],
            ];
            picture.cells = vec![picture.cells[0]; width / 4 * (height / 4)];
            Self::from_backing(picture)
        }

        fn from_backing(picture: RetainedPicture) -> Self {
            Self {
                picture,
                motion: 0,
                filtered: 0,
                commits: 0,
                prefix: core::array::from_fn(|_| Vec::new()),
            }
        }

        pub(in crate::reconstruction) fn retain(
            self,
            completion: Completion,
        ) -> Result<RetainedPicture> {
            completion.retain(self.picture.planes, self.picture.cells, None, None)
        }
    }

    impl Sink for TestSink {
        fn motion(&mut self, band: MotionBand<'_>) -> Result<()> {
            assert_eq!(band.rows.start, self.motion);
            let stride = self.picture.info.width as usize / 4;
            self.picture.cells[band.rows.start / 4 * stride..band.rows.end / 4 * stride]
                .copy_from_slice(band.cells);
            self.motion = band.rows.end;
            Ok(())
        }

        fn samples(&mut self, band: SampleBand<'_>) -> Result<()> {
            assert_eq!(band.dirty.start, self.filtered);
            assert!(band.filtered_through <= band.dirty.end);
            assert!(band.dirty.end <= self.motion);
            for component in 0..3 {
                let scale = if component == 0 { 1 } else { 2 };
                let width = self.picture.info.width as usize / scale;
                let plane = &mut self.picture.planes[component];
                let prefix = self.filtered / scale * width;
                assert_eq!(&plane[..prefix], &self.prefix[component]);
                let begin = band.dirty.start / scale * width;
                let end = band.dirty.end / scale * width;
                let origin = band.origin / scale * width;
                plane[begin..end]
                    .copy_from_slice(&band.planes[component][begin - origin..end - origin]);
                assert_eq!(&plane[..prefix], &self.prefix[component]);
                self.prefix[component] = plane[..band.filtered_through / scale * width].to_vec();
            }
            self.filtered = band.filtered_through;
            self.commits += 1;
            Ok(())
        }
    }

    pub(in crate::reconstruction) fn filter_fixture(mut frame: Frame) -> Result<RetainedPicture> {
        let width = frame.info.width as usize;
        let height = frame.info.height as usize;
        let ctb = 1usize << frame.info.ctb_log2;
        let samples = core::mem::take(&mut frame.planes);
        let cells = core::mem::take(&mut frame.cells);
        let capacity = (2 * ctb + 4).min(height);
        let mut memory = Memory::new(0, usize::MAX)?;
        frame.planes = [
            memory.allocate(capacity * width, 0)?,
            memory.allocate(capacity / 2 * (width / 2), 0)?,
            memory.allocate(capacity / 2 * (width / 2), 0)?,
        ];
        frame.cells = memory.allocate(capacity / 4 * (width / 4), cells[0])?;
        let mut sink = TestSink::new(frame.info);
        let window = Window::allocate(frame.info, &mut memory)?;
        let end = (2 * ctb).min(height);
        for (component, plane) in frame.planes.iter_mut().enumerate() {
            let scale = if component == 0 { 1 } else { 2 };
            plane.truncate(end / scale * (width / scale));
        }
        frame.cells.truncate(end / 4 * (width / 4));
        let mut producer = Producer {
            frame,
            window,
            failed: false,
        };
        let bytes = producer.heap_bytes()?;
        for start in (0..height).step_by(ctb) {
            let end = (start + ctb).min(height);
            let window = &mut producer.window;
            let frame = &mut producer.frame;
            for (component, source) in samples.iter().enumerate() {
                let scale = if component == 0 { 1 } else { 2 };
                let width = width / scale;
                let begin = start / scale * width;
                let last = end / scale * width;
                let origin = window.origin / scale * width;
                frame.planes[component][begin - origin..last - origin]
                    .copy_from_slice(&source[begin..last]);
            }
            let begin = start / 4 * (width / 4);
            let last = end / 4 * (width / 4);
            let origin = window.origin / 4 * (width / 4);
            frame.cells[begin - origin..last - origin].copy_from_slice(&cells[begin..last]);
            window.row_complete(frame, &mut sink)?;
            assert_eq!(producer.heap_bytes()?, bytes);
            assert!(producer.frame.planes[0].capacity() <= capacity * width);
            for (component, source) in samples.iter().enumerate() {
                let scale = if component == 0 { 1 } else { 2 };
                let width = width / scale;
                let last = (end / scale - 1) * width;
                assert_eq!(producer.window.intra[component], source[last..last + width]);
            }
        }
        producer
            .window
            .filter_band(&mut producer.frame, &mut sink)?;
        producer.frame.filtered = true;
        assert_eq!(sink.filtered, height);
        assert_eq!(sink.commits, height.div_ceil(ctb));
        sink.retain(producer.finish()?)
    }
}
