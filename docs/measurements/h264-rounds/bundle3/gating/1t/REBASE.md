# REBASE of the host gate's reference (bundle 3 one-thread run, held-out calls)

After the first script was killed by its own 10 h timeout during the held-out 1080p call (`h.1080p.log.killed`; a crashed run, nothing read from it), the continuation `run-b3-1t-rest.sh` found, before its first call, that the host reads
steadily faster than the class reference: ten consecutive before-checks (`hostgate.log`) failed, the last six at 720p 1.264, 1.267, 1.263, 1.260, 1.267, 1.265 s (ref 1.295; ratios 0.973 to 0.979) and 1080p 4.163, 4.161, 4.155, 4.157, 4.161, 4.162 s
(ref 4.388; ratios 0.947 to 0.949): within 0.6% and 0.2% of each other, a steady level. The gate passed before and after every earlier call of the run (g.sd to g.4k, h.sd, h.720p; the level was about 3% under the reference at the last of them).
By the gate's level-shift rule (HOST-GATE.md, "REBASE"; as `place-interior/gating/REBASE.md`), the medians of the six, 720p 1.2645 s and 1080p 4.161 s, become the reference for the remaining calls (`hostgate-rebase.py`, the unchanged `hostgate.py` with only the model-106 line changed). REBASE count 1, between calls.
Written before the restart; the within-call paired ratios (both arms in one call) do not depend on the level.
