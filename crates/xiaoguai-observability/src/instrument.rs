//! Convenience macros that emit both a `tracing` span and a Prometheus
//! histogram observation for LLM calls and scheduler ticks.
//!
//! # Why macros instead of functions?
//!
//! `tracing` macros capture the call-site source location (file, line)
//! at compile time. Using a wrapper macro means span source location
//! points to the actual business-logic call site rather than into this
//! crate.
//!
//! # Examples
//!
//! ```rust,ignore
//! use xiaoguai_observability::instrument_llm_call;
//!
//! // Instrument an LLM call — records span + histogram.
//! let result = instrument_llm_call!("ollama", "qwen2.5", async {
//!     backend.chat(&messages).await
//! });
//! ```

/// Record an LLM call with `tracing` + Prometheus.
///
/// Parameters: `provider` (str literal), `model` (str literal), async
/// block that evaluates to the call result. The macro starts a timer
/// before the block, awaits it, and observes the elapsed time.
///
/// The macro is a no-op (passes the block through unchanged) when
/// `init_prometheus` was never called.
///
/// The future is instrumented via [`tracing::Instrument`] (enter/exit per
/// poll) rather than an [`EnteredSpan`](tracing::span::EnteredSpan) guard, so
/// the awaited future stays `Send` — required when the call site returns a
/// `Send` boxed future (e.g. the LLM router's `chat_stream`).
#[macro_export]
macro_rules! instrument_llm_call {
    ($provider:expr, $model:expr, $fut:expr) => {{
        use ::tracing::Instrument as _;
        let __span = ::tracing::info_span!("llm.call", provider = $provider, model = $model);
        let __t0 = std::time::Instant::now();
        let __result = $fut.instrument(__span).await;
        let __elapsed = __t0.elapsed().as_secs_f64();
        if let Some(__handles) = $crate::prometheus::global_handles() {
            __handles
                .llm_call_duration
                .with_label_values(&[$provider, $model])
                .observe(__elapsed);
        }
        __result
    }};
}

/// Record a scheduler tick with `tracing` + Prometheus.
///
/// Parameter: async block producing the tick result.
#[macro_export]
macro_rules! instrument_scheduler_tick {
    ($fut:expr) => {{
        let _span = tracing::info_span!("scheduler.tick").entered();
        let __t0 = std::time::Instant::now();
        let __result = $fut.await;
        let __elapsed = __t0.elapsed().as_secs_f64();
        if let Some(__handles) = $crate::prometheus::global_handles() {
            __handles.scheduler_tick_duration.observe(__elapsed);
        }
        __result
    }};
}

#[cfg(test)]
mod tests {
    use crate::prometheus::{global_handles, init_prometheus};

    #[tokio::test]
    async fn instrument_llm_call_observes_histogram_and_returns_value() {
        // Idempotent across the test binary — init only sets the global once.
        let _ = init_prometheus();
        let handles = global_handles().expect("handles set after init");
        let before = handles
            .llm_call_duration
            .with_label_values(&["test_prov", "test_model"])
            .get_sample_count();

        let out = instrument_llm_call!("test_prov", "test_model", async { 7_u32 });

        assert_eq!(out, 7, "macro must pass the future's value through");
        let after = handles
            .llm_call_duration
            .with_label_values(&["test_prov", "test_model"])
            .get_sample_count();
        assert_eq!(after, before + 1, "one histogram observation recorded");
    }
}
