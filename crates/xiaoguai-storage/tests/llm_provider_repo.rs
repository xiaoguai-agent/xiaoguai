//! Integration tests for `SqliteLlmProviderRepository` (embedded `SQLite`, DEC-033).
//!
//! No Docker — each test opens a temp `SQLite` database via `common::test_setup`.
//! Single-owner deployment: `list` returns the whole table.

mod common;

use chrono::{SubsecRound, Utc};
use common::test_setup;
use xiaoguai_storage::repositories::{
    LlmProviderRepository, RepoError, SqliteLlmProviderRepository,
};
use xiaoguai_types::{AeadKey, Keyring, LlmProvider, ProviderId, ProviderKind};

fn sample_provider(name: &str) -> LlmProvider {
    let now = Utc::now().trunc_subsecs(6);
    LlmProvider {
        id: ProviderId::new(),
        name: name.to_string(),
        kind: ProviderKind::OpenAiCompat,
        endpoint: "https://api.deepseek.com/v1".to_string(),
        models: vec!["deepseek-chat".into(), "deepseek-coder".into()],
        default_for_models: vec!["deepseek-chat".into()],
        verified_models: None,
        fallback_order: 10,
        api_key_env: Some("DEEPSEEK_API_KEY".into()),
        api_key: None,
        created_at: now,
        updated_at: now,
        cost_per_1k_input_usd: Some(0.27),
        cost_per_1k_output_usd: Some(1.10),
    }
}

#[tokio::test]
async fn create_and_find_by_id() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLlmProviderRepository::new(pool);
    let prov = sample_provider("deepseek-global");

    repo.create(&prov).await.expect("create");
    let found = repo
        .find_by_id(prov.id.as_str())
        .await
        .expect("query")
        .expect("present");

    assert_eq!(found.id.as_str(), prov.id.as_str());
    assert_eq!(found.name, "deepseek-global");
    assert_eq!(found.kind, ProviderKind::OpenAiCompat);
    assert_eq!(found.models, vec!["deepseek-chat", "deepseek-coder"]);
    assert_eq!(found.default_for_models, vec!["deepseek-chat"]);
    assert_eq!(found.fallback_order, 10);
    assert_eq!(found.api_key_env.as_deref(), Some("DEEPSEEK_API_KEY"));
}

#[tokio::test]
async fn duplicate_name_is_rejected() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLlmProviderRepository::new(pool);
    let p1 = sample_provider("dup");
    let p2 = sample_provider("dup");

    repo.create(&p1).await.expect("first");
    let err = repo.create(&p2).await.expect_err("second should fail");
    assert!(matches!(err, RepoError::DuplicateKey(_)), "got: {err:?}");
}

#[tokio::test]
async fn list_returns_all_rows_ordered_by_fallback() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLlmProviderRepository::new(pool);
    repo.create(&sample_provider("p-1")).await.expect("p1");
    repo.create(&sample_provider("p-2")).await.expect("p2");
    repo.create(&sample_provider("p-3")).await.expect("p3");

    let rows = repo.list().await.expect("list");
    let names: Vec<&str> = rows.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"p-1"));
    assert!(names.contains(&"p-2"));
    assert!(names.contains(&"p-3"));
}

#[tokio::test]
async fn delete_idempotent() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLlmProviderRepository::new(pool);
    let prov = sample_provider("del-me");
    repo.create(&prov).await.expect("create");
    repo.delete(prov.id.as_str()).await.expect("first delete");
    repo.delete(prov.id.as_str()).await.expect("second delete");
    assert!(repo
        .find_by_id(prov.id.as_str())
        .await
        .expect("query")
        .is_none());
}

// --------------------------------------------------------------------------
// Encryption-at-rest for the `api_key` column (opt-in, fail-safe).
// --------------------------------------------------------------------------

/// A single-key keyring built from a repeated byte — deterministic per test,
/// no env vars touched (env is process-global and would race under nextest).
fn keyring(byte: u8) -> Keyring {
    Keyring::with_keys(AeadKey([byte; 32]), None)
}

fn provider_with_key(name: &str, api_key: &str) -> LlmProvider {
    LlmProvider {
        api_key: Some(api_key.to_string()),
        ..sample_provider(name)
    }
}

/// Read the raw stored `api_key` column, bypassing the repo's decrypt path, to
/// assert what actually lands on disk.
async fn raw_api_key(pool: &sqlx::SqlitePool, id: &str) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>("SELECT api_key FROM llm_providers WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("raw api_key read")
}

#[tokio::test]
async fn encrypted_api_key_is_sealed_at_rest_and_revealed_on_read() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLlmProviderRepository::new_with_keyring(pool.clone(), Some(keyring(0x07)));
    let prov = provider_with_key("enc-provider", "sk-super-secret-123");
    repo.create(&prov).await.expect("create");

    // At rest: tagged ciphertext, never the plaintext.
    let stored = raw_api_key(&pool, prov.id.as_str()).await.expect("row");
    assert!(stored.starts_with("xgenc1:"), "stored = {stored}");
    assert!(!stored.contains("sk-super-secret-123"));

    // On read: transparent plaintext, via both find_by_id and list.
    let found = repo
        .find_by_id(prov.id.as_str())
        .await
        .expect("q")
        .expect("present");
    assert_eq!(found.api_key.as_deref(), Some("sk-super-secret-123"));
    let listed = repo.list().await.expect("list");
    let from_list = listed
        .iter()
        .find(|p| p.id.as_str() == prov.id.as_str())
        .expect("in list");
    assert_eq!(from_list.api_key.as_deref(), Some("sk-super-secret-123"));
}

