use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Cursor};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use aven_core::attachments::{ImageOptimizationPolicy, LifecyclePolicy, default_blob_dir};
use aven_core::choices::TaskSource;
use aven_core::db::Database;
use aven_core::operations::{
    AttachmentAddInput, CreateRecurrenceSeriesParams, RecurrenceSeriesDraft, TaskDraft, TaskUpdate,
};
use aven_core::recurrence::{
    RecurrenceDuePolicy, RecurrenceOutcome, RecurrenceRule, RecurrenceSchedule, TimeZoneId,
};
use aven_core::sync::wire::MAX_BLOB_TRANSFER_BYTES;
use aven_core::sync::{
    PreparedSyncRequest, SyncHttpHeader, SyncHttpResponse, SyncRetryDecision, SyncSession,
    SyncSessionSummary,
};
use chrono::{NaiveDate, Utc};
use image::{DynamicImage, ImageFormat, RgbaImage};
use serde::Serialize;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, Row, SqliteConnection};

const SCHEMA_VERSION: u32 = 1;
const FIXED_DATE: &str = "2026-01-01";

#[derive(Clone, Copy, Serialize)]
struct Profile {
    name: &'static str,
    ordinary_tasks: usize,
    scalar_update_rounds: usize,
    recurrence_series: usize,
    outcomes_per_series: usize,
    attachments: usize,
    image_width: u32,
    image_height: u32,
    conflict_tasks: usize,
}

impl Profile {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "smoke" => Ok(SMOKE),
            "report" => Ok(REPORT),
            _ => bail!("unknown profile {value:?}; expected smoke or report"),
        }
    }
}

const SMOKE: Profile = Profile {
    name: "smoke",
    ordinary_tasks: 32,
    scalar_update_rounds: 2,
    recurrence_series: 2,
    outcomes_per_series: 1,
    attachments: 2,
    image_width: 32,
    image_height: 32,
    conflict_tasks: 2,
};

const REPORT: Profile = Profile {
    name: "report",
    ordinary_tasks: 5_000,
    scalar_update_rounds: 4,
    recurrence_series: 64,
    outcomes_per_series: 1,
    attachments: 32,
    image_width: 256,
    image_height: 256,
    conflict_tasks: 64,
};

#[derive(Default, Serialize)]
struct SyncMeasurement {
    duration_ms: u128,
    sessions: usize,
    complete: bool,
    pages: usize,
    pushed: i64,
    pulled: usize,
    request_bytes: usize,
    request_wire_bytes: usize,
    response_decoded_bytes: usize,
    apply_ms: u128,
    blob_uploaded: usize,
    blob_uploaded_bytes: u64,
    blob_downloaded: usize,
    blob_downloaded_bytes: u64,
}

impl SyncMeasurement {
    fn absorb(&mut self, summary: &SyncSessionSummary) {
        self.sessions += 1;
        self.pages += summary.pages;
        self.pushed += summary.pushed;
        self.pulled += summary.pulled;
        self.request_bytes += summary.request_bytes;
        self.request_wire_bytes += summary.request_wire_bytes;
        self.response_decoded_bytes += summary.response_decoded_bytes;
        self.apply_ms += summary.apply_ms;
        self.blob_uploaded += summary.blob_uploaded;
        self.blob_uploaded_bytes += summary.blob_uploaded_bytes;
        self.blob_downloaded += summary.blob_downloaded;
        self.blob_downloaded_bytes += summary.blob_downloaded_bytes;
    }
}

#[derive(Serialize)]
struct Stages {
    initial_push_ack: SyncMeasurement,
    peer_bootstrap: SyncMeasurement,
    steady_state_push_ack: SyncMeasurement,
    conflicting_device_sync: SyncMeasurement,
    source_convergence: SyncMeasurement,
    fresh_replica_replay: SyncMeasurement,
}

#[derive(Serialize)]
struct DatabaseSize {
    baseline_main_bytes: u64,
    baseline_wal_bytes: u64,
    final_main_bytes: u64,
    final_wal_bytes: u64,
    growth_bytes: u64,
}

