//! Ignored sync throughput benchmark. Counts rounds, HTTP requests, HTTP body
//! bytes and protected backend loads (one Keychain lookup each on macOS) for a
//! seed that pushes many changes and images, and a peer that pulls them.
//!
//! cargo test --lib encrypted_tail_http::tests::bench -- --ignored --nocapture
//! Sizes: AVEN_BENCH_TASKS (default 2000), AVEN_BENCH_IMAGES (default 500).
use super::*;
use crate::protected_local_keys::tests::BACKEND_LOADS;
use std::sync::atomic::Ordering::Relaxed;
use std::time::Instant;

fn size(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn distinct_png(index: usize) -> Vec<u8> {
    let mut image = ::image::RgbaImage::new(11, 7);
    for (offset, byte) in image.as_mut().iter_mut().enumerate() {
        *byte = (offset as u8).wrapping_mul(31);
    }
    image.as_mut()[..8].copy_from_slice(&(index as u64).to_be_bytes());
    let mut bytes = std::io::Cursor::new(Vec::new());
    ::image::DynamicImage::ImageRgba8(image)
        .write_to(&mut bytes, ::image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

struct Measured {
    rounds: usize,
    requests: u64,
    request_bytes: u64,
    response_bytes: u64,
    loads: u64,
    seconds: f64,
}

impl Measured {
    fn print(&self, label: &str) {
        println!(
            "{label}: rounds={} requests={} request_bytes={} response_bytes={} protected_loads={} loads_per_round={:.1} elapsed={:.2}s",
            self.rounds,
            self.requests,
            self.request_bytes,
            self.response_bytes,
            self.loads,
            self.loads as f64 / self.rounds.max(1) as f64,
            self.seconds
        );
    }
}

async fn measure_drain(client: &Client, store: &ProtectedLocalKeyStore, db: &Database) -> Measured {
    let (requests, request_bytes, response_bytes, loads, start) = (
        HTTP_REQUESTS.load(Relaxed),
        HTTP_REQUEST_BYTES.load(Relaxed),
        HTTP_RESPONSE_BYTES.load(Relaxed),
        BACKEND_LOADS.load(Relaxed),
        Instant::now(),
    );
    let mut rounds = 0;
    let mut drain = client.start_drain(store, db).await.unwrap();
    loop {
        let round = client
            .round_in_drain(store, db, &blobs(db), &mut drain)
            .await
            .unwrap();
        rounds += 1;
        assert_ne!(round.images, ImageTransfer::Failed);
        if round.metadata_caught_up && round.images == ImageTransfer::Complete {
            break;
        }
        assert!(rounds < 100_000, "round budget");
    }
    Measured {
        rounds,
        requests: HTTP_REQUESTS.load(Relaxed) - requests,
        request_bytes: HTTP_REQUEST_BYTES.load(Relaxed) - request_bytes,
        response_bytes: HTTP_RESPONSE_BYTES.load(Relaxed) - response_bytes,
        loads: BACKEND_LOADS.load(Relaxed) - loads,
        seconds: start.elapsed().as_secs_f64(),
    }
}

#[tokio::test]
async fn drain_reuses_protected_tail_snapshot() {
    let f = fixture().await;
    converge(&f).await;
    let workspace = f.seed.list_workspaces().await.unwrap().remove(0);
    for index in 0..5 {
        f.seed
            .create_task(&workspace, draft(&format!("snapshot task {index}")))
            .await
            .unwrap();
    }
    let client = Client::new(&f.origin).unwrap();
    let (drain, setup_loads) = BACKEND_LOADS
        .measure(client.start_drain(&f.seed_store, &f.seed))
        .await;
    let mut drain = drain.unwrap();
    let (rounds, round_loads) = BACKEND_LOADS
        .measure(async {
            for round_number in 1..=16 {
                let round = client
                    .round_in_drain(&f.seed_store, &f.seed, &blobs(&f.seed), &mut drain)
                    .await
                    .unwrap();
                if round.metadata_caught_up && round.images == ImageTransfer::Complete {
                    return round_number;
                }
            }
            panic!("round budget")
        })
        .await;
    assert!(setup_loads > 0);
    assert_eq!(rounds, 1);
    assert_eq!(round_loads, 0);
}

#[tokio::test]
async fn one_sync_drains_1500_offline_edits_and_peer_converges() {
    let f = fixture().await;
    converge(&f).await;
    let workspace = f.seed.list_workspaces().await.unwrap().remove(0);
    let task = f
        .seed
        .create_task(&workspace, draft("offline edit target"))
        .await
        .unwrap()
        .task;
    // Publish the task first so the measured backlog consists only of edits.
    let client = Client::new(&f.origin).unwrap();
    crate::sync::encrypted::drain(
        &client,
        &f.seed_store,
        &f.seed,
        &blobs(&f.seed),
        crate::sync::encrypted::ROUND_LIMIT,
    )
    .await
    .unwrap();
    crate::sync::encrypted::drain(
        &client,
        &f.peer_store,
        &f.peer,
        &blobs(&f.peer),
        crate::sync::encrypted::ROUND_LIMIT,
    )
    .await
    .unwrap();

    let before = scalar(&f.seed, "SELECT count(*) FROM changes").await;
    for index in 0..1500 {
        f.seed
            .update_task(
                &workspace,
                &task.id,
                TaskUpdate {
                    title: Some(format!("offline edit {index}")),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    assert_eq!(
        scalar(&f.seed, "SELECT count(*) FROM changes").await - before,
        1500
    );

    let mut seed_drain = client.start_drain(&f.seed_store, &f.seed).await.unwrap();
    let mut seed_rounds = 1;
    let mut round = client
        .round_in_drain(&f.seed_store, &f.seed, &blobs(&f.seed), &mut seed_drain)
        .await
        .unwrap();
    assert_eq!(
        scalar(
            &f.seed,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        0,
        "the first round must empty the 1500-edit outbox"
    );
    while !round.metadata_caught_up || round.images != ImageTransfer::Complete {
        assert!(seed_rounds < crate::sync::encrypted::ROUND_LIMIT);
        round = client
            .round_in_drain(&f.seed_store, &f.seed, &blobs(&f.seed), &mut seed_drain)
            .await
            .unwrap();
        seed_rounds += 1;
    }
    let peer = crate::sync::encrypted::drain(
        &client,
        &f.peer_store,
        &f.peer,
        &blobs(&f.peer),
        crate::sync::encrypted::ROUND_LIMIT,
    )
    .await
    .unwrap();
    assert!(peer.metadata_caught_up);
    assert_eq!(title(&f.peer, &task.id).await, "offline edit 1499");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "benchmark"]
async fn bench_push_and_pull_many_changes_and_images() {
    let (tasks, images) = (
        size("AVEN_BENCH_TASKS", 2000),
        size("AVEN_BENCH_IMAGES", 500),
    );
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let before = scalar(&f.seed, "SELECT count(*) FROM changes").await;
    let mut ids = Vec::new();
    for index in 0..tasks {
        let task = f
            .seed
            .create_task(&w, draft(&format!("bench task {index}")))
            .await
            .unwrap()
            .task;
        ids.push(task.id);
    }
    for index in 0..images {
        f.seed
            .add_task_attachment(
                &w,
                &blobs(&f.seed),
                Default::default(),
                &ids[index % ids.len()],
                aven_core::operations::AttachmentAddInput {
                    filename: Some(format!("bench-{index}.png")),
                    alt_text: None,
                    declared_media_type: None,
                    bytes: distinct_png(index),
                    optimization_policy: aven_core::attachments::ImageOptimizationPolicy::Preserve,
                    dedupe_existing: false,
                },
            )
            .await
            .unwrap();
    }
    let changes = scalar(&f.seed, "SELECT count(*) FROM changes").await - before;
    println!("workload: tasks={tasks} images={images} changes={changes}");
    let client = Client::new(&f.origin).unwrap();
    measure_drain(&client, &f.seed_store, &f.seed)
        .await
        .print("seed push");
    measure_drain(&client, &f.peer_store, &f.peer)
        .await
        .print("peer pull");
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM task_attachments WHERE deleted=0"
        )
        .await,
        scalar(
            &f.seed,
            "SELECT count(*) FROM task_attachments WHERE deleted=0"
        )
        .await
    );
}
