//! Example: watch AR aging for customers with DSO > 60.
//!
//! ## What this does
//!
//! Declares a [`WatchSpec`] that polls an `ar_aging` view for customers whose
//! Days-Sales-Outstanding has exceeded 60 days *and* were not alerted in the
//! last 24 hours.  Each qualifying row fires exactly once per day (TTL = 24 h).
//!
//! ## Integration hook for the scheduler
//!
//! The emitted [`WatchEvent`] has:
//! - `spec_id`  = `"ar-aging-dso60"`
//! - `on_match` = `ActionRef { action: "notify", target: "finance-ops" }`
//! - `payload`  = `{ "tenant_id": "...", "customer": "...", "dso": <f64> }`
//!
//! The scheduler integrator maps `action = "notify"` to whichever `PushSink`
//! is configured for `"finance-ops"` (Feishu, DingTalk, etc.).
//!
//! ## Running
//!
//! This example uses [`InMemorySource`] so no database is required:
//!
//! ```bash
//! cargo run -p xiaoguai-watch --example watch_ar_aging
//! ```
//!
//! To connect to a real Postgres instance swap [`InMemorySource`] for
//! [`SqlSource`] and pass `ReadWritePool::reader().clone()` from
//! `xiaoguai-storage`.

use std::time::Duration;

use serde_json::json;
use xiaoguai_watch::{
    ActionRef, DedupCache, InMemorySource, WatchEvent, WatchRunner, WatchSchedule, WatchSourceSpec,
    WatchSpec,
};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info,xiaoguai_watch=debug".into()),
        )
        .init();

    // ---------------------------------------------------------------------------
    // 1. Declare the watch spec (production: load from YAML config file)
    // ---------------------------------------------------------------------------
    //
    // Production SQL:
    //   SELECT tenant_id, customer, dso
    //     FROM ar_aging
    //    WHERE dso > 60
    //      AND last_alert < now() - interval '24 hours'
    //
    // The WHERE clause on `last_alert` prevents duplicate alerts at the DB
    // level.  The dedup cache provides a second, in-process layer of
    // protection for cases where the DB view is not available.
    let spec = WatchSpec {
        id: "ar-aging-dso60".into(),
        source: WatchSourceSpec::Sql {
            query: "\
                SELECT tenant_id, customer, dso \
                FROM ar_aging \
                WHERE dso > 60 \
                  AND last_alert < now() - interval '24 hours'\
            "
            .into(),
        },
        schedule: WatchSchedule::IntervalSecs {
            secs: 86_400, // poll once per day
        },
        on_match: ActionRef {
            action: "notify".into(),
            target: Some("finance-ops".into()),
            params: serde_json::Map::new(),
        },
    };

    spec.validate().expect("spec must be valid");

    // ---------------------------------------------------------------------------
    // 2. Wire up the source
    //    (production: SqlSource::new(rw_pool.reader().clone(), &spec.source))
    // ---------------------------------------------------------------------------
    let simulated_rows = vec![
        serde_json::from_value(json!({
            "tenant_id": "acme",
            "customer":  "Globex Corp",
            "dso":        72
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "tenant_id": "acme",
            "customer":  "Initech",
            "dso":        91
        }))
        .unwrap(),
    ];
    let source = InMemorySource::new(simulated_rows);

    // ---------------------------------------------------------------------------
    // 3. Build the runner with a 24-hour dedup TTL
    // ---------------------------------------------------------------------------
    let dedup = DedupCache::new(10_000, Duration::from_secs(86_400));
    let mut runner = WatchRunner::with_dedup(dedup);
    runner.register(spec, source);

    // ---------------------------------------------------------------------------
    // 4. Start and consume events
    // ---------------------------------------------------------------------------
    let mut rx = runner.run();

    println!("AR aging watch started.  Waiting for events (Ctrl-C to stop) …");

    // In production the integrator drives this loop; here we stop after 2 events.
    let mut count = 0usize;
    while let Some(event) = rx.recv().await {
        print_event(&event);
        count += 1;
        if count >= 2 {
            println!("Demo complete — {count} events received.");
            break;
        }
    }
}

fn print_event(event: &WatchEvent) {
    println!(
        "[{}] spec={} action={} target={} payload={}",
        event.fired_at.format("%Y-%m-%dT%H:%M:%SZ"),
        event.spec_id,
        event.on_match.action,
        event.on_match.target.as_deref().unwrap_or("-"),
        event.payload,
    );
}
