use super::*;
use crate::tests::writer;
use writer::Writer;

#[path = "../../test-support/pcm.rs"]
#[expect(
    clippy::duplicate_mod,
    reason = "Standalone generated fixtures retain their existing test-only visibility"
)]
mod pcm;

fn profile(writer: &mut Writer) {
    writer.u(2, 0);
    writer.flag(false);
    writer.u(5, 1);
    writer.u(32, 0);
    writer.u(4, 0);
    writer.u(32, 0);
    writer.u(12, 0);
    writer.u(8, 120);
}

fn stream(chain: bool) -> Result<Vec<Vec<u8>>> {
    stream_kind(chain, 1)
}

#[path = "../../test-support/raster.rs"]
mod raster;
use raster::stream as raster_stream;

#[cfg(feature = "native-hevc-rows-private")]
use crate::native_rows::{OutputHeader, RowSink, SinkCost};
#[cfg(feature = "native-hevc-rows-private")]
struct NativeRowsSink {
    headers: Vec<OutputHeader>,
    output: Vec<Output>,
    packet: usize,
}
#[cfg(feature = "native-hevc-rows-private")]
impl RowSink for NativeRowsSink {
    fn prepare(
        &mut self,
        header: OutputHeader,
        memory: &mut OutputMemory<'_>,
        bounded: bool,
    ) -> Result<bool> {
        if bounded {
            assert!(self.headers.len() < self.headers.capacity());
        }
        memory.reserve(
            (header.info.visible_width as usize * header.info.visible_height as usize * 3)
                + core::mem::size_of::<Output>(),
        )?;
        self.headers.push(header);
        Ok(true)
    }
    fn reserve_queue(
        &mut self,
        records: usize,
        memory: &mut OutputMemory<'_>,
        admit: &mut dyn FnMut(usize) -> Result<bool>,
    ) -> Result<bool> {
        let mut headers = Vec::new();
        let mut output = Vec::new();
        headers
            .try_reserve_exact(records)
            .map_err(|_| Error::Limit("picture_allocation"))?;
        output
            .try_reserve_exact(records)
            .map_err(|_| Error::Limit("picture_allocation"))?;
        if headers.capacity() != output.capacity() || !admit(headers.capacity())? {
            return Ok(false);
        }
        memory.reserve(
            headers.capacity()
                * (core::mem::size_of::<OutputHeader>() + core::mem::size_of::<Output>()),
        )?;
        while !self.headers.is_empty() {
            headers.push(self.headers.remove(0));
        }
        while !self.output.is_empty() {
            output.push(self.output.remove(0));
        }
        self.headers = headers;
        self.output = output;
        Ok(true)
    }
    fn qualify(&self, _: PictureInfo, _: &mut crate::native_rows::CapacityProbe) -> bool {
        true
    }
    fn fill(&mut self, offset: usize, picture: DecodedPicture<'_>) -> Result<()> {
        assert_eq!(self.headers[offset].index, picture.index);
        assert_eq!(offset, self.output.len());
        capture(picture, &mut self.output);
        Ok(())
    }
    fn truncate(&mut self, keep: usize) {
        self.headers.truncate(keep);
        self.output.truncate(keep);
    }
    fn cost(&self, debt: usize, _: usize) -> Result<SinkCost> {
        assert_eq!(debt, 0);
        Ok(SinkCost {
            max_packet_bytes: self.packet,
            max_nals: 4096,
            max_pending_bytes: 1 << 20,
            queue_capacity: self.headers.capacity(),
            queued_frames: self.headers.len(),
            record_bytes: core::mem::size_of::<Output>() + core::mem::size_of::<OutputHeader>(),
            plane_records_bytes: 3 * core::mem::size_of::<Vec<u16>>(),
            owner_bytes: core::mem::size_of::<Self>(),
            debt_peak_bytes: 0,
        })
    }
}

#[cfg(feature = "native-hevc-rows-private")]
#[test]
fn dependent_raster_job_reads_a_building_predecessor_before_its_first_publication() -> Result<()> {
    let nals = raster_stream(true)?;
    let limits = DecodeLimits {
        threads: 2,
        max_memory_bytes: 128 << 20,
        ..DecodeLimits::default()
    };
    let serial = decode_raster(&nals, false, limits, None);
    let expected = semantic(serial.0?);
    let hook = Arc::new(TestHook {
        dependent_rows: true,
        ..TestHook::default()
    });
    let mut decoder = test_decoder(limits);
    decoder.initialize()?;
    decoder.session[0].pictures.frame.scheduler.hook = Some(Arc::clone(&hook));
    let mut sink = NativeRowsSink {
        headers: Vec::new(),
        output: Vec::new(),
        packet: nals.iter().map(|nal| nal.len() + 4).sum(),
    };
    for (index, nal) in nals.iter().enumerate() {
        let origin = if index < 3 {
            0
        } else {
            77 + ((index - 3) / 32).min(1) as u64
        };
        decoder.read_nal_rows(nal, Some(origin as i64), Some(origin), &mut sink)?;
    }
    assert!(decoder.has_native_rows());
    let receive = decoder
        .receive_rows()?
        .ok_or(Error::Internal("row_owner"))?;
    assert!(receive.consume.is_none());
    assert!(!receive.pop);
    assert_eq!(hook.completed.load(Ordering::Acquire), 0);
    let summary = decoder.finish_rows(&mut sink)?;
    assert_eq!(sink.output, serial.1);
    assert_eq!(semantic(summary), expected);
    assert_eq!(summary.frame_parallel_pictures, 2);
    assert_eq!(summary.max_workers, 2);
    assert_eq!(hook.completed.load(Ordering::Acquire) & 3, 3);
    assert_eq!(*hook.entered.lock().unwrap() & 2, 2);
    assert_eq!(hook.raster_conversions.load(Ordering::Relaxed), 2);
    let mut failed = stream_kind_reorder(false, 1, 2)?;
    let mut hash = vec![132, 13, 2];
    hash.extend([0; 12]);
    hash.push(0x80);
    failed.insert(5, pcm::escaped(40, &hash));
    let serial = decode(&failed, false, limits, None);
    assert!(serial.0.is_err());
    assert!(serial.1.is_empty());
    let mut decoder = test_decoder(limits);
    let mut sink = NativeRowsSink {
        headers: Vec::new(),
        output: Vec::new(),
        packet: failed.iter().map(|nal| nal.len() + 4).sum(),
    };
    let actual = (|| {
        for (index, nal) in failed.iter().enumerate() {
            decoder.read_nal_rows(nal, Some(index as i64), Some(index as u64), &mut sink)?;
        }
        assert!(decoder.has_native_rows());
        decoder.finish_rows(&mut sink)
    })();
    assert_eq!(actual.map(semantic), serial.0.map(semantic));
    assert_eq!(sink.output, serial.1);
    Ok(())
}

