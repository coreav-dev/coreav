# MB layer round: only what later stages read (local timing)

`docs/design/h264.md`, "MB layer round: only what later stages read". Development VM of the 2.90 GHz class (Intel Xeon @
2.90 GHz, family 6, model 106). Arms built alike (`cargo build --locked --profile dist --example decode -p
coreav-h264`, separate target directories): `base` = `main`'s crates (as `40a656fe`), `k` = `e480d9cc`
(`rejected/h264-mb-layout`).

- `local-ab.jsonl`: `../luma8x8/ab.py local-ab.jsonl 6 base=BASE k=K` (one thread pinned to CPU 2, `--discard`, the
  32 training streams at the class frame counts, interleaved, six rounds).
- `local-ab.txt`: `../luma8x8/an.py local-ab.jsonl base k`.

The timed `k` binary has the md5 of a build of `e480d9cc` from a clean worktree (`12adc6d8d6cd027d9d98c8d0a6e92549`).

Spend: 0 USD (local VM only).
