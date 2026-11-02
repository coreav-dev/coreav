# coreav-kernels-vp9

The pixel kernels of CoreAV's VP9 decoder (`coreav-vp9`): the inverse DCT, ADST and WHT, and the intra predictors, written from clauses 8.5.1 and 8.7 of the VP9 Bitstream & Decoding Process Specification (version 0.7).

## Read this first

**These kernels are part of an opt-in decoder that is never part of a default CoreAV build.**

Google's patent grant for VP9 covers the implementations Google distributes as part of the WebM Project, such as libvpx, and not this decoder. A patent pool offers licences under patents it says relate to the VP9 specification. CoreAV therefore ships the decoder as a separately packaged backend that an operator enables deliberately, behind a non-default Cargo feature (`docs/design/vp9.md`, "Packaging"). CoreAV's default VP9 path is unchanged: libvpx, or the sandboxed FFmpeg fallback.

Nothing here is legal advice. Deciding whether you may use this decoder is yours to make.

## How it is built

- **Pure arithmetic.** A kernel holds no decoder state and reads no bitstream. It writes only the samples of the `BlockMut` it is given, whose length and stride were checked when it was built.
- **Scalar first.** These are the reference kernels. Faster ones come later and are compared with them bit for bit (`docs/design/vp9.md`, milestone 6).
- **Clean room.** Written from the specification only; its constants are read from the specification by `crates/coreav-vp9/tools/gen_tables.py`.