#[cfg(feature = "native-hevc-rows-private")]
#[test]
fn capacity_probe_mismatch_preserves_legacy_frames_and_summary() -> Result<()> {
    let nals = raster_stream(true)?;
    let limits = DecodeLimits {
        threads: 2,
        max_memory_bytes: 128 << 20,
        ..DecodeLimits::default()
    };
    let serial = decode_raster(&nals, false, limits, None);
    crate::native_rows::FORCE_CAPACITY_MISMATCH.set(true);
    let fallback: Result<()> = (|| {
        let mut decoder = test_decoder(limits);
        let mut sink = NativeRowsSink {
            headers: Vec::new(),
            output: Vec::new(),
            packet: nals.iter().map(|nal| nal.len() + 4).sum(),
        };
        for (index, nal) in nals.iter().enumerate() {
            let origin = if index < 3 {
                0
            } else {
                77 + ((index - 3) / 32).min(1) as u64
            };
            decoder.read_nal_rows(nal, Some(origin as i64), Some(origin), &mut sink)?;
            assert!(!decoder.has_native_rows());
        }
        let summary = decoder.finish_rows(&mut sink)?;
        assert_eq!(sink.output, serial.1);
        assert_eq!(semantic(summary), semantic(serial.0?));
        Ok(())
    })();
    crate::native_rows::FORCE_CAPACITY_MISMATCH.set(false);
    fallback?;
    Ok(())
}

fn decode_raster(
    nals: &[Vec<u8>],
    scheduled: bool,
    limits: DecodeLimits,
    hook: Option<Arc<TestHook>>,
) -> (Result<DecodeSummary>, Vec<Output>) {
    let mut decoder = test_decoder(limits);
    let mut output = Vec::new();
    let result = (|| {
        decoder.initialize()?;
        decoder.session[0].pictures.frame.scheduler.hook = hook;
        for (index, nal) in nals.iter().enumerate() {
            let origin = if index < 3 {
                0
            } else {
                77 + ((index - 3) / 32).min(1) as u64
            };
            let mut emit = |picture: DecodedPicture<'_>, _: &mut OutputMemory<'_>| {
                capture(picture, &mut output);
                Ok(())
            };
            if scheduled {
                decoder.read_nal_scheduled(nal, Some(origin as i64), Some(origin), &mut emit)?;
            } else {
                decoder.read_nal_with_origin(nal, Some(origin as i64), origin, &mut emit)?;
            }
            decoder.set_external_memory(0)?;
        }
        decoder.finish(|picture| {
            capture(picture, &mut output);
            Ok(())
        })
    })();
    (result, output)
}

#[test]
fn raster_reference_snapshots_reject_self_dependencies_before_any_wait() -> Result<()> {
    let info = PictureInfo {
        width: 8,
        height: 8,
        visible_width: 8,
        visible_height: 8,
        bit_depth: 8,
        profile_idc: 1,
        ctb_log2: 4,
        tiles: false,
        wpp: false,
        conformance_window: [0; 4],
        vui: Default::default(),
    };
    let backing = RetainedPicture::for_test(0, info, None);
    let identity = ReferenceIdentity {
        index: 0,
        poc: 0,
        long_term: false,
    };
    let mut memory = Memory::new(backing.heap_bytes()?, usize::MAX)?;
    let (rows, _publisher) = RowStore::new(
        identity,
        1,
        backing,
        Some(std::time::Instant::now() + std::time::Duration::from_millis(20)),
        &mut memory,
    )?;
    let ready = ReadyReferences {
        entries: vec![(
            identity,
            Arc::new(Publication {
                samples: OnceLock::new(),
                rows: Some(rows),
            }),
        )],
        ..Default::default()
    };
    let cycle = RasterReferences {
        ready: &ready,
        consumer: 0,
    };
    assert_eq!(
        cycle.reference_info(identity),
        Err(Error::Internal("reference_row_order"))
    );
    assert!(matches!(
        cycle.read_reference(
            identity,
            crate::reconstruction::ReadSupport::Motion { row: 0 }
        ),
        Err(Error::Internal("reference_row_order"))
    ));
    let ordered = RasterReferences {
        ready: &ready,
        consumer: 1,
    };
    assert_eq!(ordered.reference_info(identity)?, info);
    Ok(())
}

#[test]
fn admitted_raster_jobs_preserve_serial_frames_hash_failures_and_cold_boundaries() -> Result<()> {
    raster_job_case(false)?;
    raster_job_case(true)
}

fn raster_job_case(predicted: bool) -> Result<()> {
    let nals = raster_stream(predicted)?;
    let limits = DecodeLimits {
        threads: 2,
        ..DecodeLimits::default()
    };
    let serial = decode_raster(&nals, false, limits, None);
    assert_eq!(serial.1.len(), 2);
    if predicted {
        assert!(serial.1[1].inter.skip_units > 0);
    }
    let expected = semantic(serial.0?);
    for threads in [1, 2, 4, 8] {
        let limits = DecodeLimits { threads, ..limits };
        let hook = Arc::new(TestHook::default());
        let actual = decode_raster(&nals, true, limits, Some(Arc::clone(&hook)));
        assert_eq!(actual.1, serial.1, "threads={threads}");
        assert_eq!(semantic(actual.0?), expected);
        assert_eq!(
            hook.raster_conversions.load(Ordering::Relaxed),
            if threads == 1 { 0 } else { 2 }
        );
        if predicted && threads > 1 {
            assert_ne!(*hook.entered.lock().unwrap(), 0);
        }
    }
    let mut low = 0;
    let mut high = 8 * 1024 * 1024;
    while low + 1 < high {
        let middle = low + (high - low) / 2;
        if decode_raster(
            &nals,
            false,
            DecodeLimits {
                max_memory_bytes: middle,
                ..limits
            },
            None,
        )
        .0
        .is_ok()
        {
            high = middle;
        } else {
            low = middle;
        }
    }
    for memory in [
        low,
        high,
        high + 1,
        high + STACK_BYTES,
        high + 2 * STACK_BYTES,
        8 * 1024 * 1024,
    ] {
        let limits = DecodeLimits {
            max_memory_bytes: memory,
            ..limits
        };
        let direct = decode_raster(&nals, false, limits, None);
        let scheduled = decode_raster(&nals, true, limits, None);
        assert_eq!(scheduled.1, direct.1, "memory={memory}");
        assert_eq!(
            scheduled.0.map(semantic),
            direct.0.map(semantic),
            "memory={memory}"
        );
    }
    let mut failed = nals;
    let mut hash = vec![132, 13, 2];
    hash.extend([0; 12]);
    hash.push(0x80);
    failed.push(pcm::escaped(40, &hash));
    for threads in [1, 2, 4, 8] {
        let limits = DecodeLimits { threads, ..limits };
        let direct = decode_raster(&failed, false, limits, None);
        let scheduled = decode_raster(&failed, true, limits, None);
        assert!(direct.0.is_err());
        assert_eq!(direct.1.len(), if predicted { 0 } else { 1 });
        assert_eq!(scheduled.1, direct.1);
        assert_eq!(scheduled.0.map(semantic), direct.0.map(semantic));
    }
    Ok(())
}

