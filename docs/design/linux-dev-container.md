# Linux development container

## Goal

Run trusted Linux development commands on an Apple Silicon workstation without a remote development machine. This environment never judges registered measurements or processes untrusted media.

## Design

Use one Dockerfile and one command wrapper. Build linux/amd64 with the existing oracle installer and pin owner. Build linux/arm64 without the oracle. Read the Rust channel and components from `rust-toolchain.toml`, including its current `stable` channel rather than introducing another version.

The wrapper supplies a small, explicit build context, not the checkout. At runtime it mounts only the current worktree, with separate named volumes for Cargo registry data and build output. It passes no host environment variables or credentials. Commands run as a non-root image user with networking disabled unless explicitly requested, dropped capabilities, and memory and process limits.

Use per-worktree and per-architecture target volumes to separate concurrent work. Share only the registry cache. Keep dependency acquisition an explicit network-enabled command before offline tests.

## Verification

Build both architectures on the workstation. On amd64, run the existing Linux executable-inode regression and `scripts/oracle_pin.py --verify-bin`. Check the actual container mounts, identity, network mode and resource limits, and exercise argument forwarding and command failures. Record development timings in the pull request, never as performance evidence.
