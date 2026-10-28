# Kernel round: the 8x8 path at 4k and a per-macroblock call (probe)

`docs/design/h264.md`, "Kernel round: the 8x8 path at 4k and a per-macroblock call, a probe". Development VM of the
2.90 GHz class (Intel Xeon @ 2.90 GHz, family 6, model 106). No code was written for the round; this is the instruction
bound that decided not to.

- `cg.sh`: callgrind of one training stream, `main` (crates as `5b934feb`) built with
  `CARGO_PROFILE_DIST_DEBUG=line-tables-only cargo build --locked --profile dist --example decode -p coreav-h264`, one
  thread, `--discard`, 240, 120, 60 and 30 pictures for sd, 720p, 1080p and 4k. The outputs are not kept (about 2 GB).
- `edges.py`: a callgrind output to its total, self instructions per function, and the calls and inclusive instructions of
  each call edge.
- `bound.py` over the 32 `edges.py` outputs (`CG` names their directory), with the DC-only shares of
  `../luma-dc/counts.txt`: `bound.txt` (also the bound with every inter 8x8 block DC-only, and the inter 8x8
  transforms' count with their instructions and the glue's per transform).

```
# in a worktree of main: CARGO_PROFILE_DIST_DEBUG=line-tables-only cargo build --locked --profile dist --example decode -p coreav-h264
cp TARGET/dist/examples/decode decode-dbg; mkdir cg
ls TRAINING/*.h264 | DECODE=$PWD/decode-dbg OUT=$PWD/cg xargs -P 7 -n 1 ./cg.sh
for f in cg/*.out; do python3 -I edges.py $f > ${f%.out}.json; done
CG=cg python3 -I bound.py > bound.txt
```
