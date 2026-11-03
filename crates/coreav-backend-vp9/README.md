# coreav-backend-vp9

CoreAV's scalar profile-0 VP9 decoder behind the decode boundary, in safe Rust and in the calling process.

This experimental backend is opt-in and is never linked by a default or release build. Google's VP9 patent grant covers Google's implementations, not this decoder. An operator may need a patent licence. Nothing here is legal advice. See `docs/design/vp9.md`, "Packaging".

`NativeVp9` accepts 8-bit 4:2:0 VP9 packets and superframes. Its keyframe-only mode returns decoded keyframes and their still-valid shown references. Frame timestamps come from each packet. Discarded packets update references without exported sample copies. Every decode-boundary limit is checked; a failed send is terminal and drops pending output. The registered judged filter run and the complete G4 verdict remain outside this work.
