<h1 align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs-site/logo/dark.svg">
    <img alt="CoreAV" src="docs-site/logo/light.svg" width="320">
  </picture>
</h1>

<h3 align="center">The modern FFmpeg alternative</h3>

<p align="center">A Rust-based media toolkit, built to be safe and fast.</p>

<p align="center">
  <a href="https://coreav.dev/docs">Documentation</a> ·
  <a href="https://coreav.dev/docs/measurements">Measurements</a> ·
  <a href="https://github.com/coreav-dev/coreav/releases">Releases</a>
</p>

## Highlights

- **Safe.** Every container parser is safe Rust with limits on memory and work, so a damaged or hostile file gives an error, never a crash. [Safety](https://coreav.dev/docs/safety)
- **Fast.** CoreAV reads only the bytes a job needs, from a local file or an `https://` URL, and every speed claim is measured against FFmpeg in the open. [Measurements](https://coreav.dev/docs/measurements)
- **Drop-in.** `coreav ffprobe` and `coreav ffmpeg` take the command lines your scripts already run, and hand any line CoreAV doesn't run natively to the FFmpeg installed on the machine. [Commands](https://coreav.dev/docs/commands)
- **No re-encoding.** Probe, cut, split, join, remux and sanitize media files, and copy out their audio, subtitles and chapters, all by stream copy.
- **Everywhere.** Native binaries for Linux, macOS and Windows, and an npm package that runs as WebAssembly wherever else Node.js runs. [Platforms](https://coreav.dev/docs/platforms)

## Installation

```sh
npm install -g coreav
```

Or download a binary from [GitHub Releases](https://github.com/coreav-dev/coreav/releases). [Install](https://coreav.dev/docs/install) has every option.

## Usage

```sh
coreav probe input.mp4                               # describe the file as JSON
coreav clip --start 10 --end 20 input.mp4 clip.mp4   # cut at keyframes
coreav remux input.mkv output.mp4                    # Matroska to MP4, movie header first
coreav ffprobe -v error -show_streams input.mp4      # the ffprobe command line you already use
```

For files you didn't make, such as uploads, add `--strict-safe` (`coreav --strict-safe ffprobe ...`): CoreAV then refuses any command line it doesn't run natively instead of handing it to FFmpeg. [Strict-safe mode](https://coreav.dev/docs/strict-safe)

## Documentation

CoreAV's documentation is at [coreav.dev/docs](https://coreav.dev/docs), and `coreav --help` lists every command.

## Contributing

CoreAV takes bug reports, feature requests and sample files as issues, not code pull requests. [CONTRIBUTING.md](CONTRIBUTING.md) says what to send, and the design notes are in [docs/design](docs/design). Report a vulnerability privately, as [SECURITY.md](SECURITY.md) describes.

## License

Licensed under either the [MIT license](LICENSE-MIT) or the [Apache License 2.0](LICENSE-APACHE), at your option.
