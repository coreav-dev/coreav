# Rules for agents and contributors, and what enforces each

Read `CONTRIBUTING.md` and `.github/pull_request_template.md` first; design notes for each area are in `docs/design/`. Before other work, read every area file that applies to the change, and always read the cross-cutting files `docs/agents/ci.md`, `docs/agents/public-text.md`, and `docs/agents/measurement.md`. Loading this file does not load those rules. The area files below pair every rule with what enforces it, so a rule that nothing enforces is visible.

When a review corrects a mistake, fix it and add its row to the area file it belongs to. If the rule is already in that file and nothing enforces it, that is a repeat: in the same change, enforce it at the highest level that works (one owner or one way to do it, then a type, then a check whose error names the fix, then a test) and say why a higher level did not. Drop a row once its mistake cannot happen. A row that fits several areas goes in the file whose code the enforcement names.

## Area files

| File | What it covers |
|---|---|
| [docs/agents/ci.md](docs/agents/ci.md) | Workflow required checks, path filters, DCO, pinned actions, merge-group reuse, and workspace lints. |
| [docs/agents/codecs.md](docs/agents/codecs.md) | H.264, AAC, Opus, VP9, AC-3, MP3, and AudioToolbox behavior. |
| [docs/agents/compat-corpus.md](docs/agents/compat-corpus.md) | Corpus exclusions, compatibility workflows, fixture comparisons, and the B4 and R4 drivers. |
| [docs/agents/containers.md](docs/agents/containers.md) | ISO BMFF, Matroska, MPEG-TS, and AVI. |
| [docs/agents/docs.md](docs/agents/docs.md) | Documentation-site prose, schema wording, and design-note updates. |
| [docs/agents/ffmpeg-cli.md](docs/agents/ffmpeg-cli.md) | CLI and ffprobe behavior, the FFmpeg pin, and fallback names. |
| [docs/agents/measurement.md](docs/agents/measurement.md) | Speed rounds, bars, thresholds, capacity, and measurement hosts. |
| [docs/agents/public-text.md](docs/agents/public-text.md) | Pull request title, body, and issue-reference text. |
| [docs/agents/release-and-notices.md](docs/agents/release-and-notices.md) | Licenses, distribution notices, and package rehearsal. |
| [docs/agents/transcode.md](docs/agents/transcode.md) | Encode path, sandboxed workers, and audio-encode selection. |
| [docs/agents/videotoolbox.md](docs/agents/videotoolbox.md) | VideoToolbox encode, hardware status, and the macOS encoder measurement tools. |
| [docs/agents/wasm.md](docs/agents/wasm.md) | The WASM module and its HTTP source. |
