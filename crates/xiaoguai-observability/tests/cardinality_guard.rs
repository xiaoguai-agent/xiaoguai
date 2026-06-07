//! Regression test for the model-label cardinality guard on the
//! instrumented LLM call path.
//!
//! Lives in its own integration binary (own process) on purpose: it floods
//! the process-wide guard registries to the cap, which would poison any
//! parallel unit test in the library binary that expects pass-through.
//! Single test function for the same reason — phases must stay ordered.

use xiaoguai_observability::cardinality::{
    bounded_model_label, bounded_provider_label, LABEL_CARDINALITY_CAP, OTHER_LABEL,
};
use xiaoguai_observability::instrument_llm_call;
use xiaoguai_observability::prometheus::{global_handles, init_prometheus};

/// Flood the process-wide model registry to the cap, then assert the
/// instrumented hot path folds a fresh model name into the `_other`
/// series — while a model admitted before the cap keeps its own series.
#[tokio::test]
async fn instrument_llm_call_bounds_model_label_cardinality() {
    let _ = init_prometheus();
    let handles = global_handles().expect("handles set after init");

    // Phase 1: admit a provider and one model while under the caps, then
    // fill the model registry to its cap.
    assert_eq!(bounded_provider_label("guard_prov"), "guard_prov");
    assert_eq!(bounded_model_label("admitted-model"), "admitted-model");
    for i in 0..LABEL_CARDINALITY_CAP {
        bounded_model_label(&format!("flood-model-{i}"));
    }

    // Phase 2: a brand-new model past the cap must land on `_other`.
    let before = handles
        .llm_call_duration
        .with_label_values(&["guard_prov", OTHER_LABEL])
        .get_sample_count();

    let out = instrument_llm_call!("guard_prov", "brand-new-over-cap-model", async { 42_u32 });

    assert_eq!(out, 42, "macro must pass the future's value through");
    let after = handles
        .llm_call_duration
        .with_label_values(&["guard_prov", OTHER_LABEL])
        .get_sample_count();
    assert_eq!(
        after,
        before + 1,
        "over-cap model must be observed under the _other series"
    );

    // Phase 3: the model admitted before the cap keeps its verbatim series.
    let before = handles
        .llm_call_duration
        .with_label_values(&["guard_prov", "admitted-model"])
        .get_sample_count();

    let out = instrument_llm_call!("guard_prov", "admitted-model", async { 7_u32 });

    assert_eq!(out, 7);
    let after = handles
        .llm_call_duration
        .with_label_values(&["guard_prov", "admitted-model"])
        .get_sample_count();
    assert_eq!(
        after,
        before + 1,
        "admitted model must keep its verbatim series after the cap"
    );
}