fn stream_kind(chain: bool, kind: u32) -> Result<Vec<Vec<u8>>> {
    stream_kind_reorder(chain, kind, 3)
}

fn stream_kind_reorder(chain: bool, kind: u32, reorder: u32) -> Result<Vec<Vec<u8>>> {
    let fixture = pcm::pcm_fixture(8, 8, false)?;
    assert_eq!(fixture.planes.map(|plane| plane.len()), [64, 16, 16]);
    let mut vps = Writer::default();
    vps.u(4, 0);
    vps.flag(true);
    vps.flag(true);
    vps.u(6, 0);
    vps.u(3, 0);
    vps.flag(true);
    vps.u(16, 0xffff);
    profile(&mut vps);
    vps.flag(false);
    for value in [3, reorder, 0] {
        vps.ue(value);
    }
    vps.u(6, 0);
    vps.ue(0);
    vps.flag(false);
    vps.flag(false);
    let mut sps = Writer::default();
    sps.u(4, 0);
    sps.u(3, 0);
    sps.flag(true);
    profile(&mut sps);
    for value in [0, 1, 8, 8] {
        sps.ue(value);
    }
    sps.flag(false);
    sps.ue(0);
    sps.ue(0);
    sps.ue(0);
    sps.flag(false);
    for value in [3, reorder, 0, 0, 1, 0, 1, 0, 0] {
        sps.ue(value);
    }
    for flag in [false, false, false, true] {
        sps.flag(flag);
    }
    sps.u(4, 7);
    sps.u(4, 7);
    sps.ue(0);
    sps.ue(0);
    sps.flag(true);
    sps.ue(0);
    for _ in 0..5 {
        sps.flag(false);
    }
    let mut nals = vec![
        pcm::escaped(32, &vps.rbsp()),
        pcm::escaped(33, &sps.rbsp()),
        fixture.sets[2].clone(),
        fixture.nal,
    ];
    for poc in 1..=2 {
        let mut header = Writer::default();
        header.flag(true);
        header.ue(0);
        header.ue(kind);
        header.u(4, poc);
        header.flag(false);
        header.ue(1);
        header.ue(0);
        header.ue(if chain { 0 } else { poc - 1 });
        header.flag(true);
        header.flag(false);
        if kind == 0 {
            header.flag(false);
        }
        header.ue(4);
        header.se(0);
        let mut rbsp = header.rbsp();
        rbsp.extend([0xfe, 0xe0]);
        nals.push(pcm::escaped(1, &rbsp));
    }
    Ok(nals)
}

#[derive(Debug, PartialEq)]
struct Output {
    index: u64,
    poc: i32,
    timestamp: Option<i64>,
    origin: Option<u64>,
    info: PictureInfo,
    planes: [Vec<u16>; 3],
    stats: crate::IntraStats,
    inter: InterStats,
}

fn capture(picture: DecodedPicture<'_>, output: &mut Vec<Output>) {
    let mut stats = picture.stats;
    stats.memory_bytes = 0;
    output.push(Output {
        index: picture.index,
        poc: picture.poc,
        timestamp: picture.timestamp,
        origin: picture.origin,
        info: picture.info,
        planes: picture.planes.map(|plane| plane.samples.to_vec()),
        stats,
        inter: picture.inter,
    });
}

fn decode(
    nals: &[Vec<u8>],
    scheduled: bool,
    limits: DecodeLimits,
    hook: Option<Arc<TestHook>>,
) -> (Result<DecodeSummary>, Vec<Output>) {
    decode_configuration(nals, scheduled, limits, hook, None)
}

fn decode_configuration(
    nals: &[Vec<u8>],
    scheduled: bool,
    limits: DecodeLimits,
    hook: Option<Arc<TestHook>>,
    fault: Option<AdmissionFault>,
) -> (Result<DecodeSummary>, Vec<Output>) {
    let mut decoder = test_decoder(limits);
    let mut output = Vec::new();
    let result = (|| {
        decoder.initialize()?;
        decoder.session[0].pictures.frame.scheduler.hook = hook;
        decoder.session[0].pictures.frame.scheduler.admission_fault = fault;
        for (index, nal) in nals.iter().enumerate() {
            let mut emit = |picture: DecodedPicture<'_>, _: &mut OutputMemory<'_>| {
                capture(picture, &mut output);
                Ok(())
            };
            if scheduled {
                decoder.read_nal_scheduled(
                    nal,
                    Some(index as i64),
                    Some(index as u64),
                    &mut emit,
                )?;
            } else {
                decoder.read_nal_with_origin(nal, Some(index as i64), index as u64, &mut emit)?;
            }
            decoder.set_external_memory(0)?;
        }
        decoder.finish(|picture| {
            capture(picture, &mut output);
            Ok(())
        })
    })();
    (result, output)
}

fn test_decoder(limits: DecodeLimits) -> PictureDecoder {
    let mut decoder = PictureDecoder::new(limits);
    decoder.limits.threads = limits.threads.clamp(1, 16);
    decoder
}

fn semantic(mut summary: DecodeSummary) -> DecodeSummary {
    summary.parallel_pictures = 0;
    summary.frame_parallel_pictures = 0;
    summary.max_workers = 0;
    summary.memory_bytes = 0;
    summary.reconstruction.memory_bytes = 0;
    summary
}

#[test]
fn shared_anchor_siblings_enter_actual_scalar_prediction_before_either_completes() -> Result<()> {
    let nals = stream(false)?;
    let limits = DecodeLimits {
        threads: 2,
        ..DecodeLimits::default()
    };
    let serial = decode(&nals, false, limits, None);
    let hook = Arc::new(TestHook {
        overlap: true,
        ..TestHook::default()
    });
    let parallel = decode(&nals, true, limits, Some(Arc::clone(&hook)));
    assert_eq!(parallel.1, serial.1);
    let summary = parallel.0?;
    assert_eq!(*hook.entered.lock().unwrap(), 6);
    assert_eq!(
        (
            summary.frame_parallel_pictures,
            summary.parallel_pictures,
            summary.max_workers
        ),
        (2, 2, 2)
    );
    assert_eq!(semantic(summary), semantic(serial.0?));
    assert_eq!(summary.inter.skip_units, 2);
    Ok(())
}

