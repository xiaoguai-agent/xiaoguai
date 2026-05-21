//! Cache integration tests against an ephemeral Valkey/Redis container.
//!
//! All tests are gated behind `#[ignore]` since they require Docker. Run with
//! `cargo test -p xiaoguai-storage --test cache -- --ignored`.

#[cfg(test)]
mod containerized {
    use std::time::Duration;

    use serde::{Deserialize, Serialize};
    use testcontainers_modules::redis::Redis;
    use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync};
    use xiaoguai_storage::cache::Cache;

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Sample {
        id: u32,
        name: String,
    }

    /// Boot a Redis container and return a connected `Cache` plus the
    /// container handle (kept alive for the test's lifetime).
    async fn setup() -> (Cache, ContainerAsync<Redis>) {
        let container = Redis::default().start().await.expect("start redis");
        let port = container
            .get_host_port_ipv4(6379)
            .await
            .expect("redis port");
        let url = format!("redis://127.0.0.1:{port}/0");
        let cache = Cache::connect(&url, "test:").await.expect("connect cache");
        (cache, container)
    }

    #[tokio::test]
    #[ignore = "requires Docker"]
    async fn set_and_get_roundtrip() {
        let (cache, _c) = setup().await;
        let v = Sample {
            id: 42,
            name: "alpha".into(),
        };
        cache.set("user/1", &v, None).await.expect("set");
        let got: Option<Sample> = cache.get("user/1").await.expect("get");
        assert_eq!(got, Some(v));
    }

    #[tokio::test]
    #[ignore = "requires Docker"]
    async fn get_missing_returns_none() {
        let (cache, _c) = setup().await;
        let got: Option<Sample> = cache.get("nope").await.expect("get");
        assert!(got.is_none());
    }

    #[tokio::test]
    #[ignore = "requires Docker"]
    async fn set_with_ttl_expires() {
        let (cache, _c) = setup().await;
        let v = Sample {
            id: 1,
            name: "ttl".into(),
        };
        cache
            .set("ephemeral", &v, Some(Duration::from_secs(1)))
            .await
            .expect("set with ttl");
        assert!(cache.exists("ephemeral").await.expect("exists"));
        // Wait past the TTL boundary; Redis EX is second-granular.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!cache.exists("ephemeral").await.expect("exists after ttl"));
    }

    #[tokio::test]
    #[ignore = "requires Docker"]
    async fn incr_from_missing_treats_nil_as_zero() {
        let (cache, _c) = setup().await;
        let n = cache.incr("counter", 7).await.expect("incr");
        assert_eq!(n, 7);
        let n2 = cache.incr("counter", 3).await.expect("incr 2");
        assert_eq!(n2, 10);
    }

    #[tokio::test]
    #[ignore = "requires Docker"]
    async fn tenant_scope_isolates_keys() {
        let (cache, _c) = setup().await;
        let t1 = cache.tenant_scope("tenant-a");
        let t2 = cache.tenant_scope("tenant-b");

        let va = Sample {
            id: 1,
            name: "a".into(),
        };
        let vb = Sample {
            id: 2,
            name: "b".into(),
        };
        t1.set("profile", &va, None).await.expect("set a");
        t2.set("profile", &vb, None).await.expect("set b");

        let got_a: Option<Sample> = t1.get("profile").await.expect("get a");
        let got_b: Option<Sample> = t2.get("profile").await.expect("get b");
        assert_eq!(got_a, Some(va));
        assert_eq!(got_b, Some(vb));
    }

    #[tokio::test]
    #[ignore = "requires Docker"]
    async fn delete_returns_true_for_existing_false_for_missing() {
        let (cache, _c) = setup().await;
        let v = Sample {
            id: 1,
            name: "x".into(),
        };
        cache.set("doomed", &v, None).await.expect("set");
        assert!(cache.delete("doomed").await.expect("delete existing"));
        assert!(!cache.delete("doomed").await.expect("delete missing"));
        assert!(!cache.delete("never").await.expect("delete absent"));
    }

    #[tokio::test]
    #[ignore = "requires Docker"]
    async fn expire_refreshes_ttl_on_existing_key() {
        let (cache, _c) = setup().await;
        let v = Sample {
            id: 1,
            name: "live".into(),
        };
        cache.set("persisted", &v, None).await.expect("set");
        let applied = cache
            .expire("persisted", Duration::from_secs(60))
            .await
            .expect("expire");
        assert!(applied);

        // Expire on missing key returns false.
        let applied_missing = cache
            .expire("absent", Duration::from_secs(60))
            .await
            .expect("expire missing");
        assert!(!applied_missing);
    }
}