#[derive(Serialize)]
struct DatabaseGrowth {
    source: DatabaseSize,
    server: DatabaseSize,
    fresh: DatabaseSize,
}

#[derive(Serialize)]
struct OperationLog {
    change_rows: i64,
    payload_bytes: i64,
    by_operation: BTreeMap<String, i64>,
}

#[derive(Serialize)]
struct OperationLogs {
    source: OperationLog,
    server: OperationLog,
    fresh: OperationLog,
    server_growth_bytes_per_change: f64,
    projected_server_bytes_at_250k_changes: u64,
}

#[derive(Serialize)]
struct Verification {
    source_open_title_conflicts: i64,
    peer_open_title_conflicts: i64,
    fresh_open_title_conflicts: i64,
    fresh_tasks: i64,
    fresh_recurrence_series: i64,
    fresh_live_attachments: i64,
    server_distinct_referenced_blob_bytes: i64,
    fresh_distinct_blob_bytes: i64,
}

#[derive(Serialize)]
struct DerivedMetrics {
    initial_own_change_pull_ratio: f64,
    steady_state_own_change_pull_ratio: f64,
    initial_ack_response_bytes_per_push: f64,
    steady_state_ack_response_bytes_per_push: f64,
}

#[derive(Serialize)]
struct Report {
    schema_version: u32,
    profile: Profile,
    fixed_date: &'static str,
    metric_notes: BTreeMap<&'static str, &'static str>,
    stages: Stages,
    database_growth: DatabaseGrowth,
    operation_logs: OperationLogs,
    verification: Verification,
    derived: DerivedMetrics,
}

enum Invocation {
    Run {
        profile: Profile,
        output: Option<PathBuf>,
    },
    Noop {
        quiet: bool,
    },
}

fn parse_args() -> Result<Invocation> {
    let mut profile = None;
    let mut output = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bench" | "--test" | "--ignored" | "--nocapture" | "--show-output" | "--exact" => {}
            "--format" => {
                let _ = args.next();
            }
            value if value.starts_with("--format=") => {}
            "--list" => return Ok(Invocation::Noop { quiet: true }),
            "--profile" => {
                profile = Some(Profile::parse(
                    &args.next().context("--profile needs a value")?,
                )?);
            }
            "--output" => {
                output = Some(PathBuf::from(
                    args.next().context("--output needs a value")?,
                ));
            }
            other => bail!("unknown benchmark argument {other}"),
        }
    }
    Ok(match profile {
        Some(profile) => Invocation::Run { profile, output },
        None => Invocation::Noop { quiet: false },
    })
}

struct BenchServer {
    child: Option<Child>,
    output: Arc<Mutex<String>>,
    readers: Vec<JoinHandle<()>>,
    url: String,
}

impl BenchServer {
    fn start(root: &Path, data_path: &Path) -> Result<Self> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_aven"));
        command
            .args(["server", "--bind", "127.0.0.1:0", "--data"])
            .arg(data_path)
            .env("AVEN_CONFIG_DIR", root.join("config").join("aven"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env_remove("AVEN_DB")
            .env_remove("AVEN_DEV_DB")
            .env_remove("AVEN_SYNC_SERVER")
            .env_remove("AVEN_SYNC_DISABLED")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().context("spawn benchmark sync server")?;
        let stdout = child.stdout.take().context("capture server stdout")?;
        let stderr = child.stderr.take().context("capture server stderr")?;
        let output = Arc::new(Mutex::new(String::new()));
        let (url_tx, url_rx) = mpsc::channel();

        let stdout_output = Arc::clone(&output);
        let stdout_reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if let Some(value) = line.strip_prefix("listening url=")
                    && let Some(url) = value.split_whitespace().next()
                {
                    let _ = url_tx.send(url.to_string());
                }
                let mut captured = stdout_output.lock().expect("server output lock");
                captured.push_str(&line);
                captured.push('\n');
            }
        });
        let stderr_output = Arc::clone(&output);
        let stderr_reader = thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let mut captured = stderr_output.lock().expect("server output lock");
                captured.push_str(&line);
                captured.push('\n');
            }
        });

        let url = url_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|error| {
                anyhow::anyhow!(
                    "benchmark server did not announce a URL: {error}\n{}",
                    output.lock().expect("server output lock")
                )
            })?;
        Ok(Self {
            child: Some(child),
            output,
            readers: vec![stdout_reader, stderr_reader],
            url,
        })
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

