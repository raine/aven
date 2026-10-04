//! Fuzz hooks for tail records, their decrypted operations and image objects.
use super::attachments::codec::{Descriptor, DescriptorAuthority};
use super::domain::{self, Projection};
use super::*;
use crate::db::{Database, begin_immediate};
use crate::sync::fuzz::{Input, frame};
use crate::sync::persistence::changes::canonical_equal;
use crate::sync::wire::ChangeWire;
use std::collections::HashSet;

thread_local! {
    static AUTHORITY: Authority = fixture_authority();
    static REPLICA: (tempfile::TempDir, tokio::runtime::Runtime, Database) = replica();
}

fn replica() -> (tempfile::TempDir, tokio::runtime::Runtime, Database) {
    let dir = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let path = dir.path().join("replica.db");
    let db = runtime
        .block_on(crate::test_support::open_blank_database(&path))
        .unwrap();
    (dir, runtime, db)
}

/// An accepted operation must reseal and reopen to the same operation.
fn reseal(a: &Authority, change: &ChangeWire, projection: &Projection) {
    let record = codec::seal_projection(a, change, projection).expect("an opened change reseals");
    let again = codec::open(a, &record).expect("a resealed change opens");
    assert!(canonical_equal(change, &again));
}

pub(crate) fn tail(data: &[u8]) {
    AUTHORITY.with(|a| {
        let mut input = Input(data);
        match input.byte() % 4 {
            0 => {
                let record = input.0;
                let _ = a.record_is_closed(record);
                if let Ok(change) = codec::open(a, record) {
                    let projection = codec::parse(record).unwrap().projection;
                    reseal(a, &change, &projection);
                }
            }
            1 => {
                let id = input.part();
                let projection = input.part();
                let record = codec::seal_raw(a, id, projection, input.0);
                if let Ok(change) = codec::open(a, &record) {
                    let decoded = Projection::decode(projection).unwrap();
                    assert_eq!(decoded.encode(), projection);
                    reseal(a, &change, &decoded);
                }
            }
            2 => {
                if let Ok(change) = domain::decode(input.0) {
                    let projection = domain::validate(&change).unwrap();
                    let again = domain::decode(&serde_json::to_vec(&change).unwrap())
                        .expect("a decoded operation re-decodes");
                    assert!(canonical_equal(&change, &again));
                    assert!(domain::validate(&again).unwrap() == projection);
                }
            }
            _ => {
                if let Ok(projection) = Projection::decode(input.0) {
                    assert_eq!(projection.encode(), input.0);
                }
            }
        }
    })
}

/// Applies up to 16 decrypted operations as one rolled-back page, the way an
/// accepted page reaches the replica after decryption and domain checks.
pub(crate) fn apply(data: &[u8]) {
    REPLICA.with(|(_, runtime, db)| {
        runtime.block_on(async {
            let mut conn = db.acquire_writer().await.unwrap();
            let mut tx = begin_immediate(&mut conn).await.unwrap();
            let mut input = Input(data);
            let mut hashes = HashSet::new();
            let mut sequence = 0;
            while !input.is_empty() && sequence < 16 {
                let Ok(mut change) = domain::decode(input.part()) else {
                    continue;
                };
                sequence += 1;
                change.server_seq = Some(sequence);
                if client::apply_new_remote_change(&mut tx, &change, &mut hashes)
                    .await
                    .is_err()
                {
                    break;
                }
                let reconciled = async {
                    notes::reconcile(&mut tx, 0, &change).await?;
                    labels::reconcile(&mut tx, 0, &change).await?;
                    if let Some(workspace) = dependencies::affected_workspace(&change)? {
                        dependencies::reconcile(&mut tx, 0, workspace).await?;
                    }
                    moves::reconcile(&mut tx, 0).await?;
                    graphs::reconcile(&mut tx, 0).await?;
                    anyhow::Ok(())
                };
                if reconciled.await.is_err() {
                    break;
                }
            }
        })
    })
}

