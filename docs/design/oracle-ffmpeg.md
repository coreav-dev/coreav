# Oracle FFmpeg installations

`scripts/oracle-ffmpeg.env` remains the frozen Linux x86_64 oracle pin. `scripts/oracle-ffmpeg-macos.env` is the single owner of macOS arm64 download URLs, archive hashes and executable identities. The macOS installation uses Martin Riedl's prebuilt FFmpeg and ffprobe of the same stable release version, not the Linux build's additional release-branch commits.

## Installation plan

`scripts/install-oracle-ffmpeg-macos.sh` supports macOS arm64 only. It downloads the two versioned ZIP archives directly from the publisher, verifies each SHA-256 before extraction, and extracts only the named executable. It verifies arm64 architecture, the publisher's Apple Developer ID signature, both exact version banners and the `h264_videotoolbox` encoder listing before installing either program. A mismatch fails the installation without replacing existing programs. Unsupported platforms fail before download. The original Linux installer and fallback remain byte-for-byte unchanged.

`oracle_pin_macos.py --verify-bin DIR` repeats the macOS checks, including for cached installations. Its `--check` mode rejects a second owner of macOS pin values and requires the stable release version to match the existing Linux pin. The frozen `oracle_pin.py --check` remains unmodified and must also pass with the macOS files tracked. Neither checker exempts the new pin through a documentation or evidence directory.

The separate `oracle-macos.yml` workflow runs both pin checks, installer tests and a real macOS installation. Its trigger filters cover both pins, the macOS installer and verifier, their tests, and the workflow itself. The workflow checker applies the existing installer-trigger rule separately to each platform. No file in `compat_sources.FROZEN` or the registered judging roster changes. These checks establish binary identity and encoder availability, not successful encoding or session-property observability.

## Provenance and license

The [publisher's download page](https://ffmpeg.martin-riedl.de/) provides stable, versioned macOS arm64 archives and separate SHA-256 sidecars. Both downloaded archive hashes matched those sidecars. Both executables passed Apple code-signature verification with Developer ID Application authority Martin Riedl and team `KU3N25YGLU`. `lipo -archs` reports arm64. The pinned URLs, hashes, banner token and signing team live only in the env file.

`ffmpeg -L` identifies this build as GPL-3.0-or-later. Its configuration includes `--enable-gpl` and `--enable-version3`. It is a test-only black-box oracle, invoked as a separate program. It is neither linked into CoreAV nor included in CoreAV release artifacts or this repository. Installation downloads no source, libraries, documentation or examples. No FFmpeg implementation source was read or built, and neither binary was disassembled.

The native installer smoke used those already-downloaded publisher ZIPs through a cache-only test transport. Archive and executable hashes, native Apple signature and architecture checks, exact banners and encoder listings all passed. The installed encoder produced four synthetic H.264 High-profile frames at 128x96, which the installed ffprobe read back. The macOS CI step exercises direct publisher downloads. This smoke does not qualify the binary as a judged baseline.

## VideoToolbox observation limits

The existing [trace parser](../../scripts/encoders/macos/observations.py) and [build registration](../../scripts/encoders/macos/registration.py) remain the only contracts for judged VideoToolbox runs. This installation does **not** satisfy them. No alternate record schema or inference from requested controls is introduced.

A local synthetic smoke on an Apple M5 Pro MacBook Pro with macOS 27.0.1 verified the following limits. The input was generated with `testsrc`, without external media. After generating 90 raw yuv420p frames at 30000/1001, the encode used `registration.command_templates` with 128x96 geometry, bitrate 1000000, a 59-frame keyframe bound and the registered hardware controls. The command exited zero.

| Evidence | What the binary actually reports |
| --- | --- |
| Selected encoder | At trace verbosity, `[h264_videotoolbox @ ...] Init the encoder: com.apple.videotoolbox.videoencoder.ave.avc` identifies the selected Apple encoder. Stream metadata naming `h264_videotoolbox` identifies only FFmpeg's wrapper. |
| Hardware status and effective controls | No explicit session readback lines for hardware status, ExpectedFrameRate integer hint, bitrate or rate-control mode, reordering, realtime, either keyframe bound, submitted count or status. Requested arguments are not observations. |
| Encoded output | ffprobe reports High profile, level 20, 128x96 visible geometry, yuv420p, limited `tv` range, and bt709 primaries, transfer and matrix. An explicit-level MP4 smoke separately reports level 31. These fields describe output, not the effective session properties required by the trace parser. |
| Output order and keyframes | ffprobe reports no B frames and keyframe indices 0, 59 and 60. That short output does not prove the session's configured reordering or maximum keyframe interval properties. |
| Counts and timestamps | FFmpeg's summary reports 90 input frames decoded, 90 frames encoded and 90 packets muxed. `-stats_enc_post` records PTS equal to DTS from 0 through 89 with time base 1001/30000. This is packet accounting, not an explicit submitted-session count or drain-state readback. |
| Rational clocks | The raw H.264 output's ffprobe `r_frame_rate` is 1200000/1 and `avg_frame_rate` is 25/1. Neither establishes the submitted 30000/1001 source clock, and neither can establish the separate integer ExpectedFrameRate property. |

The real trace supplies the EncoderID but cannot populate the remaining closed fields. `observations.trace` continues to fail with `ffmpeg_observability_missing`. Output bitrate cannot establish effective configured bitrate or rate control. Output keyframe spacing cannot establish the configured frame and duration bounds. A separate native probe cannot attest properties of FFmpeg's session.

`registration.baseline_build` also continues to reject this build. It requires the Linux `BUILD` identity, `--disable-doc`, and a configuration without `--enable-gpl`. The prebuilt has a different build identity and a GPL configuration without that configure flag. Installing it does not waive any of those checks. A separately reviewed change must resolve the observability, provenance and clock gaps before this binary can become a judged baseline. No quality, speed or conformance result is claimed here.
