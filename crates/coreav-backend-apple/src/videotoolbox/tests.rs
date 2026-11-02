#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;
use std::sync::{Barrier, atomic::AtomicUsize};
use std::time::Duration;

#[path = "../../tests/common/h264.rs"]
mod headers;

static SERIAL: Mutex<()> = Mutex::new(());

#[derive(Clone, Copy)]
enum Mode {
    Immediate,
    Reverse,
    Overlap,
    Silent,
    Duplicate,
    SubmissionFailure,
    CompletionFailure,
    BadTiming,
    UnknownHardware,
    PoolBackpressure,
    Corrupt(u8),
}

struct Fake {
    key: usize,
    mode: Mode,
    pending: Vec<(usize, Time, Time)>,
    closes: Arc<AtomicUsize>,
}

fn sample(pts: Time, duration: Time) -> RawSample {
    let slice = headers::Slice::default().nal();
    RawSample {
        bytes: headers::unit(&[&slice]),
        sps: headers::Sequence {
            vui: true,
            ..Default::default()
        }
        .nal(),
        pps: headers::Picture::default().nal(),
        dimensions: (32, 32),
        pts,
        duration,
        dts: None,
        sync: true,
    }
}

fn emit(key: usize, frame_key: usize, pts: Time, duration: Time) {
    if let Some(lease) = lease_callback(key, frame_key) {
        lease.finish(Ok(sample(pts, duration)));
    }
}

impl Backend for Fake {
    fn identity(&self) -> Result<Identity, Error> {
        Ok(Identity {
            encoder_id: Some("test encoder".into()),
            hardware: if matches!(self.mode, Mode::UnknownHardware) {
                Hardware::Unknown
            } else {
                Hardware::Software
            },
        })
    }
    fn submit(&mut self, _: Frame<'_>, key: usize, pts: Time, duration: Time) -> Result<(), Error> {
        if matches!(self.mode, Mode::PoolBackpressure) {
            return Err(Error::Backpressure);
        }
        match self.mode {
            Mode::Reverse | Mode::Overlap | Mode::Silent | Mode::CompletionFailure => {
                self.pending.push((key, pts, duration));
            }
            Mode::BadTiming => emit(self.key, key, Time::new(999, 30)?, duration),
            Mode::Corrupt(case) => {
                let mut raw = sample(pts, duration);
                match case {
                    0 => raw.bytes.resize(4097, 0),
                    1 => raw.sps.clear(),
                    2 => raw.pps.clear(),
                    3 => raw.dimensions = (64, 32),
                    4 => raw.dts = Some(Time::new(1, 1)?),
                    5 => raw.sync = false,
                    6 => raw.sps = headers::Sequence::default().nal(),
                    _ => raw.bytes.clear(),
                }
                lease_callback(self.key, key).unwrap().finish(Ok(raw));
            }
            _ => emit(self.key, key, pts, duration),
        }
        if matches!(self.mode, Mode::Duplicate) {
            emit(self.key, key, pts, duration);
        }
        if matches!(self.mode, Mode::SubmissionFailure) {
            return Err(Error::Framework {
                operation: Operation::Encode,
                status: -1,
            });
        }
        Ok(())
    }
    fn complete(&mut self, _: Option<Time>) -> Result<(), Error> {
        match self.mode {
            Mode::Reverse => {
                for (key, pts, duration) in self.pending.drain(..).rev() {
                    emit(self.key, key, pts, duration);
                }
            }
            Mode::Overlap => {
                let barrier = Arc::new(Barrier::new(self.pending.len()));
                let session_key = self.key;
                std::thread::scope(|scope| {
                    for (key, pts, duration) in self.pending.drain(..) {
                        let barrier = Arc::clone(&barrier);
                        scope.spawn(move || {
                            let lease = lease_callback(session_key, key);
                            barrier.wait();
                            lease.unwrap().finish(Ok(sample(pts, duration)));
                        });
                    }
                });
            }
            Mode::CompletionFailure => {
                return Err(Error::Framework {
                    operation: Operation::Complete,
                    status: -2,
                });
            }
            _ => {}
        }
        Ok(())
    }
    fn close(&mut self) {
        assert!(lookup(self.key).is_none());
        self.closes.fetch_add(1, Ordering::Relaxed);
    }
}

fn settings() -> Settings {
    Settings::bt709_limited(32, 32, 30, 1, 2_000_000, HardwarePolicy::Any).unwrap()
}

fn limits() -> Limits {
    Limits {
        max_access_unit_bytes: 4096,
        max_configuration_bytes: 4096,
        max_queue_bytes: 16_384,
        ..Limits::default()
    }
}