#[cfg(hevc_thread_diagnostics)]
#[test]
fn diagnostics_attribute_real_reference_waits_without_changing_overlap() -> Result<()> {
    for chain in [false, true] {
        crate::thread_diagnostics::take();
        let hook = (!chain).then(|| {
            Arc::new(TestHook {
                overlap: true,
                ..TestHook::default()
            })
        });
        let limits = DecodeLimits {
            threads: 2,
            ..DecodeLimits::default()
        };
        let result = decode(&stream(chain)?, true, limits, hook);
        let summary = result.0?;
        let counters = crate::thread_diagnostics::take();
        let count = |key| {
            counters
                .iter()
                .filter(|(name, _)| *name == key)
                .map(|(_, count)| *count)
                .sum::<u64>()
        };
        assert_eq!(count("event_picture_i"), 1);
        assert_eq!(count("event_picture_p"), 2);
        assert_eq!(count("event_parameter_set"), 3);
        assert_eq!(count("event_queue_admitted"), 3);
        assert_eq!(
            count("barrier_reference_unavailable"),
            if chain { 2 } else { 1 }
        );
        assert_eq!(count("barrier_output_reorder"), 0);
        assert_eq!(count("barrier_output_latency"), 0);
        assert_eq!(summary.frame_parallel_pictures, if chain { 0 } else { 2 });
    }
    Ok(())
}

#[test]
fn picture_budget_caps_entropy_reservations_without_losing_overlap_or_error_order() -> Result<()> {
    let nals = stream(false)?;
    let limits = DecodeLimits {
        threads: 2,
        ..DecodeLimits::default()
    };
    let serial = decode(&nals, false, limits, None);
    let picture_bins = serial
        .1
        .iter()
        .map(|picture| picture.stats.bins)
        .max()
        .unwrap();
    assert!(picture_bins > 0);
    assert!(
        nals[3..]
            .iter()
            .all(|nal| Cabac::max_bins(nal.len()) > picture_bins)
    );
    let limits = DecodeLimits {
        max_picture_bins: picture_bins,
        ..limits
    };
    let hook = Arc::new(TestHook {
        overlap: true,
        ..TestHook::default()
    });
    let parallel = decode(&nals, true, limits, Some(Arc::clone(&hook)));
    assert_eq!(parallel.1, serial.1);
    let summary = parallel.0?;
    assert_eq!(*hook.entered.lock().unwrap(), 6);
    assert_eq!(summary.frame_parallel_pictures, 2);
    assert_eq!(semantic(summary), semantic(serial.0?));
    for threads in [1, 2, 4, 8] {
        for stream_bins in [picture_bins - 1, picture_bins, picture_bins + 1, 1 << 20] {
            for max_picture_bins in [0, 1, picture_bins - 1, picture_bins, picture_bins + 1] {
                let limits = DecodeLimits {
                    threads,
                    max_stream_bins: stream_bins,
                    max_picture_bins,
                    ..limits
                };
                let serial = decode(&nals, false, limits, None);
                let parallel = decode(&nals, true, limits, None);
                assert_eq!(parallel.1, serial.1, "{limits:?}");
                assert_eq!(
                    parallel.0.map(semantic),
                    serial.0.map(semantic),
                    "{limits:?}"
                );
            }
        }
    }
    Ok(())
}

#[cfg(hevc_thread_diagnostics)]
#[test]
fn diagnostics_count_output_bumps_that_reconcile_at_picture_sealing() -> Result<()> {
    let nals = stream_kind_reorder(false, 1, 0)?;
    let limits = DecodeLimits {
        threads: 2,
        ..DecodeLimits::default()
    };
    let serial = decode(&nals, false, limits, None);
    crate::thread_diagnostics::take();
    let parallel = decode(&nals, true, limits, None);
    assert_eq!(parallel.1, serial.1);
    let summary = parallel.0?;
    assert_eq!(summary.frame_parallel_pictures, 0);
    assert_eq!(semantic(summary), semantic(serial.0?));
    let counters = crate::thread_diagnostics::take();
    assert_eq!(
        counters
            .iter()
            .find(|(name, _)| *name == "barrier_output_reorder")
            .unwrap()
            .1,
        3
    );
    Ok(())
}

#[cfg(hevc_thread_diagnostics)]
#[test]
fn header_only_dependency_replay_distinguishes_chains_and_siblings() -> Result<()> {
    for (chain, reorder) in [(false, 0), (false, 3), (true, 0), (true, 3)] {
        let mut parser = crate::HeaderParser::new(crate::Limits::default());
        let mut references = Vec::new();
        for nal in stream_kind_reorder(chain, 1, reorder)? {
            if matches!(
                parser.read_nal(&nal, |_| Ok(()))?,
                crate::NalInfo::Slice { .. }
            ) {
                let snapshot = parser.diagnostic_dependencies()?;
                references.push(
                    snapshot
                        .active
                        .into_iter()
                        .flatten()
                        .map(|value| value.0)
                        .collect::<Vec<_>>(),
                );
            }
        }
        assert_eq!(references, [vec![], vec![0], vec![u64::from(chain)]]);
        assert_eq!(parser.finish(|_| Ok(()))?.pictures, 3);
    }
    Ok(())
}

#[test]
fn strict_p_chain_waits_for_complete_generation_identified_references() -> Result<()> {
    let nals = stream(true)?;
    let limits = DecodeLimits {
        threads: 2,
        ..DecodeLimits::default()
    };
    let serial = decode(&nals, false, limits, None);
    let hook = Arc::new(TestHook::default());
    let parallel = decode(&nals, true, limits, Some(Arc::clone(&hook)));
    assert_eq!(parallel.1, serial.1);
    let summary = parallel.0?;
    assert_eq!(*hook.entered.lock().unwrap(), 6);
    assert_eq!(
        (
            summary.frame_parallel_pictures,
            summary.parallel_pictures,
            summary.max_workers
        ),
        (0, 0, 1)
    );
    assert_eq!(semantic(summary), semantic(serial.0?));
    Ok(())
}

#[cfg(hevc_thread_diagnostics)]
#[test]
fn header_envelope_uses_the_activated_sps_ordering_limits() -> Result<()> {
    let mut parser = crate::HeaderParser::new(crate::Limits::default());
    assert_eq!(
        parser.diagnostic_sps_envelope(),
        Err(Error::InvalidData("dependency_picture"))
    );
    let mut pictures = 0;
    for nal in stream(false)? {
        if let crate::NalInfo::Slice {
            header, picture, ..
        } = parser.read_nal(&nal, |_| Ok(()))?
        {
            if header.first {
                assert_eq!(parser.diagnostic_sps_envelope()?, (4, 0));
                assert_eq!(
                    (picture.width, picture.height, picture.bit_depth),
                    (8, 8, 8)
                );
                pictures += 1;
            }
        }
    }
    assert_eq!(pictures, 3);
    Ok(())
}

