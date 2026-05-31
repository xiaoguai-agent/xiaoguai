//! Process-level singleton `wasmtime::Engine` with an epoch-tick thread.
//!
//! Both the Python and JavaScript backends share the same engine so the
//! cold-start cache (precompiled modules) survives across language
//! switches at process level.
//!
//! ## Epoch interruption, not deadlines
//!
//! User constraint (and ADR-0020 §"design drivers"): pyodide's CPython
//! contains tight syscall loops that cannot afford to check the
//! wasmtime deadline on every instruction. We therefore use **epoch
//! interruption**: a single thread increments the engine epoch every
//! 10 ms, and each per-call `Store` is configured with an epoch
//! deadline equal to `ticks_for_secs(timeout)`. When the deadline
//! ticks past, wasmtime traps on the next epoch check (cheaper than
//! a deadline branch on every instruction).

use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use wasmtime::{Config, Engine};

/// Length of one epoch tick. 10 ms gives a 100 Hz heartbeat — fine
/// granularity for our 1 s to 60 s timeout range.
const TICK_INTERVAL_MS: u64 = 10;

/// Get the shared process-level engine. First call spawns the tick
/// thread; subsequent calls are O(1).
///
/// # Panics
///
/// If wasmtime fails to construct the engine. This is a startup-time
/// configuration error (e.g. corrupt cranelift install) — there is no
/// recovery, so we panic to surface it loudly rather than degrading
/// silently.
pub fn shared_engine() -> &'static Engine {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut cfg = Config::new();
        cfg.epoch_interruption(true);
        cfg.consume_fuel(false);
        // `async_support` was a no-op getter/setter in older wasmtime and is
        // deprecated in 42.x; async is opt-in via `Linker::*_async` / `Store`
        // futures, no engine-level toggle needed.
        let engine = Engine::new(&cfg).expect("wasmtime engine init");
        spawn_tick_thread(engine.clone());
        engine
    })
}

/// Convert a timeout in **whole seconds** to epoch ticks.
///
/// One tick = 10 ms; one second = 100 ticks. Saturating multiply so a
/// pathological `u64::MAX` input doesn't overflow into a near-zero
/// deadline.
#[must_use]
pub fn ticks_for_secs(secs: u64) -> u64 {
    secs.saturating_mul(1000 / TICK_INTERVAL_MS)
}

/// Spawn the singleton tick thread. Called once from `shared_engine`'s
/// `OnceLock::get_or_init`.
fn spawn_tick_thread(engine: Engine) {
    thread::Builder::new()
        .name("xg-wasm-epoch-tick".into())
        .spawn(move || loop {
            thread::sleep(Duration::from_millis(TICK_INTERVAL_MS));
            engine.increment_epoch();
        })
        .expect("spawn epoch tick thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_engine_is_singleton() {
        let e1 = shared_engine();
        let e2 = shared_engine();
        // wasmtime::Engine is `Clone` (cheap Arc); we compare via
        // `Engine::same` which is the documented identity check.
        assert!(Engine::same(e1, e2));
    }

    #[test]
    fn ticks_for_secs_converts_correctly() {
        assert_eq!(ticks_for_secs(0), 0);
        assert_eq!(ticks_for_secs(1), 100);
        assert_eq!(ticks_for_secs(30), 3000);
        // Saturating multiply on giant input.
        assert_eq!(ticks_for_secs(u64::MAX), u64::MAX);
    }

    #[test]
    fn epoch_thread_does_not_panic_on_init() {
        // Force first access; the thread spawn happens inside the
        // get_or_init closure. If the spawn panicked we'd never get
        // here.
        let _ = shared_engine();
        // Give the tick thread a moment to do its first increment.
        std::thread::sleep(Duration::from_millis(25));
    }
}
