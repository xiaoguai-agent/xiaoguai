//! # xiaoguai-anomaly
//!
//! Time-series anomaly detection for the Xiaoguai active-wakeup system.
//!
//! ## Architecture
//!
//! ```text
//! AnomalySpec  ─────┐
//!                   ▼
//!            AnomalyRegistry  ◄──  poll tick (scheduler)
//!                   │
//!           ┌───────┴────────┐
//!           ▼                ▼
//!    ZScoreDetector    EwmaDetector
//!           │                │
//!           └──── Anomaly ───┘
//!                    │
//!              AnomalyStore  (InMemory | PG)
//! ```
//!
//! ## Quick start
//!
//! ```rust
//! use chrono::{Duration, Utc};
//! use xiaoguai_anomaly::{
//!     detector::ZScoreDetector,
//!     registry::{AnomalyRegistry, InMemoryStore},
//!     spec::{ActionRef, AnomalySpec, DetectorKind},
//! };
//!
//! let spec = AnomalySpec {
//!     id: "orders".to_string(),
//!     kpi_query: "SELECT COUNT(*) FROM orders".to_string(),
//!     window: Duration::hours(1),
//!     detector: DetectorKind::ZScore { sigma_threshold: 3.0, min_count: 10 },
//!     cool_off: Duration::minutes(5),
//!     on_anomaly: ActionRef::Notify { channel: "ops".to_string() },
//! };
//!
//! let mut registry = AnomalyRegistry::new(Box::new(InMemoryStore::default()));
//! registry.register(spec);
//!
//! let ts = Utc::now();
//! if let Some((spec, anomaly)) = registry.observe("orders", ts, 9999.0) {
//!     // dispatch spec.on_anomaly
//!     let _ = (spec, anomaly);
//! }
//! ```
//!
//! ## Scheduler integration
//!
//! `AnomalyRegistry` is `Send` but not `Sync` (interior mutability via `HashMap`).
//! Wrap it in a `Mutex<AnomalyRegistry>` inside the scheduler's `AppState` and
//! call `registry.observe(id, ts, value)` from each KPI poll task.  When
//! `Some((spec, anomaly))` is returned, read `spec.on_anomaly` to choose the
//! dispatch path (wake session / IM notify / webhook).

#![warn(clippy::pedantic)]
#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

pub mod baseline;
pub mod detector;
pub mod registry;
pub mod series;
pub mod spec;

// Re-export the most commonly needed types at the crate root.
pub use detector::{Anomaly, Detector, EwmaDetector, ZScoreDetector};
pub use registry::{AnomalyRegistry, AnomalyStore, InMemoryStore};
pub use series::TimeSeries;
pub use spec::{ActionRef, AnomalySpec, DetectorKind};