impl Drop for BenchServer {
    fn drop(&mut self) {
        self.stop();
        if std::thread::panicking() {
            eprintln!(
                "benchmark server output:\n{}",
                self.output.lock().expect("server output lock")
            );
        }
    }
}

fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .retry(reqwest::retry::never())
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .build()?)
}

async fn send_prepared(
    client: &reqwest::Client,
    prepared: &PreparedSyncRequest,
) -> Result<SyncHttpResponse> {
    let method = reqwest::Method::from_bytes(prepared.method.as_bytes())?;
    let mut request = client
        .request(method, &prepared.url)
        .timeout(Duration::from_millis(prepared.timeout.attempt_ms))
        .body(prepared.body.clone());
    for header in &prepared.headers {
        request = request.header(&header.name, &header.value);
    }
    let mut response = request.send().await?;
    let status = response.status().as_u16();
    let headers = [
        reqwest::header::CONTENT_ENCODING,
        reqwest::header::CONTENT_LENGTH,
        reqwest::header::CONTENT_TYPE,
        reqwest::header::RETRY_AFTER,
    ]
    .into_iter()
    .filter_map(|name| {
        response
            .headers()
            .get(&name)
            .and_then(|value| value.to_str().ok())
            .map(|value| SyncHttpHeader {
                name: name.as_str().to_string(),
                value: value.to_string(),
            })
    })
    .collect();
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BLOB_TRANSFER_BYTES)
    {
        bail!("error sync-response-too-large");
    }
    let limit = usize::try_from(MAX_BLOB_TRANSFER_BYTES)?;
    let inactivity = Duration::from_millis(prepared.timeout.inactivity_ms);
    let mut body = Vec::new();
    loop {
        let chunk = tokio::time::timeout(inactivity, response.chunk())
            .await
            .map_err(|_| anyhow::anyhow!("sync response stalled"))??;
        let Some(chunk) = chunk else {
            break;
        };
        ensure!(
            body.len().saturating_add(chunk.len()) <= limit,
            "error sync-response-too-large"
        );
        body.extend_from_slice(&chunk);
    }
    Ok(SyncHttpResponse {
        status,
        headers,
        body,
    })
}

async fn drive_session(session: &mut SyncSession, client: &reqwest::Client) -> Result<()> {
    loop {
        let Some(prepared) = session.prepare_request().await? else {
            break;
        };
        let response = loop {
            match send_prepared(client, &prepared).await {
                Ok(response) if !(200..300).contains(&response.status) => {
                    match session.register_http_failure(
                        &prepared.context,
                        response.status,
                        &response.headers,
                    )? {
                        SyncRetryDecision::RetryAfter { delay_ms } => {
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        }
                        SyncRetryDecision::Stop => break response,
                    }
                }
                Ok(response) => break response,
                Err(error) => match session.register_transport_failure(&prepared.context)? {
                    SyncRetryDecision::RetryAfter { delay_ms } => {
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    }
                    SyncRetryDecision::Stop => {
                        session
                            .fail_request(&prepared.context, "sync transport failed")
                            .await?;
                        return Err(error);
                    }
                },
            }
        };
        if let Err(error) = session.accept_response(&prepared.context, response).await {
            session
                .fail_request(&prepared.context, "sync response rejected")
                .await?;
            return Err(error);
        }
    }
    Ok(())
}