#[cfg(hevc_thread_diagnostics)]
#[test]
fn row_dependency_trace_refuses_parallel_decode_and_observed_workers() -> Result<()> {
    crate::thread_diagnostics::begin_rows()?;
    let mut decoder = test_decoder(DecodeLimits {
        threads: 2,
        ..DecodeLimits::default()
    });
    assert_eq!(
        decoder.read_nal(&[], None, |_| Ok(())),
        Err(Error::Unsupported("row_trace_workers"))
    );
    assert!(matches!(
        crate::thread_diagnostics::take_rows(1),
        Err(Error::Unsupported("row_trace_workers"))
    ));
    crate::thread_diagnostics::begin_rows()?;
    assert!(matches!(
        crate::thread_diagnostics::take_rows(2),
        Err(Error::Unsupported("row_trace_workers"))
    ));
    crate::thread_diagnostics::begin_rows()?;
    assert!(crate::thread_diagnostics::take_rows(1)?.is_empty());
    Ok(())
}

#[cfg(hevc_thread_diagnostics)]
#[test]
fn row_dependency_trace_observes_generated_predictions() -> Result<()> {
    let nals = stream(false)?;
    let baseline = decode(&nals, false, DecodeLimits::default(), None);
    crate::thread_diagnostics::begin_rows()?;
    let observed = decode(&nals, false, DecodeLimits::default(), None);
    let rows = crate::thread_diagnostics::take_rows(
        observed.0.as_ref().map_or(0, |summary| summary.max_workers),
    )?;
    assert_eq!(baseline.0.map(semantic), observed.0.map(semantic));
    assert_eq!(baseline.1, observed.1);
    assert_eq!(rows.len(), 3);
    for (index, picture) in rows.into_iter().enumerate() {
        assert_eq!(picture.index, index as u64);
        assert_eq!(picture.rows.len(), 1);
        let row = picture.rows.first().ok_or(Error::Internal("row_test"))?;
        if index == 0 {
            assert_eq!(row.sample_calls, 0);
            assert!(row.references.iter().all(Option::is_none));
        } else {
            assert!(row.sample_calls >= 3);
            let references: Vec<_> = row.references.iter().flatten().collect();
            assert_eq!(references.len(), 1);
            assert_eq!(
                (
                    references[0].index,
                    references[0].poc,
                    references[0].sample_row
                ),
                (0, 0, Some(0))
            );
        }
    }
    Ok(())
}

#[cfg(hevc_thread_diagnostics)]
#[test]
fn header_output_fences_match_serial_calls_across_reorder_and_sequence_end() -> Result<()> {
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(header_output_fences_match_serial_calls_inner)
        .expect("test thread starts")
        .join()
        .expect("test thread completes")
}

#[cfg(hevc_thread_diagnostics)]
fn header_output_fences_match_serial_calls_inner() -> Result<()> {
    for reorder in [0, 1, 3] {
        for aud in [false, true] {
            let base = stream_kind_reorder(false, 1, reorder)?;
            let mut nals = base.clone();
            if aud {
                nals.insert(5, vec![70, 1, 0x10]);
            }
            nals.push(vec![72, 1]);
            nals.extend(base);
            let mut parser = crate::HeaderParser::new(crate::Limits::default());
            parser.diagnostic_trace_outputs()?;
            let mut decoder = PictureDecoder::new(DecodeLimits::default());
            let mut serial = Vec::new();
            let mut count = 0;
            for nal in nals {
                decoder.read_nal(&nal, None, |picture| {
                    serial.push((picture.index, picture.poc, count));
                    Ok(())
                })?;
                if matches!(
                    parser.read_nal(&nal, |_| Ok(()))?,
                    crate::NalInfo::Slice { header, .. } if header.first
                ) {
                    count += 1;
                }
            }
            decoder.finish(|picture| {
                serial.push((picture.index, picture.poc, count));
                Ok(())
            })?;
            parser.finish(|_| Ok(()))?;
            let observed: Vec<_> = parser
                .diagnostic_outputs()?
                .into_iter()
                .flatten()
                .map(|output| {
                    (
                        output.event.picture_index,
                        output.event.poc,
                        output.before_picture,
                    )
                })
                .collect();
            assert_eq!(observed, serial, "reorder={reorder}, aud={aud}");
            assert_eq!(observed.len(), 6);
        }
    }
    Ok(())
}

#[test]
fn bump_after_failed_in_flight_job_keeps_serial_output_prefix() -> Result<()> {
    let limits = DecodeLimits {
        threads: 2,
        ..DecodeLimits::default()
    };
    let mut nals = stream(false)?;
    let mut header = Writer::default();
    header.flag(true);
    header.ue(0);
    header.ue(1);
    header.u(4, 3);
    header.flag(false);
    header.ue(1);
    header.ue(0);
    header.ue(2);
    header.flag(true);
    header.flag(false);
    header.ue(4);
    header.se(0);
    let mut rbsp = header.rbsp();
    rbsp.extend([0xfe, 0xe0]);
    nals.push(pcm::escaped(1, &rbsp));
    let serial = decode(&nals, false, limits, None);
    let scheduled = decode(&nals, true, limits, None);
    assert_eq!(scheduled.1, serial.1);
    assert_eq!(semantic(scheduled.0?), semantic(serial.0?));

    nals[4].pop();
    let serial = decode(&nals, false, limits, None);
    let scheduled = decode(&nals, true, limits, None);
    assert!(serial.0.is_err());
    assert!(serial.1.is_empty());
    assert_eq!(scheduled.1, serial.1);
    assert_eq!(scheduled.0.map(semantic), serial.0.map(semantic));
    Ok(())
}

