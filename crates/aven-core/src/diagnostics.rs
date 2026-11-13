//! Operation-scoped phase reports for host diagnostics.
//!
//! A host passes an observer to one operation, such as opening a database or
//! one sync session. The operation reports typed phases before entering
//! potentially blocking work, so a host that sees no further report knows
//! which phase has not returned. Reports carry only a closed phase and a
//! phase-specific counter: never identifiers, paths, SQL, URLs or messages.
//!
//! Observers are never global. Each operation carries its own handle, so
//! concurrent operations sharing one client cannot misattribute phases.
//! Observer failures and panics never change the operation's outcome.
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

/// A phase an observed operation is entering or has completed.
///
/// Counters are phase-specific:
/// - `DatabaseOpen`, `DatabaseMigration`: always 0.
/// - `SessionStep`: 1-based count of host `next` calls in the session.
/// - `DrainRound`: 1-based round within the drain.
/// - `MetadataDecode`: records in the page just received.
/// - `MetadataApply`: records in the page about to be applied.
/// - `TransactionCommit`: records in the page whose apply transaction
///   returned successfully.
/// - `ImageUpload`: encrypted chunks in the image about to be declared.
/// - `ImageDownload`: images already transferred in this round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticPhase {
    DatabaseOpen,
    DatabaseMigration,
    SessionStep,
    DrainRound,
    MetadataDecode,
    MetadataApply,
    TransactionCommit,
    ImageUpload,
    ImageDownload,
}

/// A closed failure class with a reviewed numeric code where one exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticFailure {
    /// SQLite primary or extended result code.
    Sqlite(i32),
    /// Operating system error number.
    Os(i32),
    /// No reviewed code was available.
    Other,
}

/// Receives phase reports. Implementations must return promptly, must not
/// call back into the reporting operation and must not block.
pub trait DiagnosticObserver: Send + Sync {
    fn phase(&self, phase: DiagnosticPhase, counter: u64);

    /// The operation failed with this classified cause.
    fn failure(&self, _failure: DiagnosticFailure) {}
}

/// One operation's optional observer. The default reports nothing.
#[derive(Clone, Default)]
pub struct Diagnostics(Option<Arc<dyn DiagnosticObserver>>);

impl Diagnostics {
    pub fn new(observer: Arc<dyn DiagnosticObserver>) -> Self {
        Self(Some(observer))
    }

    pub fn is_enabled(&self) -> bool {
        self.0.is_some()
    }

    /// Reports `phase`; a panicking observer is contained.
    pub fn phase(&self, phase: DiagnosticPhase, counter: u64) {
        if let Some(observer) = &self.0 {
            let _ = catch_unwind(AssertUnwindSafe(|| observer.phase(phase, counter)));
        }
    }

    /// Reports `error`'s classified cause; a panicking observer is contained.
    pub fn failed(&self, error: &anyhow::Error) {
        if let Some(observer) = &self.0 {
            let failure = failure(error);
            let _ = catch_unwind(AssertUnwindSafe(|| observer.failure(failure)));
        }
    }
}

impl std::fmt::Debug for Diagnostics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Diagnostics")
            .field(&self.is_enabled())
            .finish()
    }
}

