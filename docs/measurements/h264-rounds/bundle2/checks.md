# Bundle 2: checks before any timing

`base` is `main` `bb7255cd`, `bundle` is `perf/h264-bundle2` at the commit that adds this file (the composition in
`plan.md`). Binaries: `cargo build --locked --profile dist --example decode -p coreav-h264`, one toolchain,
`RUSTFLAGS` unset; the safe builds add `--no-default-features`.

| check | result | file |
|---|---|---|
| output, 32 training streams at 1 and 8 threads, `fast` | 64 of 64 identical | `identity-fast.txt` |
| output, the same without `fast` | 64 of 64 identical | `identity-safe.txt` |
| conformance, 1, 2 and 8 threads, detected and scalar kernels, fast and safe | 97 of 97 in each of the four | `conformance.txt` |
| tests of every item (`cargo test --workspace`), clippy, fmt | pass | |
| B's fuzz target `chroma_mb` | builds; 120 s, 4.1M runs, no finding | |
| F's instructions over A to E | median 0.9945, 32 of 32 fewer | `instructions-item-f.txt` |
