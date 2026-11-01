# coreav-vp9

A VP9 decoder in Rust, written from the VP9 Bitstream & Decoding Process Specification (version 0.7).

## Read this first

**This decoder is opt-in and is never part of a default CoreAV build.**

Google's patent grant for VP9 covers the implementations Google distributes as part of the WebM Project, such as libvpx, and not this decoder. A patent pool offers licences under patents it says relate to the VP9 specification. CoreAV therefore ships this decoder as a separately packaged backend that an operator enables deliberately, behind a non-default Cargo feature (`docs/design/vp9.md`, "Packaging"). CoreAV's default VP9 path is unchanged: libvpx, or the sandboxed FFmpeg fallback.

Nothing here is legal advice. Deciding whether you may use this decoder is yours to make.

## What it does today

It is being built in milestones (`docs/design/vp9.md`, "The plan, in milestones"). Today it reads every frame's uncompressed header (clause 6.2) and, through the boolean decoder (clause 9.2), its compressed header with the probabilities it updates (clause 6.3). It splits superframes (Annex B) and keeps the reference slots' sizes and the four saved probability contexts, so it can say which frames a stream shows and at what size. It decodes no samples yet. Its constant tables are read from the specification by `tools/gen_tables.py`.

## How it is built

- **Sans-IO.** It takes packets (one frame, or a superframe) and returns what they show. It opens no files and starts no threads.
- **Limits everywhere.** Every decoder takes `Limits`; a frame larger than they allow is refused before anything is allocated for it.
- **Safe Rust.** The crate is `forbid(unsafe_code)`, and hostile input gives an `Error`, never a panic.
- **Clean room.** Written from the specification only; the specification's clause numbers are in the code.
