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
#[macro_export]
macro_rules! instrument_llm_call {
    ($provider:expr, $model:expr, $fut:expr) => {{
        let _span =
            tracing::info_span!("llm.call", provider = $provider, model = $model,).entered();
        let __t0 = std::time::Instant::now();
        let __result = $fut.await;
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