async fn measure_sync(
    database: &Database,
    blob_dir: &Path,
    server_url: &str,
    client: &reqwest::Client,
) -> Result<SyncMeasurement> {
    let started = Instant::now();
    let mut total = SyncMeasurement::default();
    loop {
        let mut session = SyncSession::start_with_attachment_storage(
            database.clone(),
            server_url.to_string(),
            None,
            None,
            blob_dir.to_path_buf(),
            LifecyclePolicy::default(),
        )
        .await?;
        drive_session(&mut session, client).await?;
        let summary = session.summary();
        let stalled = summary.pushed == 0
            && summary.pulled == 0
            && summary.blob_uploaded == 0
            && summary.blob_downloaded == 0;
        total.absorb(&summary);
        if summary.complete {
            total.complete = true;
            break;
        }
        ensure!(!stalled, "sync stopped incomplete without progress");
    }
    total.duration_ms = started.elapsed().as_millis();
    Ok(total)
}

fn task_draft(index: usize) -> TaskDraft {
    TaskDraft {
        title: format!("task-{index:05}"),
        description: String::new(),
        project: Some("bench".to_string()),
        status: "todo".to_string(),
        priority: "none".to_string(),
        source: TaskSource::Unknown,
        labels: Vec::new(),
        metadata: Vec::new(),
        available_at: None,
        due_on: None,
        is_epic: false,
    }
}

fn synthetic_png(profile: Profile, index: usize) -> Vec<u8> {
    let image = RgbaImage::from_fn(profile.image_width, profile.image_height, |x, y| {
        image::Rgba([
            (index as u8).wrapping_add(x as u8),
            (index.wrapping_mul(3) as u8).wrapping_add(y as u8),
            (x as u8).wrapping_add(y as u8),
            255,
        ])
    });
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, ImageFormat::Png)
        .expect("encode synthetic PNG");
    bytes.into_inner()
}

async fn seed_source(database: &Database, profile: Profile) -> Result<Vec<aven_core::ids::TaskId>> {
    let workspace = database.resolve_workspace("default").await?;
    let mut task_ids = Vec::with_capacity(profile.ordinary_tasks);
    for index in 0..profile.ordinary_tasks {
        task_ids.push(
            database
                .create_task(&workspace, task_draft(index))
                .await?
                .task
                .id,
        );
    }
    for round in 0..profile.scalar_update_rounds {
        let updates = task_ids
            .iter()
            .enumerate()
            .map(|(index, task_id)| {
                (
                    task_id.clone(),
                    TaskUpdate {
                        description: Some(format!("revision-{round:02}-task-{index:05}")),
                        ..TaskUpdate::default()
                    },
                )
            })
            .collect();
        database.update_tasks(&workspace, updates).await?;
    }

    let start_on = NaiveDate::from_ymd_opt(2026, 1, 1).expect("fixed benchmark date");
    let at = Utc::now();
    for index in 0..profile.recurrence_series {
        let mut outcome = database
            .create_recurrence_series(
                &workspace,
                CreateRecurrenceSeriesParams::new(RecurrenceSeriesDraft {
                    title: format!("series-{index:05}"),
                    description: String::new(),
                    project: "bench".to_string(),
                    priority: "none".to_string(),
                    initial_status: "todo".to_string(),
                    labels: Vec::new(),
                    metadata: Vec::new(),
                    schedule: RecurrenceSchedule::new(
                        RecurrenceRule::daily(),
                        "UTC".parse::<TimeZoneId>()?,
                        start_on,
                        None,
                        RecurrenceDuePolicy::SameDay,
                    ),
                })
                .at(at),
            )
            .await?;
        for occurrence in 0..profile.outcomes_per_series {
            let resolution = database
                .resolve_recurrence_occurrence(
                    &workspace,
                    &outcome.task.id,
                    if occurrence % 2 == 0 {
                        RecurrenceOutcome::Completed
                    } else {
                        RecurrenceOutcome::Skipped
                    },
                )
                .await?;
            if let Some(successor) = resolution.successor {
                outcome.task = successor;
            }
        }
    }

    let blob_dir = default_blob_dir(database.path());
    for (index, task_id) in task_ids.iter().take(profile.attachments).enumerate() {
        database
            .add_task_attachment(
                &workspace,
                &blob_dir,
                LifecyclePolicy::default(),
                task_id,
                AttachmentAddInput {
                    filename: Some(format!("attachment-{index:05}.png")),
                    alt_text: Some(format!("synthetic benchmark image {index}")),
                    declared_media_type: Some("image/png".to_string()),
                    bytes: synthetic_png(profile, index),
                    optimization_policy: ImageOptimizationPolicy::Preserve,
                    dedupe_existing: false,
                },
            )
            .await?;
    }
    Ok(task_ids)
}

