//! Test fault injection: named points where a test process exits
//! mid-operation so recovery can be exercised. Without test support every
//! point is a no-op.

/// A family of crash points, each selected by its own environment variable
/// and exiting with its own status.
#[derive(Clone, Copy)]
pub(crate) enum Crash {
    Tail,
    Snapshot,
    Peer,
}

impl Crash {
    /// Exits when this family's environment variable names `stage`.
    pub(crate) fn at(self, stage: &str) {
        self.when(|requested| requested == stage);
    }

    /// Exits when `matches` accepts the stage this family's environment
    /// variable names.
    pub(crate) fn when(self, matches: impl FnOnce(&str) -> bool) {
        #[cfg(any(test, feature = "test-support"))]
        {
            let (var, status) = match self {
                Self::Tail => ("AVEN_TAIL_CRASH", 84),
                Self::Snapshot => ("AVEN_SNAPSHOT_CRASH", 83),
                Self::Peer => ("AVEN_PEER_CRASH_KIND", 79),
            };
            if std::env::var(var).is_ok_and(|requested| matches(&requested)) {
                std::process::exit(status);
            }
        }
        #[cfg(not(any(test, feature = "test-support")))]
        let _ = (self, matches);
    }
}
