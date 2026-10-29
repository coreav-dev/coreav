# Bundle 3b: checks before any timing

`base` = `main` at `1e7dd54b` (`m-1e7d`, `m-1e7d-nf` without `fast`); bundle = branch `perf/h264-bundle3b`, crates
of `bd1f8b30` (`b3n`, `b3n-nf`); `binaries.sha256`. Built with `cargo build --locked --profile dist --example decode
-p coreav-h264` (and `--no-default-features`), `RUSTFLAGS` unset, on the implementer's VM.

| check | result | file |
|---|---|---|
| output, 32 training streams at 1 and 8 threads, `fast` | identical, 64 of 64 | `identity-fast.txt` |
| output, the same without `fast` | identical, 64 of 64 | `identity-safe.txt` |
| conformance, 97 bitstreams at 1, 2 and 8 threads, detected and scalar kernels, fast and safe | 97 of 97 in each | `conformance.txt` |
| `cargo fmt --all --check`, `clippy -D warnings` (both crates, all targets), tests of both crates, `coreav-h264`'s also without `fast` | pass | |
| fuzz targets `luma_mc`, `weighted_mc`, `predict`, `filter_chroma_mb`, 300 s each | no finding | `fuzz-*.txt` |
| N's mutation check: the window's last-row copy cut to 8 samples | `a_4_wide_luma_block_matches_the_reference` fails | |

Instructions (callgrind, one thread, discard, the class frame counts; `instructions-bundle-1t.txt`, read with
`python3 -I ../bundle3/ic_ratio.py instructions-bundle-1t.txt m-1e7d b3n`): all 32 **0.9763**, 32 of 32 fewer
(0.9259 to 0.9933); sd 0.9872, 720p 0.9730, 1080p 0.9728, 4k 0.9357. Bundle 3 on its base read 0.9788 (4k 0.9510).

N alone over P, R and C on the four streams with 4-wide luma blocks (`instructions-n.txt`; `scalar-luma-share.txt`
for where the reference kernel's share was found): 0.9671, 0.9719, 0.9748, 0.9846.
