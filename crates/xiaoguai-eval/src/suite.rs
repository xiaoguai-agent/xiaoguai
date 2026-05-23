//! YAML loading for [`EvalSuite`].
//!
//! On-disk schema: one suite per file, plus a directory loader that
//! aggregates every `*.eval.yaml` file under a path into a single
//! suite (used by `xiaoguai eval run --suite regression --cases-dir
//! examples/eval/regression`).

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::types::{EvalCase, EvalSuite};

const CASE_EXTENSION: &str = "eval.yaml";

#[derive(Debug, Error)]
pub enum SuiteError {
    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("yaml parse error in {path}: {source}")]
    Yaml {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },
    #[error("no .eval.yaml files found under {path}")]
    EmptyDir { path: PathBuf },
}

impl EvalSuite {
    /// Aggregate every `*.eval.yaml` file directly under `dir` into
    /// a single suite. Each file may contain either one [`EvalCase`]
    /// or a fragment-style `{ id, ... }` object that matches the
    /// `EvalCase` serde shape — we keep the loader simple: one
    /// file = one case.
    ///
    /// Subdirectories are *not* recursed; suites stay flat so
    /// `--suite regression` and `--suite capability` are visually
    /// obvious from the layout.
    pub fn load_from_dir(name: impl Into<String>, dir: &Path) -> Result<Self, SuiteError> {
        let mut paths: Vec<PathBuf> = Vec::new();
        let read = std::fs::read_dir(dir).map_err(|source| SuiteError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        for entry in read {
            let entry = entry.map_err(|source| SuiteError::Io {
                path: dir.to_path_buf(),
                source,
            })?;
            let path = entry.path();
            if path.is_file() && is_case_file(&path) {
                paths.push(path);
            }
        }
        if paths.is_empty() {
            return Err(SuiteError::EmptyDir {
                path: dir.to_path_buf(),
            });
        }
        // Stable, alphabetical ordering — predictable test output.
        paths.sort();

        let mut cases = Vec::with_capacity(paths.len());
        for path in paths {
            let raw = std::fs::read_to_string(&path).map_err(|source| SuiteError::Io {
                path: path.clone(),
                source,
            })?;
            let case: EvalCase = serde_yaml::from_str(&raw).map_err(|source| SuiteError::Yaml {
                path: path.clone(),
                source,
            })?;
            cases.push(case);
        }
        Ok(Self {
            name: name.into(),
            cases,
        })
    }
}

fn is_case_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.ends_with(&format!(".{CASE_EXTENSION}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Assertion, MockScript, MockTurn};
    use xiaoguai_llm::Message;

    fn write_case(dir: &Path, name: &str, case: &EvalCase) {
        let path = dir.join(format!("{name}.{CASE_EXTENSION}"));
        let yaml = serde_yaml::to_string(case).unwrap();
        std::fs::write(path, yaml).unwrap();
    }

    fn sample_case(id: &str) -> EvalCase {
        EvalCase {
            id: id.into(),
            input_messages: vec![Message::user("hi")],
            mock_script: Some(MockScript::new(vec![MockTurn::text("hello")])),
            assertions: vec![Assertion::FinalMessageContains {
                text: "hello".into(),
            }],
            tags: vec!["smoke".into()],
        }
    }

    #[test]
    fn round_trip_two_cases() {
        let tmp = tempfile::tempdir().unwrap();
        write_case(tmp.path(), "a", &sample_case("a"));
        write_case(tmp.path(), "b", &sample_case("b"));
        let suite = EvalSuite::load_from_dir("regression", tmp.path()).unwrap();
        assert_eq!(suite.name, "regression");
        let ids: Vec<&str> = suite.cases.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn empty_dir_is_error() {
        let tmp = tempfile::tempdir().unwrap();
        let err = EvalSuite::load_from_dir("x", tmp.path()).unwrap_err();
        assert!(matches!(err, SuiteError::EmptyDir { .. }));
    }

    #[test]
    fn non_case_files_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("README.md"), "ignore me").unwrap();
        std::fs::write(tmp.path().join("notes.yaml"), "key: value").unwrap();
        write_case(tmp.path(), "only", &sample_case("only"));
        let suite = EvalSuite::load_from_dir("x", tmp.path()).unwrap();
        assert_eq!(suite.cases.len(), 1);
        assert_eq!(suite.cases[0].id, "only");
    }

    #[test]
    fn malformed_yaml_surfaces_path_in_error() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("bad.eval.yaml"), ": : :").unwrap();
        let err = EvalSuite::load_from_dir("x", tmp.path()).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("bad.eval.yaml"), "error path = {msg}");
    }
}
