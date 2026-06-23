//! Integration tests for `xiaoguai pack validate` (skill-pack loader Phase 1).
//!
//! Offline, hermetic: most cases build a throwaway pack in a tempdir so they
//! don't depend on the repo's `packs/` layout. One case validates a real
//! shipped pack to prove the actual manifests load.

use std::path::Path;

use xiaoguai_cli::commands::pack;

/// Write `pack.yaml` into a fresh tempdir and return the dir handle.
fn pack_dir(yaml: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tmpdir");
    std::fs::write(dir.path().join("pack.yaml"), yaml).expect("write pack.yaml");
    dir
}

#[tokio::test]
async fn validate_minimal_ok() {
    let dir = pack_dir("name: testpack\nversion: \"1.0.0\"\n");
    let report = pack::validate(dir.path()).await.expect("valid");
    assert!(report.contains("testpack"), "{report}");
    assert!(report.contains("manifest valid"), "{report}");
    assert!(
        report.contains("0 migration(s), 0 watch(es), 0 anomaly(ies), 0 agent(s)"),
        "{report}"
    );
}

#[tokio::test]
async fn validate_counts_declared_artifacts() {
    let dir = pack_dir(
        "name: counted\nversion: \"1.0.0\"\nmigrations:\n  - path: m/0001.sql\nwatches:\n  - path: w/a.yaml\n",
    );
    std::fs::create_dir_all(dir.path().join("m")).unwrap();
    std::fs::write(dir.path().join("m/0001.sql"), "-- noop\n").unwrap();
    std::fs::create_dir_all(dir.path().join("w")).unwrap();
    std::fs::write(dir.path().join("w/a.yaml"), "id: a\n").unwrap();

    let report = pack::validate(dir.path()).await.expect("valid");
    assert!(report.contains("1 migration(s), 1 watch(es)"), "{report}");
}

#[tokio::test]
async fn validate_accepts_bare_string_paths() {
    // ~45% of shipped manifests write declared paths as bare strings rather
    // than `{ path: ... }` maps; the loader tolerates both.
    let dir = pack_dir("name: bare\nversion: \"1.0.0\"\nmigrations:\n  - m/0001.sql\n");
    std::fs::create_dir_all(dir.path().join("m")).unwrap();
    std::fs::write(dir.path().join("m/0001.sql"), "-- noop\n").unwrap();
    let report = pack::validate(dir.path())
        .await
        .expect("bare-string path is valid");
    assert!(report.contains("1 migration(s)"), "{report}");
}

#[tokio::test]
async fn validate_fails_on_missing_declared_path() {
    // Declares a migration file that does not exist → should fail to load.
    let dir = pack_dir("name: broken\nversion: \"1.0.0\"\nmigrations:\n  - path: nope.sql\n");
    let err = pack::validate(dir.path()).await.unwrap_err();
    assert!(
        err.to_string().contains("broken") || format!("{err:#}").contains("does not exist"),
        "got: {err:#}"
    );
}

#[tokio::test]
async fn validate_warns_on_unknown_keys() {
    // `depends` is a real structural key the loader does not model — surfaced,
    // not silently dropped (the packs/* manifests are not schema-uniform).
    // `author` is conventional metadata in KNOWN_KEYS and must NOT be flagged,
    // so the warning stays meaningful (fires only on unmodeled structure).
    let dir = pack_dir("name: extra\nversion: \"1.0.0\"\nauthor: someone\ndepends:\n  - other\n");
    let report = pack::validate(dir.path()).await.expect("still valid");
    assert!(report.contains("ignored unknown manifest key"), "{report}");
    assert!(report.contains("depends"), "{report}");
    assert!(
        !report.contains("author"),
        "known metadata not flagged: {report}"
    );
}

#[tokio::test]
async fn validate_warns_on_unrecognised_feature() {
    let dir = pack_dir(
        "name: feat\nversion: \"1.0.0\"\nrequires:\n  features:\n    - watch\n    - frobnicate\n",
    );
    let report = pack::validate(dir.path()).await.expect("valid");
    assert!(report.contains("unrecognised feature"), "{report}");
    assert!(report.contains("frobnicate"), "{report}");
    // A known feature must NOT be flagged.
    assert!(!report.contains("unrecognised feature(s) not provided by this build: watch"));
}

#[tokio::test]
async fn validate_accepts_pack_yaml_file_directly() {
    let dir = pack_dir("name: direct\nversion: \"1.0.0\"\n");
    let file = dir.path().join("pack.yaml");
    let report = pack::validate(&file).await.expect("valid");
    assert!(report.contains("direct"), "{report}");
}

