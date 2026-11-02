# coreav-vp9

A VP9 decoder in Rust, written from the VP9 Bitstream & Decoding Process Specification (version 0.7).

## Read this first

**This decoder is opt-in and is never part of a default CoreAV build.**

Google's patent grant for VP9 covers the implementations Google distributes as part of the WebM Project, such as libvpx, and not this decoder. A patent pool offers licences under patents it says relate to the VP9 specification. CoreAV therefore ships this decoder as a separately packaged backend that an operator enables deliberately, behind a non-default Cargo feature (`docs/design/vp9.md`, "Packaging"). CoreAV's default VP9 path is unchanged: libvpx, or the sandboxed FFmpeg fallback.

Nothing here is legal advice. Deciding whether you may use this decoder is yours to make.

## What it does today

It is being built in milestones (`docs/design/vp9.md`, "The plan, in milestones"). `Decoder::read_headers` reads every frame's uncompressed header (clause 6.2) and, through the boolean decoder (clause 9.2), its compressed header with the probabilities it updates (clause 6.3). It splits superframes (Annex B) and keeps the reference slots' sizes and the four saved probability contexts, so it can say which frames a stream shows and at what size. It reads the tiles of intra frames (clause 6.4), reconstructs them (clauses 8.5.1, 8.6 and 8.7) and applies the loop filter (clause 8.8), with the pixel kernels of `coreav-kernels-vp9`: each intra frame comes with its decoded picture. Its constant tables are read from the specification by `tools/gen_tables.py`.

`FrameDecoder::decode` is a separate pixel-decoding entry point with its own reference pictures and adapted probability contexts. It reconstructs profile 0 intra frames and single-reference or compound inter frames, including subblocks and motion-vector prediction, and outputs shown pictures. Hidden frames refresh state, and `show_existing_frame` returns the stored filtered picture. Inter-frame segmentation supports updated and retained maps, temporal prediction, forced references and skip features; no partial picture or undecoded reference is accepted. This is part of milestone 4, not the complete decoder or gate G4. Neither entry point has a backend or CLI consumer yet.

## How it is built

- **Sans-IO.** It takes packets (one frame, or a superframe) and returns what they show. It opens no files and starts no threads.
- **Limits everywhere.** Every decoder takes `Limits`; a frame larger than they allow is refused before anything is allocated for it.
- **Safe Rust.** The crate is `forbid(unsafe_code)`, and hostile input gives an `Error`, never a panic.
- **Clean room.** Written from the specification only; the specification's clause numbers are in the code.
