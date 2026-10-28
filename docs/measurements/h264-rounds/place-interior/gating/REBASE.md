# Level shift of the host before the gating run (REBASE)

The first start of `run-ab-helper.sh 084ee40 42a5388` on this VM stopped before any call completed: its first call (`g.sd`,
one thread) was voided three times, each time by an after-gate at 0.949 of the class's reference on the 1080p gate stream,
with the before-gates alternating between 0.949 and 0.953 (FFmpeg 9.0.2 at 4.16 to 4.18 s against the reference's 4.388 s,
the 720p stream at 0.976 to 0.981). The host was steady and faster than the reference, not slow or noisy. The voided
reports (`*.void1` to `*.void3`) were not read.

The level-shift rule of the host gate (`HOST-GATE.md` on `measured/host-gate`, "Level-shift rule (between calls only)") was
applied between calls: six consecutive gate checks (`rebase-checks.log`, within two minutes, all failing at 0.948 to 0.949 on
the 1080p stream) agreed with each other within 0.2% on both streams (the rule asks for 2%), so their medians become this
host's reference for the run:

| Stream | Class reference (2.90 GHz) | Six checks (s) | New reference (median) | Shift |
|---|---|---|---|---|
| `36501d448eaa.0.720p` | 1.295 s | 1.263 to 1.265 | **1.264 s** | -2.4% |
| `a6af290d9053.0.1080p` | 4.388 s | 4.158 to 4.163 | **4.160 s** | -5.2% |

`hostgate-rebased.py` is `host-gate/hostgate.py` with these two numbers for model 106 and nothing else changed; the run
passes it as `HOSTGATE`. The tolerance stays +-5% before and after every call, and a failed after-gate still voids a call.
The run restarts from its first call (no call had completed: a host-fault restart). REBASE count for the run: 1.
