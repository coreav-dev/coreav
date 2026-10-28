# The CABAC registers round on 4k, on the 2.90 GHz Intel class (information)

An earlier change. After the deciding run (Zen 3, `../gating/`), the 4k class alone on the 2.90 GHz Intel host (family 6, model 106),
because the non-deciding Zen 4 run (`../gating-zen4/`) read 4k above 1.005. Not a bar.

- `base` = `main` at 28b2b22 (`bin-main28`), `c4` = the branch rebased onto it (`bin-c4r`, the code of 83a075c's tree);
  both `cargo build --locked --profile dist --example decode`; sha256 in `provenance.txt`.
- `scripts/h264_parity.py --decoder base=… --decoder c4=… --frames 150 --rounds 5 --pin 2 --require-cpu "Intel(R) Xeon(R)
  Processor @ 2.90GHz"` on the 8 4k training streams (`g.4k.json`, `g.4k.log`).
- Host gate (`hostgate.py`, the class's recorded reference): before, PASS 720p 1.009, 1080p 0.987; after, PASS 1.011, 0.978.

`c4` over `base`, median per-stream ratio, 95% bootstrap interval (seed 20261023, as `../analyze.py`):

| | median | interval | faster |
|---|---|---|---|
| discard | 0.9969 | [0.9852, 1.0052] | 5/8 |
| write | 0.9903 | [0.9765, 1.0152] | 5/8 |

Per stream, discard: 1.004 1.005 1.013 0.988 0.999 0.995 0.963 0.985; write: 1.007 1.018 1.015 0.983 0.977 0.997 0.960
0.983 (in the order of `g.4k.json`). No loss on 4k on this class.
