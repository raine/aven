//! Opt-in startup milestones, written to stderr without task data.
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn clock() -> Option<&'static Instant> {
    static START: OnceLock<Option<Instant>> = OnceLock::new();
    START
        .get_or_init(|| std::env::var_os("AVEN_STARTUP_TIMING").map(|_| Instant::now()))
        .as_ref()
}

pub fn enabled() -> bool {
    clock().is_some()
}

pub fn mark(stage: &str) {
    let Some(start) = clock() else {
        return;
    };
    eprintln!(
        "aven-startup {stage} elapsed_ms={:.3} unix_ms={:.3}",
        start.elapsed().as_secs_f64() * 1000.,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64()
            * 1000.
    );
}