fn control() -> Control {
    Control::new(Instant::now() + Duration::from_secs(30))
}

fn session(mode: Mode, control: Control, limits: Limits) -> (Session<Fake>, Arc<AtomicUsize>) {
    let closes = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&closes);
    let session = Session::new(settings(), limits, control, |key| {
        Ok(Fake {
            key,
            mode,
            pending: Vec::new(),
            closes: observed,
        })
    })
    .unwrap();
    (session, closes)
}

fn submit(session: &mut Session<Fake>, id: i64) -> Result<(), Error> {
    let bytes = vec![128; 32 * 32 * 3 / 2];
    session.submit(
        Frame::bt709_limited(&bytes, 32, 32)?,
        Time::new(id, 30)?,
        Time::new(1, 30)?,
    )
}

#[test]
fn reentrant_reverse_and_overlapping_callbacks_keep_source_order() {
    let _serial = lock(&SERIAL);
    for mode in [Mode::Immediate, Mode::Reverse, Mode::Overlap] {
        let (mut owner, closes) = session(mode, control(), limits());
        for id in 0..4 {
            submit(&mut owner, id).unwrap();
        }
        owner.start_drain().unwrap();
        for id in 0..4 {
            let packet = owner.pull().unwrap().unwrap();
            assert_eq!(packet.frame_id, id);
            assert_eq!(packet.pts, Time::new(id as i64, 30).unwrap());
        }
        assert!(owner.pull().unwrap().is_none());
        let done = owner.complete().unwrap();
        assert_eq!(done.frames, 4);
        assert_eq!(done.config.color(), Some(h264::Color::BT709_LIMITED));
        assert_eq!(closes.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn queue_backpressure_is_recoverable_and_never_submits_twice() {
    let _serial = lock(&SERIAL);
    let (mut owner, _) = session(
        Mode::Immediate,
        control(),
        Limits {
            max_pending_frames: 1,
            ..limits()
        },
    );
    submit(&mut owner, 0).unwrap();
    assert_eq!(submit(&mut owner, 1), Err(Error::Backpressure));
    assert_eq!(owner.pull().unwrap().unwrap().frame_id, 0);
    submit(&mut owner, 1).unwrap();
    assert_eq!(owner.pull().unwrap().unwrap().frame_id, 1);
    owner.start_drain().unwrap();
    assert!(owner.pull().unwrap().is_none());
    assert_eq!(owner.complete().unwrap().frames, 2);
}

#[test]
fn a_pool_limit_after_reservation_is_terminal_not_a_phantom_pending_frame() {
    let _serial = lock(&SERIAL);
    let (mut owner, closes) = session(Mode::PoolBackpressure, control(), limits());
    assert_eq!(submit(&mut owner, 0), Err(Error::Limit));
    assert_eq!(owner.pull().unwrap_err(), Error::Limit);
    assert_eq!(submit(&mut owner, 0), Err(Error::Limit));
    assert_eq!(owner.complete().unwrap_err(), Error::Limit);
    assert_eq!(closes.load(Ordering::Relaxed), 1);
}

#[test]
fn callback_and_native_failures_are_sticky_even_after_a_reentrant_success() {
    let _serial = lock(&SERIAL);
    for (mode, error) in [
        (Mode::Duplicate, Error::Output),
        (Mode::BadTiming, Error::Output),
        (
            Mode::SubmissionFailure,
            Error::Framework {
                operation: Operation::Encode,
                status: -1,
            },
        ),
    ] {
        let (mut owner, closes) = session(mode, control(), limits());
        assert_eq!(submit(&mut owner, 0), Err(error));
        assert_eq!(owner.pull().unwrap_err(), error);
        assert_eq!(owner.start_drain(), Err(error));
        drop(owner);
        assert_eq!(closes.load(Ordering::Relaxed), 1);
    }
    let (mut owner, _) = session(Mode::CompletionFailure, control(), limits());
    submit(&mut owner, 0).unwrap();
    let error = Error::Framework {
        operation: Operation::Complete,
        status: -2,
    };
    assert_eq!(owner.start_drain(), Err(error));
    assert_eq!(owner.pull().unwrap_err(), error);
}

#[test]
fn retired_callbacks_hold_quota_and_cannot_reach_a_later_session() {
    let _serial = lock(&SERIAL);
    let (mut old, closes) = session(Mode::Silent, control(), limits());
    submit(&mut old, 0).unwrap();
    let key = old.registration.key;
    let lease = lease_callback(key, 1).unwrap();
    drop(old);
    assert_eq!(closes.load(Ordering::Relaxed), 1);
    assert!(lease_callback(key, 1).is_none());
    let mut owners = Vec::new();
    for _ in 1..MAX_MAILBOXES {
        owners.push(session(Mode::Immediate, control(), limits()).0);
    }
    assert!(matches!(
        Mailbox::new(settings(), limits(), control()),
        Err(Error::Limit)
    ));
    lease.finish(Ok(sample(
        Time::new(0, 30).unwrap(),
        Time::new(1, 30).unwrap(),
    )));
    let (mut next, _) = session(Mode::Immediate, control(), limits());
    assert_ne!(next.registration.key, key);
    callback_failed(key);
    submit(&mut next, 0).unwrap();
    assert_eq!(next.pull().unwrap().unwrap().frame_id, 0);
}

#[test]
fn dropping_an_unfinished_lease_fails_the_owner() {
    let _serial = lock(&SERIAL);
    let (mut owner, _) = session(Mode::Silent, control(), limits());
    submit(&mut owner, 0).unwrap();
    drop(lease_callback(owner.registration.key, 1).unwrap());
    assert_eq!(owner.pull().unwrap_err(), Error::Output);
}

#[test]
fn malformed_oversized_wrong_colour_and_mismatched_output_cannot_escape() {
    let _serial = lock(&SERIAL);
    for case in 0..8 {
        let (mut owner, closes) = session(Mode::Corrupt(case), control(), limits());
        let error = submit(&mut owner, 0).unwrap_err();
        assert_eq!(owner.pull().unwrap_err(), error);
        assert_eq!(owner.start_drain(), Err(error));
        drop(owner);
        assert_eq!(closes.load(Ordering::Relaxed), 1);
    }
}

#[test]
fn unknown_early_and_duplicate_copying_identities_are_terminal() {
    let _serial = lock(&SERIAL);
    let (mut owner, _) = session(Mode::Silent, control(), limits());
    assert!(lease_callback(owner.registration.key, 1).is_none());
    assert_eq!(submit(&mut owner, 0), Err(Error::Output));
    let (mut owner, _) = session(Mode::Silent, control(), limits());
    submit(&mut owner, 0).unwrap();
    let first = lease_callback(owner.registration.key, 1).unwrap();
    assert!(lease_callback(owner.registration.key, 1).is_none());
    first.finish(Ok(sample(
        Time::new(0, 30).unwrap(),
        Time::new(1, 30).unwrap(),
    )));
    assert_eq!(owner.pull().unwrap_err(), Error::Output);
}

#[test]
fn reservation_arithmetic_never_wraps_or_reuses_an_exhausted_identity() {
    let counter = AtomicUsize::new(usize::MAX - 1);
    assert_eq!(reserve_counter(&counter, 1, usize::MAX), Ok(usize::MAX - 1));
    assert_eq!(reserve_counter(&counter, 1, usize::MAX), Err(Error::Limit));
    assert_eq!(counter.load(Ordering::Relaxed), usize::MAX);
}

#[test]
fn a_missing_callback_expires_instead_of_completing() {
    let _serial = lock(&SERIAL);
    let deadline = Control::new(Instant::now() + Duration::from_secs(1));
    let (mut owner, _) = session(Mode::Silent, deadline, limits());
    submit(&mut owner, 0).unwrap();
    owner.start_drain().unwrap();
    assert_eq!(owner.pull().unwrap_err(), Error::Deadline);
    assert_eq!(owner.complete().unwrap_err(), Error::Deadline);
}

#[test]
fn cancellation_unblocks_pending_output_without_teardown() {
    let _serial = lock(&SERIAL);
    let control = control();
    let (mut owner, closes) = session(Mode::Silent, control.clone(), limits());
    submit(&mut owner, 0).unwrap();
    std::thread::scope(|scope| {
        scope.spawn(|| control.cancel());
        assert_eq!(owner.pull().unwrap_err(), Error::Cancelled);
    });
    assert_eq!(owner.start_drain(), Err(Error::Cancelled));
    drop(owner);
    assert_eq!(closes.load(Ordering::Relaxed), 1);
}

#[test]
fn completion_refuses_unconsumed_output_and_submission_after_drain() {
    let _serial = lock(&SERIAL);
    let (mut owner, _) = session(Mode::Immediate, control(), limits());
    submit(&mut owner, 0).unwrap();
    owner.start_drain().unwrap();
    assert_eq!(owner.complete().unwrap_err(), Error::Output);
    let (mut owner, _) = session(Mode::Immediate, control(), limits());
    owner.start_drain().unwrap();
    assert_eq!(submit(&mut owner, 0), Err(Error::State));
}

#[test]
fn successful_accounting_seals_callbacks_before_the_native_teardown_gap() {
    let _serial = lock(&SERIAL);
    let (mut owner, closes) = session(Mode::Immediate, control(), limits());
    submit(&mut owner, 0).unwrap();
    owner.pull().unwrap().unwrap();
    owner.start_drain().unwrap();
    assert!(owner.pull().unwrap().is_none());
    let done = owner.seal_output(owner.identity().unwrap()).unwrap();
    assert_eq!(done.frames, 1);
    assert!(lease_callback(owner.registration.key, 1).is_none());
    assert_eq!(lock(&owner.mailbox.inner).error, None);
    drop(owner);
    assert_eq!(closes.load(Ordering::Relaxed), 1);
}

#[test]
fn invalid_time_input_and_global_reservation_limits_fail_before_submission() {
    let _serial = lock(&SERIAL);
    assert_eq!(Time::new(-1, 30), Err(Error::Input));
    assert_eq!(Time::new(0, 0), Err(Error::Input));
    assert!(Frame::bt709_limited(&[0; 4], 32, 32).is_err());
    let (mut owner, _) = session(Mode::Immediate, control(), limits());
    submit(&mut owner, 0).unwrap();
    assert_eq!(submit(&mut owner, 0), Err(Error::Input));
    assert_eq!(owner.pull().unwrap_err(), Error::Input);
    assert!(matches!(
        Mailbox::new(
            settings(),
            Limits {
                max_access_unit_bytes: MAX_RESERVED_BYTES,
                max_queue_bytes: MAX_RESERVED_BYTES,
                ..limits()
            },
            control()
        ),
        Err(Error::Limit)
    ));
}

#[test]
fn creation_failure_retires_the_registry_and_unknown_hardware_is_not_software() {
    let _serial = lock(&SERIAL);
    let error = Session::<Fake>::new(settings(), limits(), control(), |key| {
        assert!(lookup(key).is_some());
        Err(Error::Unavailable(Availability::NoEncoder))
    })
    .err()
    .unwrap();
    assert_eq!(error, Error::Unavailable(Availability::NoEncoder));
    assert!(lock(&REGISTRY).is_empty());
    let closes = Arc::new(AtomicUsize::new(0));
    let mut settings = settings();
    settings.hardware = HardwarePolicy::Disable;
    let error = Session::new(settings, limits(), control(), |key| {
        Ok(Fake {
            key,
            mode: Mode::UnknownHardware,
            pending: Vec::new(),
            closes: Arc::clone(&closes),
        })
    })
    .err()
    .unwrap();
    assert_eq!(error, Error::UnknownHardware);
    assert_eq!(closes.load(Ordering::Relaxed), 1);
}

#[test]
fn cadence_hints_do_not_replace_fractional_source_clocks() {
    for (num, den, hint) in [
        (30_000, 1001, 30),
        (24_000, 1001, 24),
        (60_000, 1001, 60),
        (3, 2, 2),
        (5, 2, 3),
        (7, 3, 2),
        (1, 1, 1),
        (479, 2, 240),
        (i32::MAX as u32, i32::MAX as u32, 1),
    ] {
        let settings =
            Settings::bt709_limited(32, 30, num, den, 2_000_000, HardwarePolicy::Require).unwrap();
        assert_eq!(settings.expected_frame_rate_hint(), hint);
        assert_eq!(settings.rate(), (num, den));
    }
}

#[test]
fn native_fractional_rate_creation_reads_back_the_requested_hint() {
    let _serial = lock(&SERIAL);
    let settings =
        Settings::bt709_limited(32, 30, 30_000, 1001, 2_000_000, HardwarePolicy::Require).unwrap();
    let encoder = Encoder::new(settings, Limits::default(), control()).unwrap();
    drop(encoder);
}

#[test]
fn native_drop_retires_its_key_before_the_outer_constructor_returns() {
    let _serial = lock(&SERIAL);
    let control = control();
    let mailbox = Mailbox::new(settings(), limits(), control.clone()).unwrap();
    let registration = Registration::new(Arc::clone(&mailbox)).unwrap();
    let native = sys::Native::without_session(registration.key, settings(), limits(), control);
    assert!(lookup(registration.key).is_some());
    drop(native);
    assert!(lookup(registration.key).is_none());
    assert!(matches!(lock(&mailbox.inner).phase, Phase::Closed));
    drop(registration);
    assert!(lock(&REGISTRY).is_empty());
}
