//! Ignored sync throughput benchmark. Counts rounds, HTTP requests and
//! protected backend loads (one Keychain lookup each on macOS) for a seed that
//! pushes many changes and images, and a peer that pulls them.
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
    loads: u64,
    seconds: f64,
}

impl Measured {
    fn print(&self, label: &str) {
        println!(
            "{label}: rounds={} requests={} protected_loads={} loads_per_round={:.1} elapsed={:.2}s",
            self.rounds,
            self.requests,
            self.loads,
            self.loads as f64 / self.rounds.max(1) as f64,
            self.seconds
        );
    }
}

async fn measure_drain(client: &Client, store: &ProtectedLocalKeyStore, db: &Database) -> Measured {
    let (requests, loads, start) = (
        HTTP_REQUESTS.load(Relaxed),
        BACKEND_LOADS.load(Relaxed),
        Instant::now(),
    );
    let mut rounds = 0;
    loop {
        let round = client.round(store, db, &blobs(db)).await.unwrap();
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
        loads: BACKEND_LOADS.load(Relaxed) - loads,
        seconds: start.elapsed().as_secs_f64(),
    }
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
