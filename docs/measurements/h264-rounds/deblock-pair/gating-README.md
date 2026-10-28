# Chroma deblocking pair: gating run

Base `e6d810cf`, A `824746a9` (`70864099` and later differ from it in docs only), one call per class, `--pin 2
--rounds 5`, FFmpeg 9.0.2, the 2.90 GHz class (family 6, model 106). `run-ab-helper.sh` is the run script.

The raw reports `g.<class>.json` name the two decoders `before` and `after` (the A/B helper's template); the round's
`analyze.py` reads `base` and `a`. The first `analyze.py` call on the raw reports found no streams and failed
(the traceback is not kept). `mapped/` holds the same reports with the keys renamed mechanically (`before_*` to
`base_*`, `after_*` to `a_*`, `before:*` to `base:*`, `after:*` to `a:*`, and the `decoders` map); `analysis.txt` and
`g.summary.json` are from `mapped/`. No timing was re-run.
