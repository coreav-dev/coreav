# Security policy

CoreAV reads untrusted media files: uploads, objects in storage, streams
from other services. A file that makes it misbehave is a security problem,
and we want to hear about it.

## Reporting a vulnerability

Report it privately, through GitHub (on the repository's **Security** tab,
choose **Report a vulnerability**: GitHub private vulnerability
reporting) or by e-mail to
[security@coreav.dev](mailto:security@coreav.dev). Do not open a public
issue or pull request for it.

Include:

- **a reproducer file** for any report about reading a file (parser,
  probe, policy check, stream copy): the smallest input that shows the
  problem, attached to the report;
- the command or API call, the CoreAV version or commit, and the platform
  (native, Python, npm, WebAssembly);
- what happens, with the exact output: a panic or abort, a hang, memory or
  time beyond the configured limits, an out-of-bounds access, a policy
  verdict that lets a rejected file through, or anything else.

## What counts

In scope:

- the crates under `crates/` and the packages under `packages/` that read
  untrusted input, including the `coreav` command, the Python bindings and
  the WebAssembly module;
- a panic or crash on any input (production code has a no-panic rule);
- work or memory beyond the `Limits` in force, or a limit that does not
  hold;
- memory unsafety of any kind;
- an upload policy check (`coreav check`) that accepts a file the policy
  rejects, or misreports what a file contains in a way a gate relies on.

Out of scope: the spikes (`docs/spikes/`), the measurement and oracle tools under
`tools/` and `scripts/` (development code, never shipped), the black-box
FFmpeg and MediaInfo binaries used in tests, and files that only read
differently from `ffprobe` without a security consequence (report those as
ordinary issues).

## What happens next

We reproduce the report, work on the fix in the advisory's private fork,
and tell you when we plan to publish it; nothing about it is public before
the fixed release ships. The project's own findings, a fuzzer's crash in
released code for example, take the same path: their inputs and details go
into the advisory, not into a public issue or a pull request's description
or comments. CI fuzzes every change briefly and its logs are public; when
such a run crashes in released code, its logs and artifacts are deleted
once the advisory holds them. A fixed vulnerability is published as a GitHub
security advisory, for which we request a CVE through GitHub; for the Rust
crates published on crates.io we also submit a RustSec advisory. Reporters
are credited in the advisory unless they ask not to be.

Fixes go into the latest release only; CoreAV has no long-term support
branches.

There is no bug bounty.
