//! Upload policy checks over CoreAV's probe output
//! (`docs/design/policy-check.md`).
//!
//! An operator writes a [`Policy`] (schema `coreav.policy/1`,
//! `docs/schema/policy-v1.md`): allowed containers, stream kinds and codecs;
//! maximum file size, duration, bit rates, stream counts, resolution,
//! channels and sample rate; allowed rotations. [`evaluate`] checks a
//! `coreav.probe/1` (ISO base media) or `coreav.probe-matroska/1` (Matroska
//! and WebM) document against it and returns a [`Verdict`] (schema
//! `coreav.verdict/1`, `docs/schema/verdict-v1.md`): accept, or reject with
//! every failed rule as a machine-readable [`Reason`] that points into both
//! the policy and the probe document.
//!
//! The resolution rules compare the larger of the size the sample entry
//! declares and the size the codec's parameter sets and frame headers give
//! (`docs/design/frame-sizes.md`), so a file cannot pass them by
//! understating its size.
//!
//! The crate is sans-IO and container-neutral: it reads JSON values only,
//! never media bytes, so the same check runs on the native command, in a
//! WebAssembly build and on stored probe output. The design and the
//! alternatives considered are in `docs/design/policy-check.md`.
//!
//! ```
//! use coreav_policy::{Policy, evaluate};
//! use serde_json::json;
//!
//! let policy = Policy::from_json(br#"{
//!     "schema": "coreav.policy/1",
//!     "file": {"max_size": 1000000},
//!     "streams": {"audio": {"codecs": ["aac"]}}
//! }"#).unwrap();
//! let probe = json!({
//!     "schema": "coreav.probe/1",
//!     "file": {"size": 2000000},
//!     "container": {"major_brand": "M4A ", "fragmented": false},
//!     "duration": null,
//!     "streams": [{"stream_index": 0, "kind": "audio", "codec": "aac",
//!                  "duration": null, "bit_rate": null, "sample_descriptions": 1,
//!                  "tables_valid": true, "external_data": false}],
//!     "dropped_streams": []
//! });
//! let verdict = evaluate(&policy, &probe);
//! assert!(!verdict.accepted());
//! assert_eq!(verdict.reasons[0].rule.as_deref(), Some("/file/max_size"));
//! assert_eq!(verdict.reasons[0].path.as_deref(), Some("/file/size"));
//! ```

mod evaluate;
mod exact;
mod policy;
mod verdict;

pub use evaluate::{MATROSKA_PROBE_SCHEMA, PROBE_SCHEMA, evaluate};
pub use policy::{
    Action, AudioRules, CODECS, ContainerRules, DurationRules, FileRules, Format, KINDS, KindRules,
    MAX_POLICY_BYTES, Policy, PolicyError, ROTATIONS, SCHEMA as POLICY_SCHEMA, Seconds,
    StreamRules, VideoRules,
};
pub use verdict::{Code, Reason, SCHEMA as VERDICT_SCHEMA, Verdict};