#[test]
fn independent_cold_memory_and_entropy_boundaries_keep_serial_admission() -> Result<()> {
    let nals = stream(false)?;
    let mut low = 0;
    let mut high = 4 * 1024 * 1024;
    while low + 1 < high {
        let middle = low + (high - low) / 2;
        let limits = DecodeLimits {
            threads: 2,
            max_memory_bytes: middle,
            ..DecodeLimits::default()
        };
        if decode(&nals, false, limits, None).0.is_ok() {
            high = middle;
        } else {
            low = middle;
        }
    }
    for memory in [
        low,
        high,
        high + 1,
        high + STACK_BYTES,
        high + 2 * STACK_BYTES,
        8 * 1024 * 1024,
    ] {
        let limits = DecodeLimits {
            threads: 2,
            max_memory_bytes: memory,
            ..DecodeLimits::default()
        };
        let serial = decode(&nals, false, limits, None);
        let scheduled = decode(&nals, true, limits, None);
        assert_eq!(scheduled.1, serial.1, "memory={memory}");
        assert_eq!(
            scheduled.0.map(semantic),
            serial.0.map(semantic),
            "memory={memory}"
        );
    }
    for stream_bins in [0, 1, 2, 3, 4, 5, 6, 7, 8, 4095, 4096, 8192, 1 << 20] {
        for picture_bins in [0, 1, 2, 3, 4, 1 << 20] {
            let limits = DecodeLimits {
                threads: 2,
                max_stream_bins: stream_bins,
                max_picture_bins: picture_bins,
                ..DecodeLimits::default()
            };
            let serial = decode(&nals, false, limits, None);
            let scheduled = decode(&nals, true, limits, None);
            assert_eq!(
                scheduled.1, serial.1,
                "stream={stream_bins} picture={picture_bins}"
            );
            assert_eq!(
                scheduled.0.map(semantic),
                serial.0.map(semantic),
                "stream={stream_bins} picture={picture_bins}"
            );
        }
    }
    Ok(())
}

#[test]
fn earlier_slice_errors_precede_later_headers_without_early_boundary_finalization() -> Result<()> {
    let limits = DecodeLimits {
        threads: 2,
        ..DecodeLimits::default()
    };
    for damaged in [3, 4, 5] {
        for bad_header in [false, true] {
            let mut nals = stream(false)?;
            nals[damaged].pop();
            if bad_header {
                nals.push(vec![2, 1, 0x80]);
            }
            let serial = decode(&nals, false, limits, None);
            let scheduled = decode(&nals, true, limits, None);
            assert!(serial.0.is_err());
            assert_eq!(
                scheduled, serial,
                "damaged={damaged} bad_header={bad_header}"
            );
        }
    }
    let mut nals = stream(false)?;
    nals.push(vec![2, 1, 0x80]);
    assert_eq!(
        decode(&nals, true, limits, None),
        decode(&nals, false, limits, None)
    );
    let mut nals = stream(false)?;
    nals[4].extend([0x12, 0x34]);
    nals[5].pop();
    nals.push(vec![2, 1, 0x80]);
    assert_eq!(
        decode(&nals, true, limits, None),
        decode(&nals, false, limits, None)
    );
    Ok(())
}

#[test]
fn reverse_parallel_errors_and_panics_join_all_started_jobs() -> Result<()> {
    let nals = stream(false)?;
    for fault in [
        TestFault::ReverseErrors,
        TestFault::Panic(1),
        TestFault::Panic(2),
        TestFault::Deadline,
    ] {
        let expected = if matches!(fault, TestFault::ReverseErrors) {
            Error::InvalidData("earlier_picture_job")
        } else if matches!(fault, TestFault::Deadline) {
            Error::Limit("decode_deadline")
        } else {
            Error::Internal("reconstruction_panic")
        };
        let hook = Arc::new(TestHook {
            overlap: true,
            fault: Some(fault),
            ..TestHook::default()
        });
        let (result, _) = decode(
            &nals,
            true,
            DecodeLimits {
                threads: 2,
                ..DecodeLimits::default()
            },
            Some(Arc::clone(&hook)),
        );
        assert_eq!(result, Err(expected));
        assert_eq!(hook.completed.load(Ordering::Acquire) & 6, 6);
        assert_eq!(Arc::strong_count(&hook), 1);
    }
    Ok(())
}

fn suffix(planes: &[Vec<u16>; 3], bad: bool) -> Vec<u8> {
    use md5::{Digest, Md5};
    let mut rbsp = vec![132, 49, 0];
    for plane in planes {
        let mut hash = Md5::new();
        for sample in plane {
            hash.update([*sample as u8]);
        }
        rbsp.extend(hash.finalize());
    }
    if bad {
        rbsp[3] ^= 1;
    }
    rbsp.push(0x80);
    pcm::escaped(40, &rbsp)
}

#[test]
fn suffix_hashes_and_sequence_transitions_keep_the_same_output_prefix() -> Result<()> {
    let limits = DecodeLimits {
        threads: 2,
        ..DecodeLimits::default()
    };
    for bad in [false, true] {
        let mut nals = stream(false)?;
        let serial = decode(&nals, false, limits, None);
        let hash = suffix(&serial.1[0].planes, bad);
        nals.insert(4, hash.clone());
        nals.insert(6, hash.clone());
        nals.push(hash);
        let serial = decode(&nals, false, limits, None);
        let scheduled = decode(&nals, true, limits, None);
        assert_eq!(scheduled.1, serial.1);
        assert_eq!(scheduled.0.map(semantic), serial.0.map(semantic));
    }
    for end in [36, 37] {
        let mut nals = stream(false)?;
        nals.push(vec![end << 1, 1]);
        if end == 36 {
            nals.extend(stream(false)?);
        }
        let serial = decode(&nals, false, limits, None);
        let scheduled = decode(&nals, true, limits, None);
        assert_eq!(scheduled.1, serial.1);
        assert_eq!(scheduled.0.map(semantic), serial.0.map(semantic));
    }
    Ok(())
}

#[test]
fn per_picture_sei_keeps_real_overlap_and_ordered_hash_failures() -> Result<()> {
    let limits = DecodeLimits {
        threads: 2,
        ..DecodeLimits::default()
    };
    let base = stream(false)?;
    let reference = decode(&base, false, limits, None);
    let hash = suffix(&reference.1[0].planes, false);
    let mut nals = base[..3].to_vec();
    for nal in &base[3..] {
        nals.push(pcm::escaped(39, &[1, 0, 0x80]));
        nals.push(nal.clone());
        nals.push(pcm::escaped(40, &[5, 0, 0x80]));
        nals.push(hash.clone());
    }
    let serial = decode(&nals, false, limits, None);
    let hook = Arc::new(TestHook {
        overlap: true,
        ..TestHook::default()
    });
    let scheduled = decode(&nals, true, limits, Some(Arc::clone(&hook)));
    assert_eq!(scheduled.1, serial.1);
    let summary = scheduled.0?;
    assert_eq!(summary.frame_parallel_pictures, 2);
    assert_eq!(*hook.entered.lock().unwrap(), 6);
    assert_eq!(semantic(summary), semantic(serial.0?));
    for slot in [6, 10, 14] {
        let mut invalid = nals.clone();
        invalid[slot] = suffix(&reference.1[0].planes, true);
        let serial = decode(&invalid, false, limits, None);
        let scheduled = decode(&invalid, true, limits, None);
        assert!(serial.0.is_err());
        assert_eq!(scheduled.1, serial.1);
        assert_eq!(scheduled.0.map(semantic), serial.0.map(semantic));
    }
    for extra in [
        hash,
        pcm::escaped(40, &[132, 1, 3, 0x80]),
        pcm::escaped(40, &[5, 3, 0x80]),
    ] {
        let mut invalid = nals.clone();
        invalid.insert(11, extra);
        let serial = decode(&invalid, false, limits, None);
        let scheduled = decode(&invalid, true, limits, None);
        assert!(serial.0.is_err());
        assert_eq!(scheduled.1, serial.1);
        assert_eq!(scheduled.0.map(semantic), serial.0.map(semantic));
    }
    let mut invalid = nals;
    invalid[8].pop();
    invalid.push(pcm::escaped(40, &[5, 3, 0x80]));
    let serial = decode(&invalid, false, limits, None);
    let scheduled = decode(&invalid, true, limits, None);
    assert!(serial.0.is_err());
    assert_eq!(scheduled.1, serial.1);
    assert_eq!(scheduled.0.map(semantic), serial.0.map(semantic));
    Ok(())
}