async fn diverge_titles(
    source: &Database,
    peer: &Database,
    task_ids: &[aven_core::ids::TaskId],
) -> Result<()> {
    let source_workspace = source.resolve_workspace("default").await?;
    let peer_workspace = peer.resolve_workspace("default").await?;
    let source_updates = task_ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            (
                id.clone(),
                TaskUpdate {
                    title: Some(format!("source-conflict-{index:05}")),
                    ..TaskUpdate::default()
                },
            )
        })
        .collect();
    let peer_updates = task_ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            (
                id.clone(),
                TaskUpdate {
                    title: Some(format!("peer-conflict-{index:05}")),
                    ..TaskUpdate::default()
                },
            )
        })
        .collect();
    source
        .update_tasks(&source_workspace, source_updates)
        .await?;
    peer.update_tasks(&peer_workspace, peer_updates).await?;
    Ok(())
}

fn file_bytes(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |metadata| metadata.len())
}

fn database_file_bytes(path: &Path) -> (u64, u64) {
    let mut wal = path.as_os_str().to_os_string();
    wal.push("-wal");
    (file_bytes(path), file_bytes(Path::new(&wal)))
}

async fn checkpoint(path: &Path) -> Result<()> {
    let options = SqliteConnectOptions::new().filename(path);
    let mut connection = SqliteConnection::connect_with(&options).await?;
    let row = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .fetch_one(&mut connection)
        .await?;
    let busy: i64 = row.try_get(0)?;
    ensure!(
        busy == 0,
        "database checkpoint remained busy: {}",
        path.display()
    );
    Ok(())
}

fn database_size(baseline: (u64, u64), final_size: (u64, u64)) -> DatabaseSize {
    DatabaseSize {
        baseline_main_bytes: baseline.0,
        baseline_wal_bytes: baseline.1,
        final_main_bytes: final_size.0,
        final_wal_bytes: final_size.1,
        growth_bytes: final_size
            .0
            .saturating_add(final_size.1)
            .saturating_sub(baseline.0.saturating_add(baseline.1)),
    }
}

async fn operation_log(path: &Path) -> Result<OperationLog> {
    let options = SqliteConnectOptions::new().filename(path);
    let mut connection = SqliteConnection::connect_with(&options).await?;
    let row = sqlx::query(
        "SELECT count(*) AS change_rows, coalesce(sum(length(payload)), 0) AS payload_bytes FROM changes",
    )
    .fetch_one(&mut connection)
    .await?;
    let rows = sqlx::query(
        "SELECT op_type, count(*) AS rows FROM changes GROUP BY op_type ORDER BY op_type",
    )
    .fetch_all(&mut connection)
    .await?;
    Ok(OperationLog {
        change_rows: row.try_get("change_rows")?,
        payload_bytes: row.try_get("payload_bytes")?,
        by_operation: rows
            .iter()
            .map(|row| Ok((row.try_get("op_type")?, row.try_get("rows")?)))
            .collect::<Result<_>>()?,
    })
}

async fn scalar(path: &Path, sql: &'static str) -> Result<i64> {
    let options = SqliteConnectOptions::new().filename(path);
    let mut connection = SqliteConnection::connect_with(&options).await?;
    Ok(sqlx::query_scalar(sql).fetch_one(&mut connection).await?)
}