/// Classifies the first storage cause in `error`'s chain by code alone.
pub fn failure(error: &anyhow::Error) -> DiagnosticFailure {
    for cause in error.chain() {
        if let Some(sqlx::Error::Database(database)) = cause.downcast_ref::<sqlx::Error>()
            && let Some(code) = database.code().and_then(|code| code.parse::<i32>().ok())
        {
            return DiagnosticFailure::Sqlite(code);
        }
        if let Some(sqlx::Error::Io(io)) = cause.downcast_ref::<sqlx::Error>()
            && let Some(code) = io.raw_os_error()
        {
            return DiagnosticFailure::Os(code);
        }
        if let Some(code) = cause
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::raw_os_error)
        {
            return DiagnosticFailure::Os(code);
        }
    }
    DiagnosticFailure::Other
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recorder(
        Mutex<Vec<(DiagnosticPhase, u64)>>,
        Mutex<Vec<DiagnosticFailure>>,
    );

    impl DiagnosticObserver for Recorder {
        fn phase(&self, phase: DiagnosticPhase, counter: u64) {
            self.0.lock().unwrap().push((phase, counter));
        }

        fn failure(&self, failure: DiagnosticFailure) {
            self.1.lock().unwrap().push(failure);
        }
    }

    struct Panicking;

    impl DiagnosticObserver for Panicking {
        fn phase(&self, _: DiagnosticPhase, _: u64) {
            panic!("observer failure");
        }
    }

    #[test]
    fn disabled_diagnostics_report_nothing_and_enabled_reports_in_order() {
        Diagnostics::default().phase(DiagnosticPhase::DatabaseOpen, 0);
        let recorder = Arc::new(Recorder::default());
        let diagnostics = Diagnostics::new(recorder.clone());
        diagnostics.phase(DiagnosticPhase::DrainRound, 1);
        diagnostics.phase(DiagnosticPhase::DrainRound, 2);
        assert_eq!(
            *recorder.0.lock().unwrap(),
            [
                (DiagnosticPhase::DrainRound, 1),
                (DiagnosticPhase::DrainRound, 2)
            ]
        );
    }

    #[test]
    fn a_panicking_observer_does_not_escape() {
        Diagnostics::new(Arc::new(Panicking)).phase(DiagnosticPhase::SessionStep, 1);
    }

    #[test]
    fn failure_codes_come_from_typed_causes_not_messages() {
        let io = anyhow::Error::new(std::io::Error::from_raw_os_error(28)).context("disk /secret");
        assert_eq!(failure(&io), DiagnosticFailure::Os(28));
        assert_eq!(
            failure(&anyhow::anyhow!("sqlite error 5 at /secret")),
            DiagnosticFailure::Other
        );
    }

    #[tokio::test]
    async fn database_open_reports_open_before_migration_and_stops_at_failure() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = Arc::new(Recorder::default());
        let diagnostics = Diagnostics::new(recorder.clone());
        let database =
            crate::db::Database::open_observed(&directory.path().join("a.sqlite"), &diagnostics)
                .await
                .unwrap();
        database.close().await;
        assert_eq!(
            *recorder.0.lock().unwrap(),
            [
                (DiagnosticPhase::DatabaseOpen, 0),
                (DiagnosticPhase::DatabaseMigration, 0)
            ]
        );

        let blocked = Arc::new(Recorder::default());
        let file = directory.path().join("file");
        std::fs::write(&file, b"").unwrap();
        let error = crate::db::Database::open_observed(
            &file.join("b.sqlite"),
            &Diagnostics::new(blocked.clone()),
        )
        .await
        .err()
        .expect("a database under a regular file cannot open");
        assert_eq!(
            *blocked.0.lock().unwrap(),
            [(DiagnosticPhase::DatabaseOpen, 0)]
        );
        assert!(matches!(
            failure(&error),
            DiagnosticFailure::Os(_) | DiagnosticFailure::Other
        ));
    }

    #[tokio::test]
    async fn store_open_reports_the_classified_failure_once() {
        let directory = tempfile::tempdir().unwrap();
        let recorder = Arc::new(Recorder::default());
        let diagnostics = Diagnostics::new(recorder.clone());
        // A directory where the database file belongs cannot be opened as SQLite.
        let path = directory.path().join("db");
        std::fs::create_dir(&path).unwrap();
        assert!(
            crate::api::Store::open_observed(&path, &diagnostics)
                .await
                .is_err()
        );
        assert_eq!(recorder.1.lock().unwrap().len(), 1);
        assert_eq!(
            recorder.0.lock().unwrap().first(),
            Some(&(DiagnosticPhase::DatabaseOpen, 0))
        );
    }

    #[tokio::test]
    async fn sqlite_failures_keep_their_result_code() {
        let directory = tempfile::tempdir().unwrap();
        let database = crate::db::Database::open(&directory.path().join("a.sqlite"))
            .await
            .unwrap();
        let error = anyhow::Error::new(
            sqlx::query("INSERT INTO missing_table VALUES (1)")
                .execute(database.pool())
                .await
                .unwrap_err(),
        );
        assert_eq!(failure(&error), DiagnosticFailure::Sqlite(1));
    }
}
