# coreav-hevc

An opt-in, safe Rust HEVC header parser written from ITU-T H.265 (V11), January 2026. It parses Main and Main10 parameter sets, VUI, SEI and slice segment headers. It does not decode pictures or entropy-coded slice data.

HEVC can require a patent licence depending on jurisdiction and use. This crate is separately packaged, unpublished, and absent from default artifacts. Its API requires the `native-hevc` feature. No CLI, Python, WebAssembly or npm default enables it. See [the design](../../docs/design/hevc.md).

The parser reads no files and makes no OS calls. Callers supply one complete NAL unit without its framing and set `Limits`. Annex B and length-prefixed iterators are also available. Errors distinguish malformed input, unsupported coding tools and exhausted limits. Stored parameter sets use fixed slots; RBSP scratch uses a checked, fallible allocation bounded by `max_nal_bytes`.
