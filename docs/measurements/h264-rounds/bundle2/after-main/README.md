# Bundle 2 after the rebase on `main`

`main` = `a62c11b4` (with earlier changes, which the gated `base` `bb7255cd` did not have); `bundle` = the branch
rebased on it. Binaries built alike (`cargo build --locked --profile dist --example decode -p coreav-h264`, safe builds
with `--no-default-features`; `binaries.sha256`).

- `identity-fast.txt`, `identity-safe.txt`: md5 of the full decode of each of the 32 training streams at 1 and 8
  threads, `bundle` against `main` (`ident.sh A B OUT`): 64 of 64
  identical in each.
- `conformance.txt`: `scripts/h264_conformance.py --threads 1,2,8`, detected and scalar kernels, fast and safe: 97 of 97
  in each of the four.
- `cargo test --release -p coreav-h264 -p coreav-kernels-h264`, clippy and fmt: pass.
