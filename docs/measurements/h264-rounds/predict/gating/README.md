# The predict round's gating run

`../run-ab-helper.sh 678bd69 4754464 fe97dea 7140ae8` on the 2.90 GHz Intel host (family 6, model 106; `provenance.txt`),
about an hour: builds, identity (`identity.txt`, 32 of 32 the same with `w`, `k1` and `k2` at 1 and 8 threads against
`base` at 1), then one gated call a class (`hostgate.log`: 8 gates, all PASS, ratios 0.961 to 1.010; no call void).

The script stopped at its last step with exit 1 (`run.log`): it copied `analyze.py` from the `k2` tree, which predates the
docs commit that holds it. The timing was complete; `analysis.txt` is `python3 -I ../analyze.py . g` run afterwards with the
committed analysis, byte-identical to 45a9b34's. The script now takes the analysis from beside itself.
