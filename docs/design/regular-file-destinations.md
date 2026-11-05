# Regular-file output destinations

Plan committed before implementation. Refs coreav-dev/coreav#25.

An existing native raw-S16 command with `-y` replaces a named FIFO with a regular file. The shared atomic output transaction currently checks aliases and directories, but treats other existing destination types as replaceable files. FFmpeg instead writes into a FIFO. Native atomic-file writers must not silently change this filesystem object.

Keep one destination-admission check in the shared output owner. Reject existing non-regular destinations, all symbolic links, multiply-linked Unix files and destinations without write access before staging and recheck before publication. Preserve ordinary writable regular-file overwrite behavior. Atomic replacement cannot reproduce FFmpeg's link traversal or its refusal to truncate an unwritable target, so these cases must refuse instead. Keep file creation and resource limits unchanged.

The FFmpeg compatibility boundary turns a known non-regular destination into strict-safe exit 4 or unchanged-command fallback, before native work writes output. Direct native commands retain their normal output-error handling. A destination changed to a special file after preflight is a terminal publication error; do not fall back after output execution.

Verify FIFO/socket/device-link preservation at the shared owner, including a FIFO created during the write callback. Add a reproducer on the existing raw-S16 compatibility path: strict-safe emits nothing, preserves the FIFO and leaves no staging residue; real pinned fallback preserves it and streams the same complete raw bytes as FFmpeg 9.0.2. Retain regular-file permission and replacement tests. Run focused tests, Clippy, formatting, public-text checks and the exact-head similarity guard. No frozen material, caps or baselines change.
