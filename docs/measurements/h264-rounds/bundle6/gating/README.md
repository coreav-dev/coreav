# Bundle 6: the timed runs (B6-1 to B6-7)

Bars, method and the result: `docs/design/h264.md`, "Bundle 6: a band's kept view lent and the CAVLC P path, without the edge window". Arms: `base` and `k` as `../binaries.sha256` lists them, null arm `base`, FFmpeg 9.0.2 (BtbN `n9.0.2-17-g2a571b6068`) as the reference command. The wrappers are `scripts/h264_parity_gcp.py` and `scripts/gcp_vm_job.py` as coreav-dev/coreav#73 left them (its head `1b2b470`). They keep a run's state outside a temporary directory and can collect a job whose controller stopped (`--collect`).

| directory | bars | host and options | rounds |
|---|---|---|---|
| `gcp-8t/` | B6-1 to B6-4 | `c4-standard-16`, one thread a core; `scripts/h264_parity_gcp.py --seed 20261031 --max-hours 3` (eight threads), the 32 training streams | 10 |
| `gcp-8t-heldout/` | B6-7 | the same with `--max-hours 3.5`, the 64 held-out streams | 5 |
| `gcp-1t/` | B6-5, B6-6 | the fixed-frequency one-thread class: `c4-standard-8`, one thread a core, `turboMode=ALL_CORE_MAX`; `--one-thread-class --turbo all-core-max --seed 20261031 --max-hours 2.75`, the 32 training streams | 20 |

`analysis-8t.txt`, `analysis-8t-heldout.txt` and `analysis-1t.txt` are `python3 -I scripts/h264_parity_bars.py ../bars-<call>.json <directory>`. Each directory's `verdict.json` is not void. `host.txt` has the VM's name replaced by `VM`.

The machine that ran the wrappers restarted while the calls were running, so each job was collected with `--collect`. Two calls returned nothing. No result of the round had been read when they were started again:

- The first one-thread call ran to its time limit before it was collected, and the VM deleted itself, with the job's output, at that limit. The call was started again with the same options. `gcp-1t/` is that second call.
- The first held-out call reached its 3 h limit before its job ended (bundle 5's held-out call took 2.7 h), and the wrapper deleted the VM with no output. The call was started again with the same options and a 3.5 h limit. `gcp-8t-heldout/` is that second call, which took 2.8 h.

`gcp-8t/` was the only eight-thread call on the training streams.
