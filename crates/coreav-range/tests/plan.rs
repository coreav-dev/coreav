//! The read plan over a fake host: right bytes, bounded requests, pinned
//! objects, retries and caps. The scenarios are those of the tests of the
//! range reader of the JavaScript package around the WebAssembly module.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    missing_docs
)]

use coreav_range::{Error, Host, HostError, Reader, Request, Response, Settings};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const MIB: u64 = 1 << 20;

fn data(n: u64) -> Vec<u8> {
    (0..n)
        .map(|i| ((i.wrapping_mul(2_654_435_761)) >> 24) as u8)
        .collect()
}

/// A request's record: offset and length.
type Log = Rc<RefCell<Vec<(u64, u64)>>>;

#[derive(Default)]
struct Hooks {
    /// For the n-th request (from 1): the body fails after this many bytes.
    fail: Option<Box<dyn Fn(usize) -> Option<u64>>>,
    /// Every response stops this many bytes short of the requested end.
    short: u64,
    /// The object's validator at the time of each response.
    validator: Option<Rc<RefCell<String>>>,
    /// The response's offset is this much off.
    skew: u64,
    /// The first request fails before its response.
    fail_first: bool,
    /// At most this many bytes per read (a server sending tiny chunks).
    max_read: Option<usize>,
    /// The wait every response reports.
    wait: Option<std::time::Duration>,
}

struct Fake {
    data: Rc<Vec<u8>>,
    log: Log,
    hooks: Hooks,
    open: Rc<Cell<usize>>,
    most: Rc<Cell<usize>>,
    ended: Rc<Cell<usize>>,
}

struct Handle {
    n: usize,
    offset: u64,
    pos: u64,
    end: u64,
    fail_after: Option<u64>,
    failed_head: bool,
    live: bool,
}

impl Fake {
    fn new(data: &Rc<Vec<u8>>) -> Self {
        Self {
            data: Rc::clone(data),
            log: Log::default(),
            hooks: Hooks::default(),
            open: Rc::new(Cell::new(0)),
            most: Rc::new(Cell::new(0)),
            ended: Rc::new(Cell::new(0)),
        }
    }
}

impl Host for Fake {
    type Handle = Handle;

    fn start(&mut self, request: &Request<'_>) -> Handle {
        let size = self.data.len() as u64;
        let offset = if request.suffix {
            size.saturating_sub(request.length)
        } else {
            request.offset
        };
        let request = Request {
            offset,
            ..request.clone()
        };
        self.log.borrow_mut().push((request.offset, request.length));
        let n = self.log.borrow().len();
        self.open.set(self.open.get() + 1);
        self.most.set(self.most.get().max(self.open.get()));
        let asked = (request.offset + request.length).min(size);
        let end = asked
            .saturating_sub(self.hooks.short)
            .max(request.offset + 1)
            .min(size);
        let validator_ok = match (&self.hooks.validator, request.validator) {
            (Some(current), Some(pinned)) => *current.borrow() == pinned,
            _ => true,
        };
        Handle {
            n,
            offset: request.offset,
            pos: request.offset,
            end,
            fail_after: self.hooks.fail.as_ref().and_then(|f| f(n)),
            failed_head: !validator_ok || (self.hooks.fail_first && n == 1),
            live: true,
        }
    }

    fn response(&mut self, handle: &mut Handle) -> Result<Response, HostError> {
        if handle.failed_head {
            if self.hooks.fail_first && handle.n == 1 {
                return Err(HostError::Retry("fetch failed".to_owned()));
            }
            return Err(HostError::Changed("If-Match failed".to_owned()));
        }
        Ok(Response {
            offset: handle.offset + self.hooks.skew,
            end: Some(handle.end),
            size: self.data.len() as u64,
            validator: self.hooks.validator.as_ref().map(|v| v.borrow().clone()),
            whole: false,
            wait: self.hooks.wait,
        })
    }

    fn read(&mut self, handle: &mut Handle, buf: &mut [u8]) -> Result<usize, HostError> {
        if let Some(after) = handle.fail_after {
            if handle.pos - handle.offset >= after {
                return Err(HostError::Retry("connection reset".to_owned()));
            }
        }
        let n = (buf.len() as u64).min(handle.end - handle.pos);
        let n = self.hooks.max_read.map_or(n, |m| n.min(m as u64));
        let n = match handle.fail_after {
            Some(after) => n.min(handle.offset + after - handle.pos),
            None => n,
        } as usize;
        let at = handle.pos as usize;
        buf[..n].copy_from_slice(&self.data[at..at + n]);
        handle.pos += n as u64;
        Ok(n)
    }