fn ratio(numerator: usize, denominator: i64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

async fn run_benchmark(profile: Profile) -> Result<Report> {
    let root = tempfile::tempdir()?;
    let source_path = root.path().join("source.sqlite");
    let peer_path = root.path().join("peer.sqlite");
    let fresh_path = root.path().join("fresh.sqlite");
    let server_path = root.path().join("server.sqlite");

    let source = Database::open(&source_path).await?;
    let peer = Database::open(&peer_path).await?;
    let fresh = Database::open(&fresh_path).await?;
    let mut server = BenchServer::start(root.path(), &server_path)?;
    checkpoint(&source_path).await?;
    checkpoint(&fresh_path).await?;
    checkpoint(&server_path).await?;
    let source_baseline = database_file_bytes(&source_path);
    let fresh_baseline = database_file_bytes(&fresh_path);
    let server_baseline = database_file_bytes(&server_path);
    let client = http_client()?;

    let task_ids = seed_source(&source, profile).await?;
    let initial_push_ack = measure_sync(
        &source,
        &default_blob_dir(&source_path),
        &server.url,
        &client,
    )
    .await?;
    ensure!(initial_push_ack.pushed > 0 && initial_push_ack.pulled == 0);

    let peer_bootstrap =
        measure_sync(&peer, &default_blob_dir(&peer_path), &server.url, &client).await?;
    diverge_titles(&source, &peer, &task_ids[..profile.conflict_tasks]).await?;
    let steady_state_push_ack = measure_sync(
        &source,
        &default_blob_dir(&source_path),
        &server.url,
        &client,
    )
    .await?;
    ensure!(
        steady_state_push_ack.pushed == profile.conflict_tasks as i64
            && steady_state_push_ack.pulled == 0
    );
    let conflicting_device_sync =
        measure_sync(&peer, &default_blob_dir(&peer_path), &server.url, &client).await?;
    let peer_conflicts = peer.unresolved_conflict_count().await?;
    ensure!(peer_conflicts == profile.conflict_tasks as i64);
    let source_convergence = measure_sync(
        &source,
        &default_blob_dir(&source_path),
        &server.url,
        &client,
    )
    .await?;
    let source_conflicts = source.unresolved_conflict_count().await?;
    ensure!(source_conflicts == profile.conflict_tasks as i64);
    let fresh_replica_replay =
        measure_sync(&fresh, &default_blob_dir(&fresh_path), &server.url, &client).await?;
    let fresh_conflicts = fresh.unresolved_conflict_count().await?;
    ensure!(fresh_conflicts == profile.conflict_tasks as i64);

    let fresh_tasks = scalar(&fresh_path, "SELECT count(*) FROM tasks").await?;
    let fresh_recurrence_series =
        scalar(&fresh_path, "SELECT count(*) FROM recurrence_series").await?;
    let fresh_live_attachments = scalar(
        &fresh_path,
        "SELECT count(*) FROM task_attachments WHERE deleted = 0",
    )
    .await?;
    let fresh_distinct_blob_bytes = scalar(
        &fresh_path,
        "SELECT coalesce(sum(byte_size), 0) FROM blob_inventory WHERE available = 1",
    )
    .await?;
    ensure!(fresh_recurrence_series == profile.recurrence_series as i64);
    ensure!(fresh_live_attachments == profile.attachments as i64);

    server.stop();
    let server_distinct_referenced_blob_bytes = scalar(
        &server_path,
        "SELECT coalesce(sum(byte_size), 0) FROM (SELECT DISTINCT sha256, byte_size FROM server_blob_references WHERE deleted = 0)",
    )
    .await?;
    ensure!(fresh_distinct_blob_bytes == server_distinct_referenced_blob_bytes);

    checkpoint(&source_path).await?;
    checkpoint(&fresh_path).await?;
    checkpoint(&server_path).await?;
    let growth = DatabaseGrowth {
        source: database_size(source_baseline, database_file_bytes(&source_path)),
        server: database_size(server_baseline, database_file_bytes(&server_path)),
        fresh: database_size(fresh_baseline, database_file_bytes(&fresh_path)),
    };
    ensure!(growth.source.growth_bytes > 0);
    ensure!(growth.server.growth_bytes > 0);
    ensure!(growth.fresh.growth_bytes > 0);

    let source_log = operation_log(&source_path).await?;
    let server_log = operation_log(&server_path).await?;
    let fresh_log = operation_log(&fresh_path).await?;
    ensure!(source_log.change_rows == server_log.change_rows);
    ensure!(fresh_log.change_rows == server_log.change_rows);
    let growth_per_change = growth.server.growth_bytes as f64 / server_log.change_rows as f64;
    let projected_server_bytes =
        (growth_per_change * 250_000.0).ceil() as u64 + server_baseline.0 + server_baseline.1;

    let initial_pull_ratio = ratio(initial_push_ack.pulled, initial_push_ack.pushed);
    let steady_pull_ratio = ratio(steady_state_push_ack.pulled, steady_state_push_ack.pushed);
    ensure!(initial_pull_ratio == 0.0 && steady_pull_ratio == 0.0);

    let stages = Stages {
        initial_push_ack,
        peer_bootstrap,
        steady_state_push_ack,
        conflicting_device_sync,
        source_convergence,
        fresh_replica_replay,
    };
    ensure!(
        stages.initial_push_ack.complete
            && stages.peer_bootstrap.complete
            && stages.steady_state_push_ack.complete
            && stages.conflicting_device_sync.complete
            && stages.source_convergence.complete
            && stages.fresh_replica_replay.complete
    );

    let derived = DerivedMetrics {
        initial_own_change_pull_ratio: initial_pull_ratio,
        steady_state_own_change_pull_ratio: steady_pull_ratio,
        initial_ack_response_bytes_per_push: ratio(
            stages.initial_push_ack.response_decoded_bytes,
            stages.initial_push_ack.pushed,
        ),
        steady_state_ack_response_bytes_per_push: ratio(
            stages.steady_state_push_ack.response_decoded_bytes,
            stages.steady_state_push_ack.pushed,
        ),
    };
    let mut metric_notes = BTreeMap::new();
    metric_notes.insert(
        "request_wire_bytes",
        "Encoded metadata request bodies only; excludes HTTP framing, blob probes, and blob bodies.",
    );
    metric_notes.insert(
        "response_decoded_bytes",
        "Decoded metadata response bodies only; excludes HTTP framing, blob probes, and blob bodies.",
    );
    metric_notes.insert(
        "blob_bytes",
        "Content bytes uploaded or downloaded through attachment transfer requests.",
    );

    Ok(Report {
        schema_version: SCHEMA_VERSION,
        profile,
        fixed_date: FIXED_DATE,
        metric_notes,
        stages,
        database_growth: growth,
        operation_logs: OperationLogs {
            source: source_log,
            server: server_log,
            fresh: fresh_log,
            server_growth_bytes_per_change: growth_per_change,
            projected_server_bytes_at_250k_changes: projected_server_bytes,
        },
        verification: Verification {
            source_open_title_conflicts: source_conflicts,
            peer_open_title_conflicts: peer_conflicts,
            fresh_open_title_conflicts: fresh_conflicts,
            fresh_tasks,
            fresh_recurrence_series,
            fresh_live_attachments,
            server_distinct_referenced_blob_bytes,
            fresh_distinct_blob_bytes,
        },
        derived,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    let (profile, output) = match parse_args()? {
        Invocation::Run { profile, output } => (profile, output),
        Invocation::Noop { quiet } => {
            if !quiet {
                println!(
                    "usage: cargo bench --bench sync_scaling -- --profile smoke|report [--output PATH]"
                );
            }
            return Ok(());
        }
    };
    let report = run_benchmark(profile).await?;
    let json = serde_json::to_string_pretty(&report)?;
    if let Some(path) = output {
        std::fs::write(path, json)?;
    } else {
        println!("{json}");
    }
    Ok(())
}
