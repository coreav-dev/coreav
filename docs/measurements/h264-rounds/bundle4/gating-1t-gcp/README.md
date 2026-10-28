# Bundle 4, B4-4 (one thread): run on the Compute Engine one-thread class

**Host change and reason.** B4-4 was pinned to the 2.90 GHz VM class. Those VMs failed their host gates for hours, so the arm was run on the Compute Engine one-thread class instead ("The one-thread class of rounds whose bars are committed from here on", `docs/design/h264.md`; calibration `measured/h264-gcp-1t-class`):
`c4-standard-8`, `threadsPerCore=1` (4 vCPUs), Xeon Platinum 8581C @ 2.30 GHz (family 6 model 207), `us-central1-a`. Bundle 4 does not depend on an ISA tier and B4-4 is not a held-out verdict, so a within-run A/B on another recorded class is allowed ("Round A/B runs on more than one class"). The arms, rounds and bar are as pinned:
base = main `1e7dd54b`, k = `30804952` (the branch's only commit in `crates/`), null arm = base, six rounds, `--pin 2`, seed 20261030, the 32 training streams, discard and write, FFmpeg 9.0.2 (`0d709a0b...`). Identity of `k` against `base` on the 32 streams at 1 and 8 threads, checked before timing (on another host): 32 of 32 (`identity.txt`).
Binaries (`binaries.sha256`) built with `dist`, `--locked`; harness `scripts/h264_parity.py` of main `ccb35cf2`'s tree, through `scripts/h264_parity_gcp.py --one-thread-class`.

Host gate (own references, `gate-refs.json`, `gate-checks.log`): references 1.246, 1.241, 1.231 s (720p stream) and 4.223, 4.156, 4.204 s (1080p stream), within 1.1% of their median; 8 of 8 checks before and after the four calls passed (worst 0.962 after `sd`); no call repeated, none unresolved, no stream timed again; largest null deviation 1.0%, largest round spread 1.105. `verdict.json`: not void. 0.853 h, spend bound **0.512 USD**.

Result (B4-4: at most 1.003 each, discard and write), `k` over `base`, median per-stream ratio over the 32 (95% bootstrap interval over streams, `random.Random(20261024)`), faster of the streams:

| | all 32 | sd | 720p | 1080p | 4k |
|---|---|---|---|---|---|
| discard | **1.0017** [1.0007, 1.0037] (10/32) | 1.0016 | 1.0017 | 1.0020 | 1.0029 |
| write | **1.0006** [0.9994, 1.0018] (11/32) | 0.9983 | 1.0004 | 1.0007 | 1.0023 |

Both medians are within the 1.003 bar; the upper end of the discard interval (1.0037) is above it, and 4k discard is 1.0029. Against FFmpeg 9.0.2 on this host (median per-stream ratio, faster of the streams), base then k: discard 0.964 (21/32), 0.967 (21/32); write 0.943 (27/32), 0.947 (26/32); per class discard (k) sd 0.938, 720p 1.017, 1080p 1.021, 4k 0.878.
