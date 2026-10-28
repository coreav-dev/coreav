# Luma residual round: gating run on the Intel 2.90 GHz VM

Run by the branch's `run-ab-helper.sh 6a4ff38 e49998b` (base main `6a4ff38`, k the branch's code commit `e49998b`; the script as fetched from the branch tip is kept as `run-ab-helper.sh.used`).
Both arms built by the script on this VM (`dist`, `--locked`, rustc 1.99.1; binary SHA-256 and the host-gate script's in `provenance.txt`). `analyze.py` is the branch's (`analysis.txt`).

**Host class: development VM, Intel Xeon @ 2.90 GHz, family 6 model 106 (the script stops on any other class; `provenance.txt`).** Identity 32 of 32 (k at 1, 2 or the listed thread counts against base at 1, `identity.txt`). Eight calls between gate checks
(`g`: four pinned `--pin 2 --rounds 20`; `t8`: four unpinned `--threads 8 --rounds 10`): 16 gate passes (`hostgate.log`), no void, no failed check. Spend: none.

Result (`analysis.txt`): `bars (k over base): LD-1 FAIL, LD-2 pass, LD-3 pass, LD-4 pass` (LD-5, output, is the identity and the conformance report). One thread pinned: discard overall 0.9989 [0.9953, 1.0006] (21/32 faster), write 0.9989 [0.9954, 1.0028];
per class discard sd 1.0012, 720p 0.9991, 1080p 0.9991, 4k 0.9915 [0.9873, 0.9943]. Eight threads: discard overall 0.9981 [0.9893, 1.0055], write 1.0019 [0.9946, 1.0074].
