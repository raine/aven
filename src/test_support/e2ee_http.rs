//! Servers, seeded clients and subprocess workers shared by the encrypted sync
//! HTTP transport tests.
use crate::protected_local_keys::{ProtectedLocalKeyStore, tests::isolated_store};
use aven_core::{
    db::Database,
    sync::{
        bootstrap_format::Package,
        seed_claim::{ClaimAuthentication, Secret, SeedAuthority, SetupAuthority},
    },
};
use axum::Router;
use std::path::Path;

pub(crate) fn setup() -> SetupAuthority {
    SetupAuthority::from_verifier(
        [9; 32],
        SetupAuthority::verifier([9; 32], &Secret::new([7; 32])),
    )
}

pub(crate) async fn fixture(
    root: &Path,
) -> (Database, ProtectedLocalKeyStore, SeedAuthority, Package) {
    fixture_with_domain(root, false).await
}

pub(crate) async fn fixture_with_domain(
    root: &Path,
    representative: bool,
) -> (Database, ProtectedLocalKeyStore, SeedAuthority, Package) {
    let db = Database::open(&root.join("client.sqlite")).await.unwrap();
    let workspace = db.list_workspaces().await.unwrap().remove(0);
    let task = db
        .create_task(
            &workspace,
            aven_core::operations::TaskDraft {
                title: "PRIVATE-HTTP-SEED-TASK".into(),
                description: String::new(),
                project: Some("app".into()),
                status: "todo".into(),
                priority: "none".into(),
                source: aven_core::choices::TaskSource::Cli,
                labels: vec![],
                metadata: vec![],
                available_at: None,
                due_on: None,
                is_epic: false,
            },
        )
        .await
        .unwrap()
        .task;
    let mut bytes = std::io::Cursor::new(Vec::new());
    let image = if representative {
        // Incompressible pixels exercise multi-kilobyte encrypted HTTP responses.
        let mut image = image::RgbaImage::new(256, 128);
        let mut state = 1_u32;
        for byte in image.as_mut() {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *byte = state as u8;
        }
        image
    } else {
        image::RgbaImage::new(3, 2)
    };
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    db.add_task_attachment(
        &workspace,
        root,
        Default::default(),
        &task.id,
        aven_core::operations::AttachmentAddInput {
            filename: Some("PRIVATE-HTTP-IMAGE.png".into()),
            alt_text: None,
            declared_media_type: None,
            bytes: bytes.into_inner(),
            optimization_policy: aven_core::attachments::ImageOptimizationPolicy::Preserve,
            dedupe_existing: false,
        },
    )
    .await
    .unwrap();
    if representative {
        representative_domain(&db, &workspace, &task.id).await;
    }
    let store = isolated_store(&db, &root.join("keys")).await;
    let seed = store.prepare_seed_claim(&db, [9; 32]).await.unwrap();
    store.prepare_seed_source(&db).await.unwrap();
    db.capture_local_shared_state_never_dispatched()
        .await
        .unwrap();
    let package = store
        .package_seed_capture(&db, root, [9; 32])
        .await
        .unwrap()
        .upload_package();
    if !representative {
        db.update_task(
            &workspace,
            &task.id,
            aven_core::operations::TaskUpdate {
                title: Some("AFTER-CAPTURE".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    (db, store, seed, package)
}

async fn representative_domain(
    db: &Database,
    ws: &aven_core::workspaces::Workspace,
    task: &aven_core::ids::TaskId,
) {
    use aven_core::{operations::*, recurrence::*};
    use chrono::{TimeZone, Utc};
    let epic = db
        .create_task(
            ws,
            TaskDraft {
                title: "epic".into(),
                description: "".into(),
                project: Some("app".into()),
                status: "todo".into(),
                priority: "none".into(),
                source: aven_core::choices::TaskSource::Cli,
                labels: vec![],
                metadata: vec![],
                available_at: None,
                due_on: None,
                is_epic: true,
            },
        )
        .await
        .unwrap()
        .task;
    db.add_task_to_epic(ws, task, &epic.id).await.unwrap();
    db.add_task_related_link(ws, task, &epic.id).await.unwrap();
    for title in ["alpha", "beta"] {
        db.update_task(
            ws,
            task,
            TaskUpdate {
                title: Some(title.into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    let at = Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap();
    db.create_recurrence_series(
        ws,
        CreateRecurrenceSeriesParams::new(RecurrenceSeriesDraft {
            title: "daily".into(),
            description: "template".into(),
            project: "app".into(),
            priority: "none".into(),
            initial_status: "todo".into(),
            labels: vec![],
            metadata: vec![],
            schedule: RecurrenceSchedule::new(
                RecurrenceRule::daily(),
                "UTC".parse().unwrap(),
                at.date_naive(),
                None,
                RecurrenceDuePolicy::SameDay,
            ),
        })
        .at(at),
    )
    .await
    .unwrap();
    // Synthetic retained conflict: this fixture tests preservation, not conflict generation.
    let mut conn = aven_core::test_support::acquire(db).await.unwrap();
    let changes: Vec<String> = sqlx::query_scalar("SELECT change_id FROM changes WHERE entity_id=? AND field='title' ORDER BY local_seq DESC LIMIT 2").bind(task).fetch_all(&mut *conn).await.unwrap();
    sqlx::query("INSERT INTO conflicts(workspace_id,entity_type,entity_id,task_id,field,base_version,local_value,remote_value,local_change_id,remote_change_id,variant_a,variant_b,created_at,resolved) VALUES(?,'task',?,?,'title',NULL,'beta','remote',?,?,'variant-a','variant-b','2026-09-21T12:30:00Z',0)")
        .bind(&ws.id).bind(task).bind(task).bind(&changes[0]).bind(&changes[1]).execute(&mut *conn).await.unwrap();
}

/// Every encrypted sync route, with `tail` serving the tail and image routes.
pub(crate) fn router_with_tail(db: Database, tail: Router) -> Router {
    crate::seed_bootstrap_http::router(db.clone(), Some(setup()), Default::default())
        .merge(crate::peer_enrollment_http::router(db))
        .merge(tail)
}

pub(crate) fn router(db: Database) -> Router {
    router_with_tail(db.clone(), crate::encrypted_tail_http::router(db))
}

/// Serves `app` on `address` and returns its origin.
pub(crate) async fn serve(app: Router, address: &str) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (origin, task)
}

/// Claims the fixture seed with the `setup` secret and uploads its bootstrap.
pub(crate) async fn adopt(
    origin: &str,
    db: &Database,
    store: &ProtectedLocalKeyStore,
    seed: &SeedAuthority,
) {
    let bootstrap = crate::seed_bootstrap_http::Client::new(origin).unwrap();
    bootstrap
        .claim(
            seed.genesis(),
            ClaimAuthentication::SetupSecret(&Secret::new([7; 32])),
        )
        .await
        .unwrap();
    bootstrap.resume(store, db).await.unwrap();
}

/// Runs the ignored test `name` of this test binary in a subprocess.
pub(crate) fn worker(name: &str) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", name, "--ignored", "--nocapture"]);
    command
}