fn two_row_stream(wpp: bool, complete: bool) -> Vec<Vec<u8>> {
    let mut vps = Writer::default();
    vps.u(4, 0);
    vps.flag(true);
    vps.flag(true);
    vps.u(6, 0);
    vps.u(3, 0);
    vps.flag(true);
    vps.u(16, 0xffff);
    profile(&mut vps);
    vps.flag(false);
    for value in [3, 3, 0] {
        vps.ue(value);
    }
    vps.u(6, 0);
    vps.ue(0);
    vps.flag(false);
    vps.flag(false);
    let mut sps = Writer::default();
    sps.u(4, 0);
    sps.u(3, 0);
    sps.flag(true);
    profile(&mut sps);
    for value in [0, 1, 16, 32] {
        sps.ue(value);
    }
    sps.flag(false);
    sps.ue(0);
    sps.ue(0);
    sps.ue(0);
    sps.flag(false);
    for value in [3, 3, 0, 1, 0, 0, 1, 0, 0] {
        sps.ue(value);
    }
    for flag in [false, false, false, true] {
        sps.flag(flag);
    }
    sps.u(4, 7);
    sps.u(4, 7);
    sps.ue(1);
    sps.ue(0);
    sps.flag(true);
    sps.ue(0);
    for _ in 0..5 {
        sps.flag(false);
    }
    let mut pps = Writer::default();
    pps.ue(0);
    pps.ue(0);
    pps.flag(false);
    pps.flag(false);
    pps.u(3, 0);
    pps.flag(false);
    pps.flag(false);
    pps.ue(0);
    pps.ue(0);
    pps.se(0);
    for _ in 0..3 {
        pps.flag(false);
    }
    pps.se(0);
    pps.se(0);
    for flag in [false, false, false, false, false, wpp, false] {
        pps.flag(flag);
    }
    pps.flag(true);
    pps.flag(false);
    pps.flag(true);
    pps.flag(false);
    pps.flag(false);
    pps.ue(0);
    pps.flag(false);
    pps.flag(false);
    let mut header = Writer::default();
    header.flag(true);
    header.flag(false);
    header.ue(0);
    header.ue(2);
    header.se(0);
    if wpp {
        header.ue(u32::from(complete));
        if complete {
            header.ue(8);
            header.u(9, 387);
        }
    }
    let mut rbsp = header.rbsp();
    rbsp.extend([0x86, 0x80]);
    rbsp.extend(core::iter::repeat_n(34, 384));
    rbsp.extend([if complete { 0xfd } else { 0xfe }, 0x80]);
    if complete {
        rbsp.extend([0x86, 0x80]);
        rbsp.extend(core::iter::repeat_n(51, 384));
        rbsp.extend([0xfe, 0x80]);
    }
    vec![
        pcm::escaped(32, &vps.rbsp()),
        pcm::escaped(33, &sps.rbsp()),
        pcm::escaped(34, &pps.rbsp()),
        pcm::escaped(19, &rbsp),
    ]
}

#[test]
fn session_synchronization_and_polling_preserve_warm_wavefront_workers() -> Result<()> {
    let nals = two_row_stream(true, true);
    for scheduled in [false, true] {
        let mut decoder = test_decoder(DecodeLimits {
            threads: 2,
            ..DecodeLimits::default()
        });
        for nal in &nals {
            if scheduled {
                decoder.read_nal_scheduled(nal, None, None, |_, _| Ok(()))?;
            } else {
                decoder.read_nal(nal, None, |_| Ok(()))?;
            }
            decoder.set_external_memory(0)?;
        }
        let before = decoder.session[0].pictures.frame.cache.hits();
        assert!(
            decoder.session[0]
                .pictures
                .frame
                .cache
                .retained_bytes(0, usize::MAX)?
                > 0
        );
        if scheduled {
            decoder.read_nal_scheduled(&nals[3], None, None, |_, _| Ok(()))?;
        } else {
            decoder.read_nal(&nals[3], None, |_| Ok(()))?;
        }
        assert_eq!(decoder.session[0].pictures.frame.cache.hits(), before + 1);
        let summary = decoder.finish(|_| Ok(()))?;
        assert_eq!(
            (
                summary.parallel_pictures,
                summary.frame_parallel_pictures,
                summary.max_workers
            ),
            (2, 0, 2)
        );
    }
    Ok(())
}

#[test]
fn changing_worker_count_joins_without_committing_an_ordered_pending_error() -> Result<()> {
    let mut nals = stream(false)?;
    nals[4].extend([0x12, 0x34]);
    let limits = DecodeLimits {
        threads: 2,
        ..DecodeLimits::default()
    };
    let expected = decode(&nals, false, limits, None).0.unwrap_err();
    let mut decoder = test_decoder(limits);
    for nal in &nals {
        decoder.read_nal_scheduled(nal, None, None, |_, _| Ok(()))?;
    }
    decoder = decoder.with_threads(1);
    assert!(
        decoder.session[0]
            .pictures
            .frame
            .scheduler
            .jobs
            .iter()
            .all(|job| job.handle.is_none())
    );
    assert_eq!(decoder.finish(|_| Ok(())), Err(expected));
    assert!(decoder.session.is_empty());
    Ok(())
}