pub(crate) fn attachment(data: &[u8]) {
    AUTHORITY.with(|a| {
        let mut input = Input(data);
        match input.byte() % 2 {
            0 => {
                if let Ok(d) = Descriptor::decode(input.0) {
                    assert_eq!(d.encode().expect("a decoded descriptor encodes"), input.0);
                }
            }
            _ => {
                let descriptor = input.part();
                let mut records = Vec::new();
                while !input.is_empty() && records.len() < 32 {
                    records.push(input.part().to_vec());
                }
                if let Ok(d) = Descriptor::decode(descriptor) {
                    for (index, record) in records.iter().enumerate() {
                        let _ = d.verify_chunk(index, record);
                    }
                    let _ = d.open(a, &records);
                }
            }
        }
    })
}

pub(crate) fn strict_json(bytes: &[u8]) {
    if let Ok(value) = domain::strict_value(bytes) {
        let again = domain::strict_value(&serde_json::to_vec(&value).unwrap())
            .expect("strict JSON re-encodes strictly");
        // serde_json's default float parsing is not exact, so only integer
        // and non-numeric values must survive the round trip unchanged.
        if !has_float(&value) {
            assert_eq!(value, again);
        }
    }
}

fn has_float(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Number(n) => n.is_f64(),
        serde_json::Value::Array(items) => items.iter().any(has_float),
        serde_json::Value::Object(map) => map.values().any(has_float),
        _ => false,
    }
}

/// Operations from a small local history, valid for the encrypted tail.
fn changes() -> Vec<ChangeWire> {
    crate::sync::fuzz::with_history(async |db| {
        let source = db.list_workspaces().await.unwrap().remove(0);
        db.create_project(&source, "Move source").await.unwrap();
        let target = db.create_workspace("Move destination").await.unwrap();
        db.create_project(&target, "Destination").await.unwrap();
        let task = db
            .create_task(
                &source,
                crate::operations::TaskDraft {
                    title: "Movable fuzz task".into(),
                    description: String::new(),
                    project: Some("Move source".into()),
                    status: "todo".into(),
                    priority: "none".into(),
                    source: crate::choices::TaskSource::Cli,
                    labels: Vec::new(),
                    metadata: Vec::new(),
                    available_at: None,
                    due_on: None,
                    is_epic: false,
                },
            )
            .await
            .unwrap()
            .task;
        db.move_tasks(
            &source,
            crate::operations::MoveTasksInput {
                task_ids: vec![task.id],
                target_workspace: target,
                target_project: "Destination".into(),
            },
        )
        .await
        .unwrap();
        let mut conn = db.acquire_writer().await.unwrap();
        let ids: Vec<String> = sqlx::query_scalar("SELECT change_id FROM changes ORDER BY rowid")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
        let mut changes = Vec::new();
        for id in ids {
            let change = client::load_change(&mut conn, &id).await.unwrap().unwrap();
            if domain::validate(&change).is_ok() {
                changes.push(change);
            }
        }
        changes
    })
}

pub(crate) fn seeds() -> Vec<(&'static str, Vec<u8>)> {
    let changes = changes();
    assert!(!changes.is_empty());
    let plain: Vec<Vec<u8>> = changes
        .iter()
        .map(|c| serde_json::to_vec(c).unwrap())
        .collect();
    let mut seeds = Vec::new();
    AUTHORITY.with(|a| {
        for (change, plain) in changes.iter().zip(&plain) {
            let projection = domain::validate(change).unwrap();
            let record = codec::seal_projection(a, change, &projection).unwrap();
            let encoded = projection.encode();
            seeds.push(("tail", frame(&[0], &[&record])));
            seeds.push((
                "tail",
                frame(&[1], &[change.change_id.as_bytes(), &encoded, plain]),
            ));
            seeds.push(("tail", frame(&[2], &[plain])));
            seeds.push(("tail", frame(&[3], &[&encoded])));
            seeds.push(("wire", frame(&[6], &[plain])));
        }
        let parts: Vec<&[u8]> = plain.iter().map(Vec::as_slice).chain([&[][..]]).collect();
        seeds.push(("tail_apply", frame(&[], &parts)));
        let (d, records) = Descriptor::seal(a, &[42; 40000]).unwrap();
        let encoded = d.encode().unwrap();
        seeds.push(("attachment", frame(&[0], &[&encoded])));
        let mut parts = vec![&encoded[..]];
        parts.extend(records.iter().map(Vec::as_slice));
        parts.push(&[]);
        seeds.push(("attachment", frame(&[1], &parts)));
    });
    seeds
}
