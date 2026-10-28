# Local A/B (information)

This VM: Intel Xeon @ 2.90 GHz (family 6, model 106), four cores with two threads each, no PMU. `base` = `main` at
`82ad7c31`, `k` = the branch (`../binaries.sha256`). `run.sh`: eight threads unpinned, six rounds (`ab8.py`); one
thread pinned to CPU 3, four rounds (`ab1t.py`). Median per-stream ratio `k` over `base`, 95% bootstrap interval.

| | all 32 | sd | 720p | 1080p | 4k |
|---|---|---|---|---|---|
| eight threads, wall | 1.0000 [0.9821, 1.0123] | 0.9936 | 0.9981 | 1.0108 | 1.0129 |
| eight threads, CPU | 0.9910 [0.9769, 1.0053] | 0.9786 | 0.9989 | 1.0039 | 1.0070 |
| one thread, CPU | 1.0001 [0.9988, 1.0035] | 1.0010 | 1.0031 | 0.9990 | 1.0010 |

`first-version/`: the first version of the change, which keyed the kept views by the decoding picture's bands
(`docs/design/h264.md`, the round's "Design"): eight threads, wall 1.0053 [0.9963, 1.0219] (1080p 1.0217, 4k
1.0267), CPU 1.0080 [1.0002, 1.0195]. That reading led to the fix; its one-thread run was stopped unfinished.

This VM did not show bundle 4's eight-thread gain either (its local A/B read 0.9977 against 0.9832 on the gating
host), so the stall check on the gating host's CPU (`../check/`) decides whether the round goes to timing.
