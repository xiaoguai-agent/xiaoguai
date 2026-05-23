//! Report I/O — JSON writer for [`EvalReport`].

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::types::EvalReport;

#[derive(Debug, Error)]
pub enum ReportError {
    #[error("io error writing {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("json serialization failed: {0}")]
    Serde(#[from] serde_json::Error),
}

impl EvalReport {
    /// Write the report as pretty-printed JSON to `path`. Overwrites
    /// existing files — callers handle versioning.
    pub fn write_json(&self, path: &Path) -> Result<(), ReportError> {
        let body = serde_json::to_string_pretty(self)?;
        std::fs::write(path, body).map_err(|source| ReportError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::types::{CaseStatus, EvalReport, EvalResult};
    use chrono::Utc;

    #[test]
    fn write_json_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("report.json");
        let started = Utc::now();
        let report = EvalReport::from_results(
            "regression",
            started,
            started,
            vec![
                EvalResult {
                    case_id: "a".into(),
                    status: CaseStatus::Pass,
                    transcript_len: 3,
                    duration_ms: 12,
                },
                EvalResult {
                    case_id: "b".into(),
                    status: CaseStatus::Fail {
                        reasons: vec!["nope".into()],
                    },
                    transcript_len: 1,
                    duration_ms: 7,
                },
            ],
        );
        report.write_json(&path).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["suite"], "regression");
        assert_eq!(parsed["results"].as_array().unwrap().len(), 2);
        assert!((parsed["pass_rate"].as_f64().unwrap() - 0.5).abs() < 1e-3);
    }
}
