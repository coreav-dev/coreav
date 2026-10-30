# CoreAV

A Rust-based media toolkit, built to be safe and fast. It aims to become a modern alternative to FFmpeg.

## Status

CoreAV is pre-release: no release exists yet. The first release is 0.1.0, tracked in [#43](https://github.com/coreav-dev/coreav/issues/43). Until then, commands, options and APIs can change.

Experimental: `coreav ffprobe` and `coreav ffmpeg`, until they meet their [0.1.0 bars](docs/design/compat-cli.md#the-010-bars-pre-registered), and `coreav transcode` for sources other than H.264.

## Install

Build from source with Rust 1.89 or later:

```sh
git clone https://github.com/coreav-dev/coreav
cd coreav
cargo install --locked --path crates/coreav-cli --profile dist
```

The default build has every command except `thumbnail` and `transcode`, which need a decoder backend feature ([build features](https://coreav.dev/docs/build-features)).

Coming with 0.1.0: `coreav` binaries for Linux, macOS and Windows on GitHub Releases, and the npm package `coreav`.

## Quick start

```sh
coreav probe input.mp4                            # prints a coreav.probe/1 JSON document
coreav clip --start 1 --end 3 input.mp4 clip.mp4  # cuts from the keyframe at or before 1 s
coreav remux input.mkv output.mp4                 # writes MP4 with the movie header first
```

`coreav --help` lists every command, and `coreav <command> --help` describes one. `coreav ffmpeg` has no help text of its own: strict-safe mode refuses `coreav ffmpeg --help` with exit status 4, and otherwise it prints the real `ffmpeg`'s help. [Its reference](https://coreav.dev/docs/commands/ffmpeg) lists what it runs.

## Read untrusted files in strict-safe mode

`coreav ffprobe` and `coreav ffmpeg` take `ffprobe` and `ffmpeg -c copy` command lines. A command line that CoreAV does not implement runs the real `ffprobe` or `ffmpeg` program instead, which is FFmpeg's C code, and CoreAV prints a one-line notice on standard error when it does. If you read files you did not make, such as uploads, set `COREAV_STRICT_SAFE=1` or pass `coreav --strict-safe`. In that mode CoreAV refuses such a command line with exit status 4 instead of running FFmpeg. See [strict-safe mode](https://coreav.dev/docs/strict-safe) for the details.

## Documentation

- [coreav.dev/docs](https://coreav.dev/docs): installing, a quick start, a page for each command, strict-safe mode, the Python module and safety.
- [Measurements](https://coreav.dev/docs/measurements): every measurement page, with its bars, method and raw evidence.
- Design notes: [docs/design](docs/design).
- Contributing: [CONTRIBUTING.md](CONTRIBUTING.md). CoreAV takes bug reports, feature requests and sample files as issues, not code pull requests.

## Security

Report a vulnerability privately, as [SECURITY.md](SECURITY.md) describes, and not in a public issue.

## License

Licensed under either the [MIT license](LICENSE-MIT) or the [Apache License 2.0](LICENSE-APACHE), at your option.