#[test]
fn external_overflow_and_caller_validation_preserve_the_first_pending_slice_error() -> Result<()> {
    let mut nals = stream(false)?;
    nals.truncate(4);
    nals[3].pop();
    let limits = DecodeLimits {
        threads: 2,
        ..DecodeLimits::default()
    };
    let expected = decode(&nals, false, limits, None).0.unwrap_err();
    for external in [false, true] {
        let mut decoder = test_decoder(limits);
        for nal in &nals {
            decoder.read_nal_scheduled(nal, None, None, |_, _| Ok(()))?;
        }
        let result = if external {
            decoder.set_external_memory(usize::MAX)
        } else {
            decoder.synchronize_pending()
        };
        assert_eq!(result, Err(expected));
        assert!(decoder.session.is_empty());
    }
    Ok(())
}

#[test]
fn cold_layout_receipt_uses_the_original_typed_allowance() -> Result<()> {
    let decoder = PictureDecoder::new(DecodeLimits::default());
    println!(
        "fixed={} summary={} serial_summary={} dpb={} core={} serial_session={} frame={} slots={}",
        decoder.fixed_memory()?,
        core::mem::size_of::<DecodeSummary>(),
        core::mem::size_of::<SerialAccounting>(),
        core::mem::size_of::<Dpb>(),
        core::mem::size_of::<StreamCore>(),
        core::mem::size_of::<Session<Option<Frame>, SerialAccounting>>(),
        core::mem::size_of::<Frame>(),
        core::mem::size_of::<Slots>()
    );
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    assert_eq!(decoder.fixed_memory()?, 634320);
    Ok(())
}

#[test]
fn next_header_validation_precedes_prior_incomplete_picture_finalization() {
    let limits = DecodeLimits {
        threads: 2,
        ..DecodeLimits::default()
    };
    for bad_header in [false, true] {
        let mut nals = two_row_stream(false, false);
        nals.push(if bad_header {
            vec![2, 1, 0x80]
        } else {
            nals[3].clone()
        });
        let serial = decode(&nals, false, limits, None);
        let scheduled = decode(&nals, true, limits, None);
        assert!(serial.0.is_err());
        if bad_header {
            assert_ne!(
                serial.0,
                Err(Error::InvalidData("selected_picture_incomplete"))
            );
        } else {
            assert_eq!(
                serial.0,
                Err(Error::InvalidData("selected_picture_incomplete"))
            );
        }
        assert_eq!(scheduled, serial);
    }
}

#[test]
fn biprediction_can_read_one_pinned_publication_twice_without_a_reference_lock() -> Result<()> {
    let nals = stream_kind(false, 0)?;
    let limits = DecodeLimits {
        threads: 2,
        ..DecodeLimits::default()
    };
    let serial = decode(&nals, false, limits, None);
    let hook = Arc::new(TestHook {
        overlap: true,
        ..TestHook::default()
    });
    let scheduled = decode(&nals, true, limits, Some(hook));
    assert_eq!(scheduled.1, serial.1);
    let summary = scheduled.0?;
    assert_eq!(
        (summary.inter.bi_units, summary.frame_parallel_pictures),
        (2, 2)
    );
    assert_eq!(semantic(summary), semantic(serial.0?));
    Ok(())
}

#[test]
fn optional_input_snapshot_record_and_start_refusals_keep_serial_results() -> Result<()> {
    let nals = stream(false)?;
    let limits = DecodeLimits {
        threads: 3,
        ..DecodeLimits::default()
    };
    let serial = decode(&nals, false, limits, None);
    for fault in [
        AdmissionFault::Input,
        AdmissionFault::Snapshot,
        AdmissionFault::Record,
        AdmissionFault::Spawn,
    ] {
        let scheduled = decode_configuration(&nals, true, limits, None, Some(fault));
        assert_eq!(scheduled.1, serial.1);
        assert_eq!(scheduled.0.map(semantic), serial.0.map(semantic));
    }
    let hook = Arc::new(TestHook {
        overlap: true,
        ..TestHook::default()
    });
    let partial = decode_configuration(
        &nals,
        true,
        limits,
        Some(Arc::clone(&hook)),
        Some(AdmissionFault::SpawnAfterOne),
    );
    assert_eq!(partial.1, serial.1);
    let summary = partial.0?;
    assert_eq!(
        (summary.frame_parallel_pictures, summary.max_workers),
        (2, 2)
    );
    assert_eq!(semantic(summary), semantic(serial.0?));
    assert_eq!(Arc::strong_count(&hook), 1);
    Ok(())
}

fn reserved_output(
    nals: &[Vec<u8>],
    scheduled: bool,
    bytes: usize,
    hook: Option<Arc<TestHook>>,
) -> (Result<DecodeSummary>, Vec<Output>) {
    let mut decoder = test_decoder(DecodeLimits {
        threads: 2,
        max_memory_bytes: 8 * 1024 * 1024,
        ..DecodeLimits::default()
    });
    let mut output = Vec::new();
    let caller = std::thread::current().id();
    let result = (|| {
        decoder.initialize()?;
        decoder.session[0].pictures.frame.scheduler.hook = hook;
        for nal in nals {
            if scheduled {
                decoder.read_nal_scheduled(nal, None, None, |_, _| {
                    Err(Error::Internal("early_output"))
                })?;
            } else {
                decoder.read_nal(nal, None, |_| Err(Error::Internal("early_output")))?;
            }
            decoder.set_external_memory(0)?;
        }
        decoder.finish_with_output(|picture, memory| {
            assert_eq!(std::thread::current().id(), caller);
            memory.reserve(bytes)?;
            memory.release(bytes)?;
            capture(picture, &mut output);
            Ok(())
        })
    })();
    if result.is_err() {
        assert!(decoder.session.is_empty());
    } else {
        assert_eq!(
            decoder.session[0]
                .pictures
                .frame
                .scheduler
                .bytes(Some(decoder.session[0].core.dpb()))
                .unwrap(),
            0
        );
        assert!(!decoder.session[0].pictures.frame.scheduler.has_input());
    }
    (result, output)
}

#[test]
fn output_reservations_retire_pending_jobs_and_pins_before_cold_admission() -> Result<()> {
    let nals = stream(false)?;
    let mut low = 0;
    let mut high = 8 * 1024 * 1024;
    while low + 1 < high {
        let middle = low + (high - low) / 2;
        if reserved_output(&nals, false, middle, None).0.is_ok() {
            low = middle;
        } else {
            high = middle;
        }
    }
    for bytes in [0, low, high] {
        let serial = reserved_output(&nals, false, bytes, None);
        let hook = Arc::new(TestHook {
            overlap: true,
            ..TestHook::default()
        });
        let scheduled = reserved_output(&nals, true, bytes, Some(Arc::clone(&hook)));
        assert_eq!(scheduled.1, serial.1);
        assert_eq!(scheduled.0.map(semantic), serial.0.map(semantic));
        assert_eq!(hook.completed.load(Ordering::Acquire) & 6, 6);
        assert_eq!(Arc::strong_count(&hook), 1);
    }
    Ok(())
}
