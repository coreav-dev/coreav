//! The verdict, schema `coreav.verdict/1` (`docs/schema/verdict-v1.md`).

use serde_json::{Value, json};

/// The schema identifier of a verdict.
pub const SCHEMA: &str = "coreav.verdict/1";

/// What kind of failure a [`Reason`] records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    /// The probe rejected the file; [`Reason::error`] holds its
    /// `coreav.error/1` object.
    Unreadable,
    /// The value is not one the rule allows.
    NotAllowed,
    /// The value is above the rule's maximum.
    AboveMaximum,
    /// The value is below the rule's minimum.
    BelowMinimum,
    /// The rule needs a value the probe output does not give.
    Unknown,
}

impl Code {
    /// The name printed in `code`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Unreadable => "unreadable",
            Self::NotAllowed => "not_allowed",
            Self::AboveMaximum => "above_maximum",
            Self::BelowMinimum => "below_minimum",
            Self::Unknown => "unknown",
        }
    }
}

/// One reason a file was rejected.
#[derive(Clone, Debug, PartialEq)]
pub struct Reason {
    /// What kind of failure.
    pub code: Code,
    /// JSON pointer of the rule in the policy (for a kind that is not
    /// allowed, where its rules would be); `None` for [`Code::Unreadable`].
    pub rule: Option<String>,
    /// JSON pointer of the value the rule read in the `coreav.probe/1`
    /// document; `None` when it combined several (the overall bit rate,
    /// stream counts) or the file was unreadable.
    pub path: Option<String>,
    /// The stream's `stream_index` (`ffprobe`'s `index`), for a reason about
    /// one stream.
    pub stream_index: Option<u64>,
    /// The value compared with the limit.
    pub value: Value,
    /// The exact value as `(num, den)` in lowest terms, for durations and
    /// bit rates when both fit in 64 bits.
    pub exact: Option<(u64, u64)>,
    /// The limit: a number, or the allowed values.
    pub limit: Value,
    /// A sentence for people; its wording may change within `/1`.
    pub message: String,
    /// The probe's `coreav.error/1` object, for [`Code::Unreadable`].
    pub error: Option<Value>,
}

impl Reason {
    /// The reason as a JSON object; every key is always present.
    pub fn to_json(&self) -> Value {
        json!({
            "code": self.code.name(),
            "rule": self.rule,
            "path": self.path,
            "stream_index": self.stream_index,
            "value": self.value,
            "exact": self.exact.map(|(num, den)| json!({"num": num, "den": den})),
            "limit": self.limit,
            "message": self.message,
            "error": self.error,
        })
    }
}

/// The outcome of checking a file against a policy.
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    /// The policy's name.
    pub policy: Option<String>,
    /// Every reason the file was rejected, in the order of the verdict
    /// reference; empty when the file is accepted.
    pub reasons: Vec<Reason>,
}

impl Verdict {
    /// Whether the file is accepted: no rule failed.
    pub fn accepted(&self) -> bool {
        self.reasons.is_empty()
    }

    /// The verdict for a file the probe rejected: `error` is the probe's
    /// `coreav.error/1` object.
    pub fn unreadable(policy: Option<String>, error: Value) -> Self {
        let detail = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("the probe rejected the file");
        let reason = Reason {
            code: Code::Unreadable,
            rule: None,
            path: None,
            stream_index: None,
            value: Value::Null,
            exact: None,
            limit: Value::Null,
            message: format!("the file could not be read: {detail}"),
            error: Some(error),
        };
        Self {
            policy,
            reasons: vec![reason],
        }
    }

    /// The verdict document.
    pub fn to_json(&self) -> Value {
        json!({
            "schema": SCHEMA,
            "policy": self.policy,
            "verdict": if self.accepted() { "accept" } else { "reject" },
            "reasons": self.reasons.iter().map(Reason::to_json).collect::<Vec<_>>(),
        })
    }
}
