# Agent integration: the `coreav mcp` server

Status: Accepted. Scope: agent discoverability: an MCP server
with the command's operations on local paths and public URLs.

## What it does

- `coreav mcp [--root DIR]... [--timeout SECONDS]` is a Model Context
  Protocol server over standard input and output (JSON-RPC 2.0, one
  message per line). It offers nine tools, one per command: `probe`,
  `check`, `clip`, `split`, `remux`, `sanitize`, `extract_audio`,
  `extract_subtitles`, `chapters`. `coreav hls` has no tool yet: it writes
  an index and a directory of resources, or one resource from an index,
  rather than one output file.
  Each returns the command's JSON document, as text and as
  `structuredContent`, or an error result with the command's
  `coreav.error/1` object or message. Code: `crates/coreav-cli/src/mcp.rs`.

## Choices

1. **The server is a subcommand of the `coreav` command, in Rust.** It
   ships wherever the command does: the binary, the Python wheel
   (`uvx coreav mcp`) and a later npm package, with no new dependency (the
   protocol is a few JSON-RPC methods, and `serde_json` is already used).
   Rejected: a Python server on the official MCP SDK, which brings an
   interpreter-side dependency tree (pydantic, anyio, HTTP stacks) for
   three methods; a Python server on the standard library, which would not
   serve the binary or npm users.
2. **Each tool call runs the `coreav` command itself as a child process.**
   A tool then returns exactly what the command prints, with its exit
   codes and messages, and nothing about a command is written twice. A
   child is also the unit of isolation: a call over its time limit is
   killed, a call that prints too much fails, and neither affects the
   server. Starting a process costs milliseconds, which is nothing next to
   an agent's turn. Rejected: calling the library in-process, which would
   repeat each command's option handling a third time (after `main.rs` and
   the Python module) and could not stop a long call. The command is the
   running executable; from the Python wheel it is the installed `coreav`
   script (the first argument), since the executable there is Python. If
   the running command's path cannot be found, `coreav mcp` exits rather
   than run a `coreav` found on `PATH`.
3. **Paths stay inside the roots.** The server reads and writes only under
   its `--root` directories (default: the current directory, where an
   agent host starts it). A path is resolved, links included, before it is
   compared with the resolved roots, so `..` and a link pointing outside
   are refused. A relative path is relative to the first root. An output's
   directory must resolve inside a root, and an output that is itself a
   link is refused, so a write cannot be redirected outside. A media
   input may also be a URL (`docs/design/remote.md`), as `--urls` allows:
   `public` (the default) takes `https://` URLs of public addresses only,
   and the commands the server runs get `COREAV_REMOTE=public`, which makes
   them check every address a host resolves to when connecting (DNS
   rebinding and redirects included); `any` takes what the command line
   takes, private addresses and `http://` included, for a server on a
   developer's own network; `none` takes local files only. Outputs and
   policies are always local files. `split`'s
   segment pattern (`%d`) may appear only in the file name: the command
   expands it anywhere in the path, so a `%` in the directory could name
   directories the check never saw (a directory `a%d` beside links `a0`,
   `a1`). A path that does not exist is checked on its nearest existing
   directory, so the answers do not tell whether a file outside the roots
   exists. Rejected:
   trusting the client's MCP roots, which a client may not send and the
   server cannot verify. The check and the child's open are separate
   steps: a process that rewrites links inside a root between them could
   redirect a call, but such a process can already write anywhere the
   server can, so the check guards against paths an agent is given, not
   against other local processes.
4. **Outputs are files, never standard output**, which carries the
   protocol. An existing output is replaced only with `overwrite: true`
   (the command's `--force`); the input is never overwritten, which the
   command already refuses.
5. **Limits.** A message longer than 1 MiB is answered with a parse error
   and skipped; a tool call may print at most 1 MiB on each stream, and run
   at most `--timeout` seconds (default 600), after which it is killed and
   its output is no longer waited for (a process it started could hold the
   pipes open; the command starts none). `split` takes at most 10,000 cut
   times. The command's own limits
   (`Limits::default()`, the codec allowlists) apply to every file. Calls
   run one at a time.
6. **Arguments are checked before the command runs.** Unknown arguments,
   wrong types and negative indexes are error results naming the
   argument. Every value reaches the command as `--name=value` and every
   path as a resolved absolute path, so no value is read as an option.
7. **`check` takes the policy inline or as a file.** An inline
   `coreav.policy/1` object is written to a new file in the temporary
   directory, with a random name, created exclusively and readable only by
   its owner, and removed after the call; error messages name it
   `(policy)`.
8. **Protocol.** `initialize` (protocol versions 2025-11-25, 2025-06-18,
   2025-03-26 and 2024-11-05; another requested version gets the newest),
   `ping`, `tools/list` and `tools/call`; notifications and responses from
   the client get no answer; batches, ids other than strings and integers,
   and unknown methods get JSON-RPC errors. The tools carry
   descriptions written for choosing between coreav and ffmpeg, input
   schemas with `additionalProperties: false`, and read-only hints.
   Rejected for now: resources and prompts, which the use cases do not
   need.
9. **The server stands alone.** Any MCP client starts `coreav mcp` (for
   example as `uvx coreav mcp`, which needs no install step). Rejected for now: a
   separate server-only package, which adds nothing while the server is
   part of the command.

## Not yet

- **Distribution.** Listing the server in a registry is outward-facing and
  waits for the maintainer. It may not be listed before the `coreav` name is
  published on PyPI: `uvx coreav` would otherwise fetch whatever package
  takes the name.
