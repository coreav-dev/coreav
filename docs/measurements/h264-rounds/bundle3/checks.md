# Bundle 3: checks before any timing

`base` = `main` at `974f46c1` (bins `m-974f`, `m-974f-nf`); bundle = `perf/h264-bundle3` with `crates/` at `72cf01fe`
(bins `b3`, `b3-nf`), built `cargo build --locked --profile dist --example decode -p coreav-h264` (`-nf`: with
`--no-default-features`, the safe build without `fast`); `binaries.sha256`. Implementer's VM (Intel Xeon @ 2.50 GHz,
family 6, model 85, AVX-512).

- Output: identical to `base` on the 32 training streams at 1 and 8 threads, 64 of 64 with `fast`
  (`identity-fast.txt`) and 64 of 64 without (`identity-safe.txt`).
- Conformance: 97 of 97 at 1, 2 and 8 threads with detected and scalar kernels, fast and safe (`conformance.txt`,
  `scripts/h264_conformance.py`).
- `cargo fmt --all --check`, `cargo clippy -p coreav-h264 -p coreav-kernels-h264 --all-targets -- -D warnings` and
  `cargo test --release -p coreav-h264 -p coreav-kernels-h264`: clean, all tests pass.
- Fuzz targets `predict` and `filter_chroma_mb`, 300 s each, no finding (`fuzz-predict.txt`,
  `fuzz-filter_chroma_mb.txt`; this VM has AVX-512, so the targets compare the AVX-512 and AVX2 tiers with the
  reference).
- Callgrind instructions, one thread, discard, the class frame counts (`instructions-bundle-1t.txt`, `ic32.sh`;
  information): `python3 -I ic_ratio.py instructions-bundle-1t.txt m-974f b3` gives all 32 **0.9788**, 32 of 32
  fewer (0.9280 to 0.9946); sd 0.9881, 720p 0.9773, 1080p 0.9813, 4k 0.9510.
