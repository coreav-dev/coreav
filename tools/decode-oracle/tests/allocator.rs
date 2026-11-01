//! The release allocator must fit the decoder worker's address-space cap.
#![cfg(all(
    feature = "libvpx",
    feature = "release-allocator",
    target_os = "linux",
    target_env = "musl",
    target_arch = "x86_64"
))]

use coreav_decode::{Codec, Decoder, Limits, StreamConfig};
use coreav_worker::{SandboxedDecoder, WorkerBackend, WorkerCommand, WorkerOptions};

#[test]
fn release_allocator_opens_a_worker_with_a_small_budget() {
    let stream = StreamConfig {
        width: Some(64),
        height: Some(48),
        ..StreamConfig::new(Codec::Vp9)
    };
    let options = WorkerOptions::new(WorkerCommand {
        program: env!("CARGO_BIN_EXE_decode-oracle").into(),
        args: vec![coreav_worker::WORKER_ARG.into()],
    });
    let limits = Limits {
        max_memory_bytes: 512 << 20,
        timeout: std::time::Duration::from_secs(5),
        ..Limits::default()
    };
    let decoder = SandboxedDecoder::open(&options, WorkerBackend::Libvpx, &stream, &limits)
        .expect("the release allocator must leave space for a 512 MiB worker");
    assert_eq!(decoder.info().backend, coreav_decode::BackendKind::Libvpx);
}
