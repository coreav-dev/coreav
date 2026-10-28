# Second run: the build-alignment round

`run2/run-ab-helper.sh 32bbbff 433ca30` (one directory up), the first run's two binaries (same hashes,
`provenance.txt`), on the same development VM class (Intel Xeon @ 2.90 GHz, family 6 model 106). `g.*`:
one thread pinned to core 2, five rounds; `t8.*`: eight threads unpinned, **ten** rounds. Output
identical on 32 of 32 streams (`identity.txt`); the host gate passed before and after each of the
eight calls, none void (`hostgate.log`); `analysis.txt` reproduces from the `*.json` with
`../analyze.py`. AL-1 was not met (upper end of the interval 1.0003 against below 1.000); the round
is rejected (docs/design/h264.md, "Build round: code alignment", "Second run").
