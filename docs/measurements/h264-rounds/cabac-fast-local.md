# CABAC fast mode against safe mode, local timings

Directional evidence for the rejected hot-path `unsafe` sites (`docs/design/h264.md`, "Result: no site merged"); not a bar run.

- Machine: `Intel(R) Xeon(R) Processor @ 2.50GHz` (family 6, model 85,
  stepping 4), 8 logical CPUs, core 3 pinned.
- Toolchain: rustc 1.99.1 (`58cf8f9c35b355199727627d07ae34a4e2846d67`),
  LLVM 23.1.1.
- Builds: `cargo build --profile dist --example decode -p coreav-h264`, all
  three alike apart from the change:
  - `safe5`: branch tip `252003a`, `--no-default-features` (safe mode),
    SHA-256 `95758e096017c25f19b5833a80acd9c5ae788732a4918b1e86bfd5c301d8a32a`;
  - `f4`: built from a commit whose crate is the one at `5d4b370`, fast mode
    with `decision`,
    `map_4x4` and `map_8x8`, SHA-256
    `a4a4274301ecaa8e06a255451284a26f9a74b01fab4c75f78251240ef0fd142a`;
  - `f5`: `252003a`, fast mode with `level_prefix` too, SHA-256
    `267e49da457ce9a4b824b971b8e23ab57101cc9524216ed275e8e87490ca0e39`.
- Method: `decode --discard`, 9 rounds per stream in a rotating order, the
  child's user plus system CPU time, the per-stream ratio of medians to
  `safe5`, the median over streams. Ten training streams at 300 frames,
  four heavy 8x8-transform streams at 60.

| Stream | safe5 (s) | f4 | f5 |
|---|---|---|---|
| 36501d448eaa.0.720p | 1.023 | 1.003 | 0.984 |
| 41e324ebb708.0.sd | 0.372 | 0.990 | 1.020 |
| 6b18d9f0f88d.0.720p | 0.560 | 0.997 | 0.967 |
| 8ae90cadb5bc.0.sd | 0.509 | 0.987 | 0.983 |
| 9db41aa16b5a.0.720p | 1.080 | 1.003 | 1.000 |
| a81de82106be.0.1080p | 1.451 | 0.999 | 0.983 |
| 3c593b8f0809.0.720p | 1.493 | 1.004 | 0.988 |
| 6f7799768413.0.sd | 0.106 | 1.000 | 0.993 |
| 5185721dda11.0.sd | 0.137 | 1.011 | 1.000 |
| ed75c044e944.0.4k | 0.739 | 0.992 | 1.000 |
| **median, ten streams** | | **0.9995** | **0.9903** |
| 175433794e02.0.1080p | 1.854 | 1.006 | 1.000 |
| a6af290d9053.0.1080p | 2.365 | 0.989 | 0.988 |
| 6e90422a670a.0.1080p | 1.892 | 0.988 | 0.984 |
| f8d92a911e72.0.4k | 5.972 | 1.013 | 1.003 |
| **median, four heavy** | | **0.9971** | **0.9939** |

A wall-clock run of the same binaries at 60 frames per stream gave ratios
that moved by up to 8% between two sets of the same pair; that run is not
used.