    fn cancel(&mut self, handle: Handle) {
        if handle.live {
            self.open.set(self.open.get() - 1);
            self.ended.set(self.ended.get() + 1);
        }
    }
}

fn settings(change: impl FnOnce(&mut Settings)) -> Settings {
    let mut settings = Settings::WORKERS;
    change(&mut settings);
    settings
}

fn read(reader: &mut Reader<Fake>, offset: u64, length: u64) -> Vec<u8> {
    let mut buf = vec![0u8; length as usize];
    reader.read(offset, &mut buf).unwrap();
    buf
}

/// A small deterministic generator.
fn random(seed: u64) -> impl FnMut() -> f64 {
    let mut state = seed;
    move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[test]
fn random_reads_return_the_right_bytes() {
    let bytes = Rc::new(data(3 * MIB + 17));
    let fake = Fake::new(&bytes);
    let log = Rc::clone(&fake.log);
    let mut reader = Reader::open(
        fake,
        settings(|s| {
            s.first_span = 4096;
            s.max_skip = 100_000;
        }),
    )
    .unwrap();
    assert_eq!(reader.size(), bytes.len() as u64);
    let mut next = random(7);
    for i in 0..400 {
        let length = 1 + (next() * 70_000.0) as u64;
        let offset = (next() * (bytes.len() as u64 - length) as f64) as u64;
        assert_eq!(
            read(&mut reader, offset, length),
            &bytes[offset as usize..(offset + length) as usize],
            "read {i} at {offset}"
        );
    }
    assert!(log.borrow().len() > 1);
}

#[test]
fn a_sequential_scan_runs_through_a_pipeline_in_few_requests() {
    let bytes = Rc::new(data(40 * MIB));
    let fake = Fake::new(&bytes);
    let log = Rc::clone(&fake.log);
    let mut reader = Reader::open(fake, settings(|s| s.segment_bytes = 4 * MIB)).unwrap();
    let mut at = 0;
    while at < bytes.len() as u64 {
        let n = 65536.min(bytes.len() as u64 - at);
        assert_eq!(
            read(&mut reader, at, n),
            &bytes[at as usize..(at + n) as usize]
        );
        at += n;
    }
    // The first response (4 MiB), then 9 segments of 4 MiB.
    assert!(log.borrow().len() <= 11, "{} requests", log.borrow().len());
    assert_eq!(reader.stats().pipelines, 1);
}

#[test]
fn the_pipeline_keeps_several_requests_in_flight_within_its_buffer() {
    let bytes = Rc::new(data(64 * MIB));
    let fake = Fake::new(&bytes);
    let most = Rc::clone(&fake.most);
    let mut reader = Reader::open(
        fake,
        settings(|s| {
            s.first_span = MIB;
            s.segment_bytes = MIB;
            s.parallel = 4;
            s.buffer_bytes = 8 * MIB;
        }),
    )
    .unwrap();
    let mut at = 0;
    while at + 65536 <= bytes.len() as u64 {
        assert_eq!(
            read(&mut reader, at, 65536),
            &bytes[at as usize..(at + 65536) as usize]
        );
        at += 65536 + 1000;
    }
    let most = most.get();
    assert!((3..=5).contains(&most), "{most} requests open at once");
}

#[test]
fn advice_starts_the_pipeline_where_the_copy_pass_begins() {
    let bytes = Rc::new(data(32 * MIB));
    let fake = Fake::new(&bytes);
    let log = Rc::clone(&fake.log);
    let mut reader = Reader::open(
        fake,
        settings(|s| {
            s.first_span = MIB;
            s.segment_bytes = 2 * MIB;
        }),
    )
    .unwrap();
    read(&mut reader, 0, 100);
    reader.will_read(20 * MIB, 4 * MIB);
    // The advice alone starts the requests, before any read there.
    assert!(
        log.borrow().iter().any(|(o, _)| *o == 20 * MIB),
        "{:?}",
        log.borrow()
    );
    let before = log.borrow().len();
    let mut at = 20 * MIB;
    while at < 24 * MIB {
        assert_eq!(
            read(&mut reader, at, 65536),
            &bytes[at as usize..(at + 65536) as usize]
        );
        at += 65536;
    }
    // No request beyond those the advice started.
    assert!(
        log.borrow().len() <= before + 1,
        "{} more requests",
        log.borrow().len() - before
    );
}

#[test]
fn gaps_up_to_max_skip_are_read_through_larger_ones_start_a_new_request() {
    let bytes = Rc::new(data(8 * MIB));
    let fake = Fake::new(&bytes);
    let log = Rc::clone(&fake.log);
    let mut reader = Reader::open(
        fake,
        settings(|s| {
            s.first_span = 8 * MIB;
            s.max_skip = MIB;
            s.cache_bytes = 0;
        }),
    )
    .unwrap();
    read(&mut reader, 0, 100);
    read(&mut reader, 512 * 1024, 100);
    assert_eq!(log.borrow().len(), 1);
    read(&mut reader, 4 * MIB, 100);
    assert_eq!(log.borrow().len(), 2);
    assert_eq!(log.borrow()[1].0, 4 * MIB);
    // Backwards always starts again.
    read(&mut reader, 10, 10);
    assert_eq!(log.borrow().len(), 3);
}

#[test]
fn recent_bytes_serve_reads_that_go_back() {
    let bytes = Rc::new(data(8 * MIB));
    let fake = Fake::new(&bytes);
    let log = Rc::clone(&fake.log);
    let mut reader = Reader::open(
        fake,
        settings(|s| {
            s.first_span = 4 * MIB;
            s.max_skip = 0;
            s.cache_bytes = 2 * MIB;
        }),
    )
    .unwrap();
    read(&mut reader, 0, 100);
    read(&mut reader, 100, 600_000);
    assert_eq!(read(&mut reader, 10, 300_000), &bytes[10..300_010]);
    // A read that starts in kept bytes and continues in the open response.
    assert_eq!(
        read(&mut reader, 599_000, 300_000),
        &bytes[599_000..899_000]
    );
    assert_eq!(log.borrow().len(), 1);
    // Older than the cache: a new request.
    read(&mut reader, 6 * MIB, 2 * MIB);
    read(&mut reader, 10, 10);
    assert_eq!(log.borrow().len(), 3);
}

#[test]
fn a_response_stays_open_while_another_position_is_read() {
    let bytes = Rc::new(data(32 * MIB));
    let fake = Fake::new(&bytes);
    let log = Rc::clone(&fake.log);
    let mut reader = Reader::open(
        fake,
        settings(|s| {
            s.first_span = MIB;
            s.max_skip = 4096;
            s.cache_bytes = 0;
        }),
    )
    .unwrap();
    // A movie box at the end: the start, the end, then the start again.
    assert_eq!(read(&mut reader, 0, 64), &bytes[..64]);
    assert_eq!(
        read(&mut reader, 31 * MIB, 4096),
        &bytes[31 * MIB as usize..31 * MIB as usize + 4096]
    );
    assert_eq!(read(&mut reader, 1000, 70_000), &bytes[1000..71_000]);
    assert_eq!(log.borrow().len(), 2);
    // Four positions far apart: at most three responses stay open.
    for at in [8 * MIB, 16 * MIB, 24 * MIB] {
        read(&mut reader, at, 10);
    }
    read(&mut reader, 2000, 10);
    assert_eq!(log.borrow().len(), 6);
}

#[test]
fn the_first_object_seen_is_pinned() {
    let bytes = Rc::new(data(2 * MIB));
    let etag = Rc::new(RefCell::new("v1".to_owned()));
    let mut fake = Fake::new(&bytes);
    fake.hooks.validator = Some(Rc::clone(&etag));
    let mut reader = Reader::open(fake, settings(|s| s.first_span = 1024)).unwrap();
    read(&mut reader, 0, 10);
    *etag.borrow_mut() = "v2".to_owned();
    let mut buf = [0u8; 10];
    assert!(matches!(reader.read(MIB, &mut buf), Err(Error::Changed(_))));
}

#[test]
fn a_body_that_fails_mid_transfer_is_requested_again() {
    let bytes = Rc::new(data(4 * MIB));
    let mut fake = Fake::new(&bytes);
    fake.hooks.fail = Some(Box::new(|n| (n == 2).then_some(70_000)));
    let log = Rc::clone(&fake.log);
    let mut reader = Reader::open(fake, settings(|s| s.first_span = 64 * 1024)).unwrap();
    let mut at = 0;
    while at < 2 * MIB {
        assert_eq!(
            read(&mut reader, at, 50_000),
            &bytes[at as usize..(at + 50_000) as usize]
        );
        at += 50_000;
    }
    assert!(log.borrow().len() >= 3);
}

#[test]
fn retries_are_bounded() {
    let bytes = Rc::new(data(MIB));
    let mut fake = Fake::new(&bytes);
    fake.hooks.fail = Some(Box::new(|n| (n > 1).then_some(0)));
    let mut reader = Reader::open(
        fake,
        settings(|s| {
            s.first_span = 1024;
            s.retries = 2;
        }),
    )
    .unwrap();
    let mut buf = [0u8; 10];
    match reader.read(500_000, &mut buf) {
        Err(Error::Failed(message)) => assert!(message.contains("connection reset"), "{message}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(reader.stats().requests, 4);
}

#[test]
fn the_request_cap_is_enforced_before_the_request() {
    let bytes = Rc::new(data(MIB));
    let fake = Fake::new(&bytes);
    let log = Rc::clone(&fake.log);
    let mut reader = Reader::open(
        fake,
        settings(|s| {
            s.first_span = 1024;
            s.max_skip = 0;
            s.max_requests = 3;
        }),
    )
    .unwrap();
    read(&mut reader, 100_000, 10);
    read(&mut reader, 200_000, 10);
    let mut buf = [0u8; 10];
    assert_eq!(reader.read(300_000, &mut buf), Err(Error::RequestLimit(3)));
    assert_eq!(log.borrow().len(), 3);
}

#[test]
fn a_response_for_the_wrong_offset_is_refused() {
    let bytes = Rc::new(data(MIB));
    let mut fake = Fake::new(&bytes);
    fake.hooks.skew = 1;
    match Reader::open(fake, Settings::WORKERS) {
        Err(Error::Refused(message)) => assert!(message.contains("starts at 1"), "{message}"),
        Err(other) => panic!("{other:?}"),
        Ok(_) => panic!("accepted"),
    }
}

#[test]
fn the_first_request_is_retried_and_every_request_is_ended() {
    let bytes = Rc::new(data(8 * MIB));
    let mut fake = Fake::new(&bytes);
    fake.hooks.fail_first = true;
    let open = Rc::clone(&fake.open);
    let log = Rc::clone(&fake.log);
    let mut reader = Reader::open(
        fake,
        settings(|s| {
            s.first_span = 64 * 1024;
            s.max_skip = 0;
        }),
    )
    .unwrap();
    let mut at = 0;
    while at < 4 * MIB {
        read(&mut reader, at, 65536);
        at += 65536;
    }
    read(&mut reader, 7 * MIB, 10);
    drop(reader);
    assert!(log.borrow().len() > 2);
    assert_eq!(open.get(), 0, "requests left open");
}

#[test]
fn a_response_that_holds_less_than_asked_is_read_as_far_as_it_goes() {
    let bytes = Rc::new(data(2 * MIB));
    let mut fake = Fake::new(&bytes);
    fake.hooks.short = 1000;
    let mut reader = Reader::open(fake, settings(|s| s.first_span = 64 * 1024)).unwrap();
    let mut at = 0;
    while at < MIB {
        assert_eq!(
            read(&mut reader, at, 50_000),
            &bytes[at as usize..(at + 50_000) as usize]
        );
        at += 50_000;
    }
}

#[test]
fn caps_on_bytes_and_reads_past_the_end() {
    let bytes = Rc::new(data(4 * MIB));
    let fake = Fake::new(&bytes);
    let mut reader = Reader::open(
        fake,
        settings(|s| {
            s.first_span = MIB;
            s.max_bytes = 2 * MIB;
        }),
    )
    .unwrap();
    let mut buf = vec![0u8; 3 * MIB as usize];
    assert_eq!(reader.read(0, &mut buf), Err(Error::ByteLimit(2 * MIB)));
    let mut buf = [0u8; 10];
    assert!(matches!(
        reader.read(4 * MIB - 5, &mut buf),
        Err(Error::OutOfRange { .. })
    ));
}

#[test]
fn settings_are_checked() {
    let bytes = Rc::new(data(10));
    assert!(Reader::open(Fake::new(&bytes), settings(|s| s.first_span = 0)).is_err());
    assert!(Reader::open(Fake::new(&bytes), settings(|s| s.parallel = 0)).is_err());
    assert_eq!(Settings::default(), Settings::NATIVE);
}

#[test]
fn a_jump_past_a_response_still_unread_keeps_it_open() {
    // A probe of a small file: its start, an index at its end, its start
    // again. The first response stays open for the third read.
    let bytes = Rc::new(data(6 * MIB));
    let fake = Fake::new(&bytes);
    let log = Rc::clone(&fake.log);
    let mut reader = Reader::open(fake, Settings::WORKERS).unwrap();
    read(&mut reader, 0, 4096);
    read(&mut reader, 6 * MIB - 1000, 963);
    assert_eq!(read(&mut reader, 3972, 4096), &bytes[3972..8068]);
    assert_eq!(log.borrow().len(), 2, "{:?}", log.borrow());
}

#[test]
fn the_tail_is_asked_for_with_the_first_request() {
    // A probe of a file with its index at the end: the start, the end, the
    // start again, in the round trip of the first request.
    let bytes = Rc::new(data(40 * MIB));
    let fake = Fake::new(&bytes);
    let log = Rc::clone(&fake.log);
    let mut reader = Reader::open(fake, settings(|s| s.tail_bytes = 64 * 1024)).unwrap();
    assert_eq!(log.borrow().len(), 2);
    assert_eq!(log.borrow()[0].0, 40 * MIB - 64 * 1024);
    read(&mut reader, 0, 4096);
    assert_eq!(
        read(&mut reader, 40 * MIB - 1000, 963),
        &bytes[(40 * MIB - 1000) as usize..(40 * MIB - 37) as usize]
    );
    assert_eq!(read(&mut reader, 3972, 4096), &bytes[3972..8068]);
    assert_eq!(log.borrow().len(), 2, "{:?}", log.borrow());
    // A small object: the first response holds it all, the tail is dropped.
    let small = Rc::new(data(100_000));
    let fake = Fake::new(&small);
    let open = Rc::clone(&fake.open);
    let mut reader = Reader::open(fake, settings(|s| s.tail_bytes = 64 * 1024)).unwrap();
    assert_eq!(read(&mut reader, 99_000, 1000), &small[99_000..]);
    drop(reader);
    assert_eq!(open.get(), 0);
}

#[test]
fn a_server_sending_tiny_pieces_costs_memory_by_the_bytes_held() {
    // Every read returns at most 3 bytes: the plan must not keep a buffer
    // per read (a server can send its body a byte at a time).
    let bytes = Rc::new(data(3 * MIB));
    let mut fake = Fake::new(&bytes);
    fake.hooks.max_read = Some(3);
    let mut reader = Reader::open(
        fake,
        settings(|s| {
            s.first_span = 256 * 1024;
            s.segment_bytes = 512 * 1024;
            s.cache_bytes = MIB;
        }),
    )
    .unwrap();
    let mut most = 0;
    let mut at = 0;
    while at < 3 * MIB {
        assert_eq!(
            read(&mut reader, at, 65536),
            &bytes[at as usize..(at + 65536) as usize]
        );
        most = most.max(reader.held_bytes());
        at += 65536;
    }
    // The cache (1 MiB) and the pipelines' kept pieces (behind and ahead of
    // the reader), each at most twice the bytes it holds.
    assert!(most < 48 * MIB, "{most} bytes held");
    assert!(reader.stats().pipelines >= 1);
}

/// Regions `count` regions of `len` bytes, one every `every` bytes from
/// `first`.
fn regions(first: u64, every: u64, len: u64, count: u64) -> Vec<(u64, u64)> {
    (0..count).map(|k| (first + k * every, len)).collect()
}

/// Reads every region in order, in pieces of at most 64 KiB, checking the
/// bytes.
fn read_regions(reader: &mut Reader<Fake>, bytes: &[u8], regions: &[(u64, u64)]) {
    for &(offset, len) in regions {
        let mut at = offset;
        while at < offset + len {
            let n = (offset + len - at).min(64 << 10);
            assert_eq!(read(reader, at, n), &bytes[at as usize..(at + n) as usize]);
            at += n;
        }
    }
}

#[test]
fn advised_regions_are_requested_and_the_gaps_between_them_skipped() {
    let bytes = Rc::new(data(80 * MIB));
    let fake = Fake::new(&bytes);
    let log = Rc::clone(&fake.log);
    let most = Rc::clone(&fake.most);
    let mut reader = Reader::open(fake, settings(|_| {})).unwrap();
    let before = log.borrow().len();
    // 15 regions of 100 KiB, 5 MiB apart: gaps over `sparse_gap`.
    let advised = regions(6 * MIB, 5 * MIB, 100 << 10, 15);
    reader.will_read_regions(&advised);
    read_regions(&mut reader, &bytes, &advised);
    let requests = log.borrow()[before..].to_vec();
    // One request per region, each inside its region.
    assert_eq!(requests.len(), advised.len(), "{requests:?}");
    for &(offset, length) in &requests {
        assert!(
            advised
                .iter()
                .any(|&(o, l)| offset >= o && offset + length <= o + l),
            "request {offset}+{length} outside the regions"
        );
    }
    assert!(most.get() <= 5);
    // The end of the advice: reads go anywhere again, as before.
    reader.will_read_regions(&[]);
    assert_eq!(
        read(&mut reader, 3 * MIB + 5, 1000),
        &bytes[3 * MIB as usize + 5..][..1000]
    );
}

#[test]
fn regions_closer_than_the_sparse_gap_are_read_through() {
    let bytes = Rc::new(data(40 * MIB));
    let fake = Fake::new(&bytes);
    let log = Rc::clone(&fake.log);
    let mut reader = Reader::open(fake, settings(|_| {})).unwrap();
    let before = log.borrow().len();
    // 30 regions of 64 KiB, 1 MiB apart: one span, read in 16 MiB segments.
    let advised = regions(5 * MIB, MIB, 64 << 10, 30);
    reader.will_read_regions(&advised);
    read_regions(&mut reader, &bytes, &advised);
    let requests = log.borrow()[before..].to_vec();
    assert!(requests.len() <= 3, "{requests:?}");
    assert_eq!(requests[0].0, 5 * MIB);
}

#[test]
fn a_read_outside_the_regions_asks_for_its_own_bytes_only() {
    let bytes = Rc::new(data(80 * MIB));
    let fake = Fake::new(&bytes);
    let log = Rc::clone(&fake.log);
    let mut reader = Reader::open(fake, settings(|_| {})).unwrap();
    let advised = regions(10 * MIB, 10 * MIB, 64 << 10, 6);
    reader.will_read_regions(&advised);
    read_regions(&mut reader, &bytes, &advised[..2]);
    // Between the regions: a request for exactly the read.
    let before = log.borrow().len();
    assert_eq!(
        read(&mut reader, 25 * MIB, 3000),
        &bytes[25 * MIB as usize..][..3000]
    );
    assert!(log.borrow()[before..].contains(&(25 * MIB, 3000)));
    // The regions go on from where they were.
    read_regions(&mut reader, &bytes, &advised[2..]);
}

#[test]
fn a_read_that_jumps_over_regions_skips_them() {
    let bytes = Rc::new(data(200 * MIB));
    let fake = Fake::new(&bytes);
    let log = Rc::clone(&fake.log);
    let mut reader = Reader::open(fake, settings(|_| {})).unwrap();
    let advised = regions(10 * MIB, 10 * MIB, 64 << 10, 18);
    reader.will_read_regions(&advised);
    read_regions(&mut reader, &bytes, &advised[..1]);
    read_regions(&mut reader, &bytes, &advised[12..]);
    // Requests for the first regions in flight (`parallel`), then from the
    // 13th on; none for the ones in between beyond those.
    let requested: Vec<u64> = log.borrow().iter().map(|r| r.0).collect();
    for skipped in &advised[5..12] {
        assert!(!requested.contains(&skipped.0), "{requested:?}");
    }
}

#[test]
fn regions_fit_half_the_requests_left_by_joining_more() {
    let bytes = Rc::new(data(120 * MIB));
    let fake = Fake::new(&bytes);
    let log = Rc::clone(&fake.log);
    let mut reader = Reader::open(fake, settings(|s| s.max_requests = 30)).unwrap();
    let before = log.borrow().len();
    // 100 regions 1.1 MiB apart would need 100 requests at a 1 MiB gap; the
    // gap doubles until they need at most 14.
    let advised = regions(MIB, 1_100_000, 4096, 100);
    reader.will_read_regions(&advised);
    read_regions(&mut reader, &bytes, &advised);
    assert!(
        log.borrow().len() - before <= 14,
        "{:?}",
        &log.borrow()[before..]
    );
}

#[test]
fn random_regions_and_reads_return_the_right_bytes() {
    let mut next = random(31);
    for round in 0..20 {
        let bytes = Rc::new(data(20 * MIB + (next() * 1000.0) as u64));
        let fake = Fake::new(&bytes);
        let most = Rc::clone(&fake.most);
        let mut reader = Reader::open(
            fake,
            settings(|s| {
                s.sparse_gap = Some((next() * 3.0 * MIB as f64) as u64);
                s.segment_bytes = 1 + (next() * 4.0 * MIB as f64) as u64;
            }),
        )
        .unwrap();
        let size = bytes.len() as u64;
        let mut advised = Vec::new();
        let mut at = MIB + (next() * MIB as f64) as u64;
        while at < size {
            let len = 1 + (next() * 200_000.0) as u64;
            advised.push((at, len.min(size - at)));
            at += len + (next() * 3.0 * MIB as f64) as u64;
        }
        reader.will_read_regions(&advised);
        for &(offset, len) in &advised {
            // Some regions are skipped; some reads go outside.
            if next() < 0.2 {
                continue;
            }
            if next() < 0.1 {
                // A read outside the advice (before its first region), as a
                // writer's reads of other boxes would be.
                let o = (next() * (MIB - 5000) as f64) as u64;
                assert_eq!(read(&mut reader, o, 5000), &bytes[o as usize..][..5000]);
            }
            let start = offset + (next() * len as f64) as u64 / 2;
            let n = (offset + len - start).max(1);
            assert_eq!(
                read(&mut reader, start, n),
                &bytes[start as usize..(start + n) as usize],
                "round {round}"
            );
        }
        reader.will_read_regions(&[]);
        assert!(most.get() <= 6, "round {round}: {} open", most.get());
    }
}

#[test]
fn advice_never_ended_costs_a_later_pass_a_pipeline_not_a_request_per_read() {
    let bytes = Rc::new(data(64 * MIB));
    let fake = Fake::new(&bytes);
    let log = Rc::clone(&fake.log);
    let mut reader = Reader::open(fake, settings(|_| {})).unwrap();
    let advised = regions(8 * MIB, 8 * MIB, 64 << 10, 7);
    reader.will_read_regions(&advised);
    read_regions(&mut reader, &bytes, &advised);
    // The advice is not ended; a pass over every byte follows.
    let before = log.borrow().len();
    let mut at = 0;
    while at < 64 * MIB {
        assert_eq!(
            read(&mut reader, at, 64 << 10),
            &bytes[at as usize..][..64 << 10]
        );
        at += 64 << 10;
    }
    let requests = log.borrow().len() - before;
    assert!(requests <= 16, "{requests} requests for 1,024 reads");
}

#[test]
fn the_sparse_gap_follows_the_first_response_wait() {
    let bytes = Rc::new(data(MIB));
    let gap = |wait: Option<u64>, fixed: Option<u64>| {
        let mut fake = Fake::new(&bytes);
        fake.hooks.wait = wait.map(std::time::Duration::from_millis);
        Reader::open(fake, settings(|s| s.sparse_gap = fixed))
            .unwrap()
            .sparse_gap()
    };
    // 275 ms at 60 MB/s over four requests: about 4 MB.
    assert_eq!(gap(Some(275), None), 4_125_000);
    // Kept within 1 to 8 MiB.
    assert_eq!(gap(Some(20), None), MIB);
    assert_eq!(gap(Some(2_000), None), 8 * MIB);
    // No measurement: 4 MiB. A fixed gap wins.
    assert_eq!(gap(None, None), 4 * MIB);
    assert_eq!(gap(Some(275), Some(12345)), 12345);
}

#[test]
fn regions_an_open_response_holds_need_no_request() {
    let bytes = Rc::new(data(20 * MIB));
    let fake = Fake::new(&bytes);
    let log = Rc::clone(&fake.log);
    // The native preset: the first response asks for 32 MiB, the object.
    let mut reader = Reader::open(fake, Settings::NATIVE).unwrap();
    assert_eq!(read(&mut reader, 0, 100), &bytes[..100]);
    let before = log.borrow().len();
    let advised = regions(MIB, 300 << 10, 64 << 10, 60);
    reader.will_read_regions(&advised);
    read_regions(&mut reader, &bytes, &advised);
    assert_eq!(log.borrow().len(), before, "{:?}", &log.borrow()[before..]);
    // With a 4 MiB first response, the pipeline starts where it ends.
    let fake = Fake::new(&bytes);
    let log = Rc::clone(&fake.log);
    let mut reader = Reader::open(fake, settings(|_| {})).unwrap();
    assert_eq!(read(&mut reader, 0, 100), &bytes[..100]);
    let before = log.borrow().len();
    reader.will_read_regions(&advised);
    read_regions(&mut reader, &bytes, &advised);
    let requests = log.borrow()[before..].to_vec();
    assert!(
        requests.iter().all(|&(offset, _)| offset >= 4 * MIB),
        "{requests:?}"
    );
}

#[test]
fn a_read_past_the_open_response_before_the_pipeline_asks_for_its_own_bytes() {
    let bytes = Rc::new(data(32 * MIB));
    let fake = Fake::new(&bytes);
    let log = Rc::clone(&fake.log);
    let mut reader = Reader::open(fake, settings(|s| s.sparse_gap = Some(MIB))).unwrap();
    assert_eq!(read(&mut reader, 0, 100), &bytes[..100]);
    let before = log.borrow().len();
    // The first two regions lie in the 4 MiB first response; the pipeline
    // starts at the third. The second read runs on past the response.
    let advised: Vec<(u64, u64)> = [
        64 << 10,
        4 * MIB - 8192,
        6 * MIB,
        9 * MIB,
        12 * MIB,
        20 * MIB,
    ]
    .into_iter()
    .map(|at| (at, 4096))
    .collect();
    reader.will_read_regions(&advised);
    for (i, &(at, _)) in advised.iter().enumerate() {
        let n = if i == 1 { 16 << 10 } else { 4096 };
        assert_eq!(
            read(&mut reader, at, n),
            &bytes[at as usize..][..n as usize]
        );
    }
    // Its own bytes past the response, not a pipeline to the end.
    let requested: u64 = log.borrow()[before..].iter().map(|&(_, n)| n).sum();
    assert!(requested < MIB, "{:?}", &log.borrow()[before..]);
}

#[test]
fn advice_an_open_response_serves_whole_ends_the_advice_before() {
    let bytes = Rc::new(data(64 * MIB));
    let fake = Fake::new(&bytes);
    let log = Rc::clone(&fake.log);
    let open = Rc::clone(&fake.open);
    let mut reader = Reader::open(fake, settings(|s| s.sparse_gap = Some(MIB))).unwrap();
    assert_eq!(read(&mut reader, 0, 100), &bytes[..100]);
    reader.will_read_regions(&[(30 * MIB, 4096), (40 * MIB, 4096)]);
    assert!(open.get() > 1);
    // Regions within the first response: its pipeline ends.
    let advised = [(MIB, 4096), (2 * MIB, 4096)];
    reader.will_read_regions(&advised);
    assert_eq!(open.get(), 1);
    let before = log.borrow().len();
    read_regions(&mut reader, &bytes, &advised);
    assert_eq!(log.borrow().len(), before);
}

#[test]
fn a_forward_pass_that_jumps_ahead_and_back_reads_without_retries() {
    let bytes = Rc::new(data(90 * MIB));
    let fake = Fake::new(&bytes);
    let mut reader = Reader::open(fake, settings(|s| s.retries = 0)).unwrap();
    let mut at = 0;
    while at < 5 * MIB {
        assert_eq!(
            read(&mut reader, at, 64 << 10),
            &bytes[at as usize..][..64 << 10]
        );
        at += 64 << 10;
    }
    // Past the pipeline's frontier, within max_skip, then back before it.
    for at in [73 * MIB, 72 * MIB, 74 * MIB] {
        assert_eq!(
            read(&mut reader, at, 64 << 10),
            &bytes[at as usize..][..64 << 10]
        );
    }
}
