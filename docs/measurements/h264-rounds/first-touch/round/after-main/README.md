# The first-touch round after the rebase on `main`

`main` = `641fb937` (with bundle 2, change A, and the boundary-strength kernel, change B, which the gated `base` `538265c4` did not
have); `ft` = the branch rebased on it. Binaries built alike (`cargo build --locked --profile dist --example decode -p
coreav-h264`, safe builds with `--no-default-features`; `binaries.sha256`).

- `identity-fast.txt`, `identity-safe.txt` (`ident.sh A B OUT`): `ft` against `main`, the 32 training streams at 1 and 8
  threads, 64 of 64 identical in each.
- `conformance.txt`: `scripts/h264_conformance.py --threads 1,2,8`, detected and scalar kernels, fast and safe: 97 of 97 in each.
- `cargo test --release -p coreav-h264 -p coreav-kernels-h264`, clippy and fmt: pass.