#[tokio::test]
async fn validate_fails_when_no_manifest() {
    let dir = tempfile::tempdir().expect("tmpdir");
    let err = pack::validate(dir.path()).await.unwrap_err();
    assert!(err.to_string().contains("no pack.yaml"), "got: {err}");
}

/// A real shipped pack validates (proves the actual manifests load, not just
/// synthetic ones). Resolves the workspace `packs/` dir relative to this crate.
#[tokio::test]
async fn validate_real_ar_collections_pack() {
    let pack = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/ar-collections");
    if !pack.join("pack.yaml").exists() {
        eprintln!("SKIP: {} not present", pack.display());
        return;
    }
    let report = pack::validate(&pack).await.expect("ar-collections valid");
    assert!(report.contains("ar-collections"), "{report}");
    assert!(report.contains("manifest valid"), "{report}");
}

#[tokio::test]
async fn validate_accepts_ref_idiom() {
    // ~30% of shipped manifests reference declared files with `{ ref: ... }`
    // rather than `{ path: ... }` or a bare string; the loader treats all three
    // identically.
    let dir = pack_dir("name: refpack\nversion: \"1.0.0\"\nagents:\n  - ref: a/x.yaml\n");
    std::fs::create_dir_all(dir.path().join("a")).unwrap();
    std::fs::write(dir.path().join("a/x.yaml"), "id: x\n").unwrap();
    let report = pack::validate(dir.path()).await.expect("ref idiom valid");
    assert!(report.contains("1 agent(s)"), "{report}");
}

#[tokio::test]
async fn validate_all_batches_a_directory_of_packs() {
    // Parent dir holding two valid packs and one broken (missing migration).
    let parent = tempfile::tempdir().expect("tmpdir");
    for name in ["good-a", "good-b"] {
        let d = parent.path().join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("pack.yaml"), "name: x\nversion: \"1.0.0\"\n").unwrap();
    }
    let bad = parent.path().join("broken");
    std::fs::create_dir_all(&bad).unwrap();
    std::fs::write(
        bad.join("pack.yaml"),
        "name: b\nversion: \"1.0.0\"\nmigrations:\n  - nope.sql\n",
    )
    .unwrap();

    // The parent has no pack.yaml of its own → batch mode.
    assert!(!pack::is_single_pack(parent.path()));
    let outcome = pack::validate_all(parent.path()).await.expect("batch ran");
    assert_eq!(outcome.total, 3);
    assert_eq!(outcome.failed, 1);
    assert!(outcome.report.contains("✓ good-a"), "{}", outcome.report);
    assert!(outcome.report.contains("✗ broken"), "{}", outcome.report);
    assert!(
        outcome.report.contains("2/3 pack(s) valid"),
        "{}",
        outcome.report
    );
}

#[tokio::test]
async fn is_single_pack_detects_pack_dir_vs_parent() {
    let single = pack_dir("name: s\nversion: \"1.0.0\"\n");
    assert!(pack::is_single_pack(single.path()));
    let empty = tempfile::tempdir().unwrap();
    assert!(!pack::is_single_pack(empty.path()));
}

#[tokio::test]
async fn validate_soft_warns_missing_adapter_file() {
    // sources/outputs are parsed + counted but NOT hard-validated by the loader
    // (so the pack still passes); a declared adapter whose file is absent — e.g.
    // a work-in-progress pack — is surfaced as a soft warning, not a load error.
    let dir =
        pack_dir("name: wip\nversion: \"1.0.0\"\nsources:\n  - inbound/not-written-yet.yaml\n");
    let report = pack::validate(dir.path())
        .await
        .expect("still valid (soft)");
    assert!(report.contains("declares 1 source(s)"), "{report}");
    assert!(report.contains("not found"), "{report}");
    assert!(report.contains("inbound/not-written-yet.yaml"), "{report}");
}

#[tokio::test]
async fn validate_existing_adapter_not_warned() {
    let dir = pack_dir("name: ok\nversion: \"1.0.0\"\noutputs:\n  - out/a.yaml\n");
    std::fs::create_dir_all(dir.path().join("out")).unwrap();
    std::fs::write(dir.path().join("out/a.yaml"), "id: a\n").unwrap();
    let report = pack::validate(dir.path()).await.expect("valid");
    assert!(report.contains("1 output(s)"), "{report}");
    assert!(!report.contains("not found"), "no false warning: {report}");
    // sources/outputs are modeled now → NOT flagged as unknown keys.
    assert!(!report.contains("ignored unknown manifest key"), "{report}");
}
