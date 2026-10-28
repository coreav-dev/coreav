# The view path's one-thread cost at 4k, on a quiet host: result

Plan and rules, committed before the run: `plan.md`. Run: `run/` (`scripts/h264_parity_gcp.py --threads 1 --pin 3
--classes 4k`, `c4-standard-16` in `us-central1-a`, 10 rounds, seed 20261027). The binaries are the local timing's:
`decode-b2` `47ef059e88ce2740…`, `decode-vp2` `78b8f3b128518b70…` (`run/host.txt`). The run is not void
(`run/verdict.json`): the reference moved +1.04%, and no stream was unresolved. VM 0.79 h, spend bound 0.95 USD.
With the void first attempt (`plan.md`, 0.03 USD), 0.98 USD in all.

## Result (`run/summary.txt`, `summary.py`)

| one thread, 4k, the 8 training streams | median | 95% interval | `vp2` faster |
|---|---|---|---|
| `vp2` / `b2`, discard | **1.0035** | [0.9886, 1.0077] | 3 of 8 |
| `vp2` / `b2`, write | 0.9987 | [0.9936, 1.0037] | |
| null arm (`b2` twice), discard | **+0.61%** | largest 1.15% | |

**The run decides nothing (D-2): the null arm's median, +0.61%, lies outside [0.995, 1.005].** D-1's reading of
1.0035 would meet its 1.005, but the same binary timed twice differs by more than D-1's margin on this host at one
pinned thread. So the 4k cost seen on the development VM (1.0152) is not confirmed. It is not ruled out either, and
by the rule the view path stays out of the bundle pool. The run is not repeated: D-2 is not a host fault.

As information only: this host read 1.0035 (discard) and 0.9987 (write), with intervals that include 1.000, not
the development VM's 1.0152. The one-thread pinned null arm on `c4-standard-16` moved +0.61% here, against +0.09%
and −0.03% for the eight-thread null arms of probes PV and PW on the same machine type
(`../scaling-8t/pv-run1/`, `../publish-wake/pw-run1/null.txt`). So a one-thread bar on that host should probably
allow about 1%; that figure is an inference from this one run.

Against FFmpeg 9.0.2 (information): `b2` 0.9756 at one thread at 4k on this host.