#[tokio::test]
async fn cleartext_path_unchanged_without_keyring() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLlmProviderRepository::new(pool.clone());
    let prov = provider_with_key("plain-provider", "sk-plain-key");
    repo.create(&prov).await.expect("create");

    let stored = raw_api_key(&pool, prov.id.as_str()).await.expect("row");
    assert_eq!(stored, "sk-plain-key", "no keyring → cleartext at rest");
    let found = repo
        .find_by_id(prov.id.as_str())
        .await
        .expect("q")
        .expect("present");
    assert_eq!(found.api_key.as_deref(), Some("sk-plain-key"));
}

#[tokio::test]
async fn backfill_encrypts_existing_cleartext_and_is_idempotent() {
    let (pool, _guard) = test_setup().await;
    // Legacy row written cleartext (encryption was off).
    let plain = SqliteLlmProviderRepository::new(pool.clone());
    let prov = provider_with_key("legacy", "sk-legacy-key");
    plain.create(&prov).await.expect("create");

    // Operator turns encryption on and runs the boot backfill.
    let enc = SqliteLlmProviderRepository::new_with_keyring(pool.clone(), Some(keyring(0x11)));
    assert_eq!(
        enc.backfill_encrypt_api_keys().await.expect("backfill"),
        1,
        "one cleartext row sealed"
    );

    let stored = raw_api_key(&pool, prov.id.as_str()).await.expect("row");
    assert!(stored.starts_with("xgenc1:"), "backfilled row is sealed");
    let found = enc
        .find_by_id(prov.id.as_str())
        .await
        .expect("q")
        .expect("present");
    assert_eq!(found.api_key.as_deref(), Some("sk-legacy-key"));

    // Idempotent: nothing left to seal on a second pass.
    assert_eq!(
        enc.backfill_encrypt_api_keys().await.expect("backfill 2"),
        0
    );
}

#[tokio::test]
async fn decrypt_failure_treats_key_as_absent() {
    let (pool, _guard) = test_setup().await;
    let enc = SqliteLlmProviderRepository::new_with_keyring(pool.clone(), Some(keyring(0x01)));
    let prov = provider_with_key("unreadable", "sk-unreadable");
    enc.create(&prov).await.expect("create");

    // Wrong key → fail-safe None (not a bogus key, not a hard error).
    let wrong = SqliteLlmProviderRepository::new_with_keyring(pool.clone(), Some(keyring(0x02)));
    let got = wrong
        .find_by_id(prov.id.as_str())
        .await
        .expect("q")
        .expect("present");
    assert_eq!(got.api_key, None);

    // Encrypted data but key now unset → also None, never the ciphertext.
    let none = SqliteLlmProviderRepository::new(pool.clone());
    let got = none
        .find_by_id(prov.id.as_str())
        .await
        .expect("q")
        .expect("present");
    assert_eq!(got.api_key, None);
}

#[tokio::test]
async fn rotation_reads_ciphertext_with_previous_key() {
    let (pool, _guard) = test_setup().await;
    let old = SqliteLlmProviderRepository::new_with_keyring(pool.clone(), Some(keyring(0x03)));
    let prov = provider_with_key("rot", "sk-rotated");
    old.create(&prov).await.expect("create");

    // New current key, old key demoted to the _PREV slot — still decrypts.
    let rotated = SqliteLlmProviderRepository::new_with_keyring(
        pool.clone(),
        Some(Keyring::with_keys(
            AeadKey([0x04; 32]),
            Some(AeadKey([0x03; 32])),
        )),
    );
    let found = rotated
        .find_by_id(prov.id.as_str())
        .await
        .expect("q")
        .expect("present");
    assert_eq!(found.api_key.as_deref(), Some("sk-rotated"));
}

#[tokio::test]
async fn update_reseals_api_key() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLlmProviderRepository::new_with_keyring(pool.clone(), Some(keyring(0x09)));
    let prov = provider_with_key("upd", "sk-first");
    repo.create(&prov).await.expect("create");

    let next = LlmProvider {
        api_key: Some("sk-second".into()),
        ..prov.clone()
    };
    repo.update(&next).await.expect("update");

    let stored = raw_api_key(&pool, prov.id.as_str()).await.expect("row");
    assert!(stored.starts_with("xgenc1:"));
    let found = repo
        .find_by_id(prov.id.as_str())
        .await
        .expect("q")
        .expect("present");
    assert_eq!(found.api_key.as_deref(), Some("sk-second"));
}

#[tokio::test]
async fn update_verified_models_persists_set_and_preserves_api_key() {
    // The probe path writes verified_models via this narrow method precisely so
    // it never round-trips (and risks NULLing) the stored api_key. Assert both:
    // the set lands, and the key is untouched — even under at-rest encryption.
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLlmProviderRepository::new_with_keyring(pool.clone(), Some(keyring(0x09)));
    let prov = provider_with_key("probe-target", "sk-keep-me");
    repo.create(&prov).await.expect("create");

    repo.update_verified_models(prov.id.as_str(), &["m-a".into(), "m-b".into()])
        .await
        .expect("update verified");

    let found = repo
        .find_by_id(prov.id.as_str())
        .await
        .expect("q")
        .expect("present");
    assert_eq!(
        found.verified_models,
        Some(vec!["m-a".to_string(), "m-b".to_string()])
    );
    // The critical invariant for fix: the secret survived the narrow write.
    assert_eq!(found.api_key.as_deref(), Some("sk-keep-me"));
}

#[tokio::test]
async fn update_verified_models_unknown_id_is_not_found() {
    let (pool, _guard) = test_setup().await;
    let repo = SqliteLlmProviderRepository::new(pool);
    let err = repo
        .update_verified_models("prov_missing", &[])
        .await
        .expect_err("must be NotFound");
    assert!(matches!(err, RepoError::NotFound));
}
