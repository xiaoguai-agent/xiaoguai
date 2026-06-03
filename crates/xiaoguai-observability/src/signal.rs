//! The four Google SRE golden signals.
//!
//! Used by `xiaoguai-watch`'s alert sink to classify watch-task alerts.
//! (The broader SLO-contract machinery that also consumed this enum was
//! removed under the single-user pivot, DEC-033 — burn-rate / error-budget
//! tracking was a server-operator concern.)

use serde::{Deserialize, Serialize};

/// One of the four Google SRE golden signals.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    Latency,
    Traffic,
    Errors,
    Saturation,
}

impl Signal {
    /// Stable lowercase label for metrics / logs.
    #[must_use]
    pub fn as_label(self) -> &'static str {
        match self {
            Signal::Latency => "latency",
            Signal::Traffic => "traffic",
            Signal::Errors => "errors",
            Signal::Saturation => "saturation",
        }
    }
}
