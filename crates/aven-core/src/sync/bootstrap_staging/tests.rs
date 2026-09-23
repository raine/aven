use super::*;
use crate::db::Database;
use crate::sync::seed_claim::{ClaimAuthentication, Secret, SeedAuthority, SetupAuthority};
use crate::sync::{LocalSharedStatePackageContext, LocalSharedStatePackageKey, bootstrap_format};
use sha2::{Digest, Sha256};

mod failures;

struct Fixture {
    dir: tempfile::TempDir,
    source: Database,
    server: Database,
    seed: SeedAuthority,
    key: LocalSharedStatePackageKey,
    package: bootstrap_format::Package,
    id: [u8; 32],
    image_hash: String,
}

impl Fixture {
    async fn new() -> Self {
        Self::build(false).await
    }

    async fn build(large: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let source = Database::open(&dir.path().join("source.sqlite"))
            .await
            .unwrap();
        let mut conn = source.acquire_writer().await.unwrap();
        let workspace = crate::workspaces::ensure_default_workspace(&mut conn)
            .await
            .unwrap();
        drop(conn);
        let task = source
            .create_task(
                &workspace,
                crate::operations::TaskDraft {
                    title: "PRIVATE-STAGING-TASK-TITLE".into(),
                    description: "PRIVATE-STAGING-DESCRIPTION".into(),
                    project: Some("app".into()),
                    status: "todo".into(),
                    priority: "none".into(),
                    source: crate::choices::TaskSource::Cli,
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
        if large {
            let updates = (0..27_000)
                .map(|i| {
                    (
                        task.id.clone(),
                        "description".into(),
                        format!("PRIVATE-HISTORY-{i}"),
                    )
                })
                .collect::<Vec<_>>();
            source.set_task_fields(&workspace, &updates).await.unwrap();
        }
        let mut image_hash = String::new();
        for width in [2, 3] {
            let mut encoded = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(image::RgbaImage::new(width, 1))
                .write_to(&mut encoded, image::ImageFormat::Png)
                .unwrap();
            let outcome = source
                .add_task_attachment(
                    &workspace,
                    dir.path(),
                    Default::default(),
                    &task.id,
                    crate::operations::AttachmentAddInput {
                        filename: Some("PRIVATE-STAGING-FILENAME.png".into()),
                        alt_text: Some("PRIVATE-STAGING-ALT-TEXT".into()),
                        declared_media_type: None,
                        bytes: encoded.into_inner(),
                        optimization_policy:
                            crate::attachments::optimization::ImageOptimizationPolicy::Preserve,
                        dedupe_existing: false,
                    },
                )
                .await
                .unwrap();
            image_hash = outcome.outcome.attachment.sha256;
            if width == 3 {
                source
                    .delete_task_attachment(&workspace, &outcome.outcome.attachment.attachment_id)
                    .await
                    .unwrap();
            }
        }
        let context = LocalSharedStatePackageContext {
            vault_id: [31; 32],
            generation_id: [42; 32],
        };
        let key = LocalSharedStatePackageKey::new([53; 32]);
        let seed = SeedAuthority::generate(context, &key, [64; 32]).unwrap();
        let capture = source
            .capture_local_shared_state_never_dispatched(dir.path())
            .await
            .unwrap();
        let id = hex::decode(capture.candidate_id())
            .unwrap()
            .try_into()
            .unwrap();
        let package = source
            .package_local_shared_state_never_dispatched(
                dir.path(),
                context,
                &key,
                seed.genesis().commitment(),
            )
            .await
            .unwrap()
            .upload_package();
        assert_eq!(
            bootstrap_format::validate_keyless(&package)
                .unwrap()
                .image_count,
            2
        );
        let server = Database::open(&dir.path().join("server.sqlite"))
            .await
            .unwrap();
        let setup_secret = Secret::new([75; 32]);
        let setup = SetupAuthority::from_verifier(
            [64; 32],
            SetupAuthority::verifier([64; 32], &setup_secret),
        );
        server
            .admit_seed_claim(
                &seed.genesis().claim_bytes(),
                Some(&setup),
                ClaimAuthentication::SetupSecret(&setup_secret),
            )
            .await
            .unwrap();
        Self {
            dir,
            source,
            server,
            seed,
            key,
            package,
            id,
            image_hash,
        }
    }

    fn auth(&self) -> Authentication<'_> {
        Authentication {
            vault_id: self.seed.genesis().context().vault_id,
            genesis_commitment: self.seed.genesis().commitment(),
            bearer: self.seed.bearer(),
        }
    }

    fn commitment(&self) -> [u8; 32] {
        Sha256::digest(&self.package.descriptor).into()
    }

    fn budget(&self) -> Budget {
        let mut bytes = 0;
        let mut chunks = 0;
        for (_, records) in self.components() {
            for record in records {
                bytes += record.len() as u64;
                chunks += 1;
            }
        }
        Budget { bytes, chunks }
    }

    fn components(&self) -> Vec<(Component, Vec<&[u8]>)> {
        let mut result = Vec::new();
        for (index, component) in [
            Component::DataCatalog,
            Component::PrefixCatalog,
            Component::ImageCatalog,
        ]
        .into_iter()
        .enumerate()
        {
            result.push((
                component,
                self.package.catalogs[index].chunks(1_048_576).collect(),
            ));
        }
        result.push((
            Component::Manifest,
            self.package.manifest.iter().map(Vec::as_slice).collect(),
        ));
        result.push((
            Component::State,
            self.package.state.iter().map(Vec::as_slice).collect(),
        ));
        result.extend(self.package.images.iter().map(|i| {
            (
                Component::Image(i.object_id),
                i.records.iter().map(Vec::as_slice).collect(),
            )
        }));
        result
    }

    fn request<'a>(
        &self,
        epoch: u64,
        component: Component,
        index: u64,
        bytes: &'a [u8],
    ) -> PutChunk<'a> {
        PutChunk {
            bootstrap_id: self.id,
            descriptor_commitment: self.commitment(),
            epoch,
            component,
            index,
            bytes,
        }
    }

    async fn declare(&self) -> StagingStatus {
        self.server
            .declare_bootstrap_staging(&self.auth(), &self.package.descriptor, self.budget())
            .await
            .unwrap()
    }

    async fn status(&self) -> StagingStatus {
        match self
            .server
            .bootstrap_staging_status(&self.auth(), self.id)
            .await
            .unwrap()
        {
            Status::Staging(status) => status,
            _ => panic!("expected declared candidate"),
        }
    }

    async fn upload(&self, epoch: u64) {
        for (component, records) in self.components() {
            for (index, bytes) in records.iter().enumerate() {
                let result = self
                    .server
                    .put_bootstrap_chunk(
                        &self.auth(),
                        self.request(epoch, component, index as u64, bytes),
                    )
                    .await
                    .unwrap();
                if component.catalog().is_some() && index + 1 < records.len() {
                    assert!(matches!(
                        result,
                        PutOutcome::Quarantined | PutOutcome::Verified
                    ));
                } else {
                    assert_eq!(result, PutOutcome::Verified);
                }
            }
        }
    }
}

fn presence(status: &StagingStatus, component: Component) -> &[Presence] {
    &status
        .components
        .iter()
        .find(|c| c.component == component)
        .unwrap()
        .chunks
}

#[tokio::test]
async fn captured_package_round_trip_restart_exact_retries_and_local_immutability() {
    let mut f = Fixture::new().await;
    assert_eq!(
        f.server
            .bootstrap_staging_status(&f.auth(), f.id)
            .await
            .unwrap(),
        Status::Missing
    );
    let s = f.declare().await;
    assert_eq!(s, f.declare().await);
    assert!(
        s.components
            .iter()
            .all(|c| c.chunks.iter().all(|p| *p == Presence::Missing))
    );
    // A committed PUT with its response ignored must survive reopening the server.
    f.server
        .put_bootstrap_chunk(
            &f.auth(),
            f.request(s.epoch, Component::DataCatalog, 0, &f.package.catalogs[0]),
        )
        .await
        .unwrap();
    f.server = Database::open(&f.dir.path().join("server.sqlite"))
        .await
        .unwrap();
    let resumed = f
        .server
        .ensure_bootstrap_staging(&f.auth(), f.id, f.commitment())
        .await
        .unwrap();
    assert_eq!(resumed.epoch, s.epoch);
    assert_eq!(
        presence(&resumed, Component::DataCatalog),
        [Presence::Verified]
    );
    f.upload(resumed.epoch).await;
    f.upload(resumed.epoch).await;
    let full = f.status().await;
    assert!(
        full.components
            .iter()
            .all(|c| c.chunks.iter().all(|p| *p == Presence::Verified))
    );
    assert_eq!(
        f.budget().chunks as usize,
        full.components
            .iter()
            .map(|c| c.chunks.len())
            .sum::<usize>()
    );
    let mut conn = f.server.acquire_reader().await.unwrap();
    for (component, expected) in f.components() {
        let stored: Vec<Vec<u8>> = sqlx::query_scalar("SELECT bytes FROM server_bootstrap_chunks WHERE bootstrap = ? AND component = ? ORDER BY chunk_index")
            .bind(f.id.as_slice()).bind(component.key()).fetch_all(&mut *conn).await.unwrap();
        assert_eq!(
            stored.iter().map(Vec::as_slice).collect::<Vec<_>>(),
            expected
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM changes")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(count, 0);
    drop(conn);
    f.server
        .cancel_bootstrap_staging(&f.auth(), f.id)
        .await
        .unwrap();
    let local = f
        .source
        .package_local_shared_state_never_dispatched(
            f.dir.path(),
            f.seed.genesis().context(),
            &f.key,
            f.seed.genesis().commitment(),
        )
        .await
        .unwrap();
    assert!(local.upload_package() == f.package);
    bootstrap_format::authenticate(
        &f.package,
        &f.key,
        f.seed.genesis().context(),
        *local.stream_id(),
        f.id,
        f.seed.genesis().commitment(),
    )
    .unwrap();
}

#[tokio::test]
async fn authentication_context_and_successor_gate_protect_every_operation() {
    let f = Fixture::new().await;
    let wrong = Secret::new([99; 32]);
    let mut auth = f.auth();
    auth.bearer = &wrong;
    assert!(
        f.server
            .declare_bootstrap_staging(&auth, &f.package.descriptor, f.budget())
            .await
            .is_err()
    );
    for auth in [
        auth,
        Authentication {
            vault_id: [0; 32],
            ..f.auth()
        },
        Authentication {
            genesis_commitment: [0; 32],
            ..f.auth()
        },
    ] {
        assert!(
            f.server
                .bootstrap_staging_status(&auth, f.id)
                .await
                .is_err()
        );
        assert!(
            f.server
                .cancel_bootstrap_staging(&auth, f.id)
                .await
                .is_err()
        );
    }
    // The exact descriptor profile positions are frozen in the codec documentation.
    for offset in [7, 71, 135] {
        let mut descriptor = f.package.descriptor.clone();
        descriptor[offset] ^= 1;
        assert!(
            f.server
                .declare_bootstrap_staging(&f.auth(), &descriptor, f.budget())
                .await
                .is_err()
        );
    }
    let unclaimed = Database::open(&f.dir.path().join("unclaimed.sqlite"))
        .await
        .unwrap();
    assert!(
        unclaimed
            .declare_bootstrap_staging(&f.auth(), &f.package.descriptor, f.budget())
            .await
            .is_err()
    );
    assert!(
        unclaimed
            .cancel_bootstrap_staging(&f.auth(), f.id)
            .await
            .is_err()
    );
    let s = f.declare().await;
    let wrong_auth = Authentication {
        bearer: &wrong,
        ..f.auth()
    };
    assert!(
        f.server
            .ensure_bootstrap_staging(&wrong_auth, f.id, f.commitment())
            .await
            .is_err()
    );
    assert!(
        f.server
            .reclaim_bootstrap_staging(&wrong_auth, f.id, f.commitment(), s.epoch, Reclaim::All)
            .await
            .is_err()
    );
    assert!(
        f.server
            .put_bootstrap_chunk(
                &wrong_auth,
                f.request(s.epoch, Component::Manifest, 0, &f.package.manifest[0])
            )
            .await
            .is_err()
    );
    let mut request = f.request(s.epoch, Component::Manifest, 0, &f.package.manifest[0]);
    request.descriptor_commitment[0] ^= 1;
    assert!(
        f.server
            .put_bootstrap_chunk(&f.auth(), request)
            .await
            .is_err()
    );
    let mut descriptor = f.package.descriptor.clone();
    descriptor[39] ^= 1; // same bootstrap, divergent stream
    assert!(
        f.server
            .declare_bootstrap_staging(&f.auth(), &descriptor, f.budget())
            .await
            .is_err()
    );
    let mut conn = f.server.acquire_writer().await.unwrap();
    sqlx::query("UPDATE server_seed_claim SET genesis_only = 0")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    assert!(
        f.server
            .declare_bootstrap_staging(&f.auth(), &f.package.descriptor, f.budget())
            .await
            .is_err()
    );
    assert!(
        f.server
            .bootstrap_staging_status(&f.auth(), f.id)
            .await
            .is_err()
    );
    assert!(
        f.server
            .ensure_bootstrap_staging(&f.auth(), f.id, f.commitment())
            .await
            .is_err()
    );
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(s.epoch, Component::Manifest, 0, &f.package.manifest[0])
            )
            .await
            .is_err()
    );
    assert!(
        f.server
            .cancel_bootstrap_staging(&f.auth(), f.id)
            .await
            .is_err()
    );
    assert!(
        f.server
            .reclaim_bootstrap_staging(&f.auth(), f.id, f.commitment(), s.epoch, Reclaim::All)
            .await
            .is_err()
    );
    assert!(
        f.server
            .admit_seed_claim(
                &f.seed.genesis().claim_bytes(),
                None,
                ClaimAuthentication::SeedBearer(f.seed.bearer())
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn data_requires_verified_catalogs_and_exact_conflicts_never_overwrite() {
    let f = Fixture::new().await;
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(1, Component::State, 0, &f.package.state[0])
            )
            .await
            .is_err()
    );
    let s = f.declare().await;
    for (component, bytes) in [
        (Component::State, &f.package.state[0]),
        (
            Component::Image(f.package.images[0].object_id),
            &f.package.images[0].records[0],
        ),
    ] {
        assert!(
            f.server
                .put_bootstrap_chunk(&f.auth(), f.request(s.epoch, component, 0, bytes))
                .await
                .is_err()
        );
    }
    f.upload(s.epoch).await;
    for (component, records) in f.components() {
        let mut bad = records[0].to_vec();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(
            f.server
                .put_bootstrap_chunk(&f.auth(), f.request(s.epoch, component, 0, &bad))
                .await
                .is_err()
        );
    }
    f.upload(s.epoch).await;
}

#[tokio::test]
async fn cancellation_before_declare_competition_and_upload_race_are_terminal() {
    let f = Fixture::new().await;
    f.server
        .cancel_bootstrap_staging(&f.auth(), f.id)
        .await
        .unwrap();
    f.server
        .cancel_bootstrap_staging(&f.auth(), f.id)
        .await
        .unwrap();
    assert_eq!(
        f.server
            .bootstrap_staging_status(&f.auth(), f.id)
            .await
            .unwrap(),
        Status::Canceled
    );
    assert!(
        f.server
            .declare_bootstrap_staging(&f.auth(), &f.package.descriptor, f.budget())
            .await
            .is_err()
    );
    let mut other = f.package.descriptor.clone();
    other[103] ^= 1;
    f.server
        .declare_bootstrap_staging(&f.auth(), &other, f.budget())
        .await
        .unwrap();
    other[103] ^= 2;
    assert!(
        f.server
            .declare_bootstrap_staging(&f.auth(), &other, f.budget())
            .await
            .is_err()
    );

    let f = Fixture::new().await;
    let s = f.declare().await;
    // Independent pools exercise SQLite serialization, not only the shared gate.
    let second = Database::open(&f.dir.path().join("server.sqlite"))
        .await
        .unwrap();
    let auth = f.auth();
    let (cancel, put) = tokio::join!(
        second.cancel_bootstrap_staging(&auth, f.id),
        f.server.put_bootstrap_chunk(
            &auth,
            f.request(s.epoch, Component::Manifest, 0, &f.package.manifest[0])
        )
    );
    cancel.unwrap();
    let _ = put; // either committed before cancel or rejected afterward
    assert_eq!(
        f.server
            .bootstrap_staging_status(&f.auth(), f.id)
            .await
            .unwrap(),
        Status::Canceled
    );
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(s.epoch, Component::Manifest, 0, &f.package.manifest[0])
            )
            .await
            .is_err()
    );
    let mut conn = f.server.acquire_reader().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM server_bootstrap_chunks")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn expiry_resume_and_reclamation_fence_stale_requests_but_preserve_verified_bytes() {
    let f = Fixture::new().await;
    let first = f.declare().await;
    f.upload(first.epoch).await;
    let mut conn = f.server.acquire_writer().await.unwrap();
    sqlx::query("UPDATE server_bootstrap_candidates SET expires_at = 1")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let expired = f.status().await;
    assert_eq!(expired.expires_at, 1);
    assert_eq!(f.status().await, expired); // status is observational
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(first.epoch, Component::Manifest, 0, &f.package.manifest[0])
            )
            .await
            .is_err()
    );
    let resumed = f
        .server
        .ensure_bootstrap_staging(&f.auth(), f.id, f.commitment())
        .await
        .unwrap();
    assert!(resumed.epoch > first.epoch);
    assert_eq!(resumed.components, expired.components);
    assert!(
        f.server
            .reclaim_bootstrap_staging(&f.auth(), f.id, f.commitment(), first.epoch, Reclaim::All)
            .await
            .is_err()
    );
    f.server
        .reclaim_bootstrap_staging(
            &f.auth(),
            f.id,
            f.commitment(),
            resumed.epoch,
            Reclaim::Quarantine,
        )
        .await
        .unwrap();
    let retained = f
        .server
        .ensure_bootstrap_staging(&f.auth(), f.id, f.commitment())
        .await
        .unwrap();
    assert_eq!(retained.components, resumed.components);
    f.server
        .reclaim_bootstrap_staging(
            &f.auth(),
            f.id,
            f.commitment(),
            retained.epoch,
            Reclaim::All,
        )
        .await
        .unwrap();
    let empty = f
        .server
        .ensure_bootstrap_staging(&f.auth(), f.id, f.commitment())
        .await
        .unwrap();
    assert!(
        empty
            .components
            .iter()
            .all(|c| c.chunks.iter().all(|p| *p == Presence::Missing))
    );
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(
                    retained.epoch,
                    Component::Manifest,
                    0,
                    &f.package.manifest[0]
                )
            )
            .await
            .is_err()
    );
    f.upload(empty.epoch).await;
}

#[tokio::test]
async fn competing_declarations_and_cancel_before_declare_races_serialize_across_pools() {
    let f = Fixture::new().await;
    let second = Database::open(&f.dir.path().join("server.sqlite"))
        .await
        .unwrap();
    let mut descriptor = f.package.descriptor.clone();
    descriptor[103] ^= 1;
    let other_id: [u8; 32] = descriptor[103..135].try_into().unwrap();
    let auth = f.auth();
    let (first, other) = tokio::join!(
        f.server
            .declare_bootstrap_staging(&auth, &f.package.descriptor, f.budget()),
        second.declare_bootstrap_staging(&auth, &descriptor, f.budget())
    );
    assert_ne!(first.is_ok(), other.is_ok());
    f.server
        .cancel_bootstrap_staging(&auth, f.id)
        .await
        .unwrap();
    f.server
        .cancel_bootstrap_staging(&auth, other_id)
        .await
        .unwrap();
    descriptor[103] ^= 2;
    let id = descriptor[103..135].try_into().unwrap();
    let (cancel, declare) = tokio::join!(
        f.server.cancel_bootstrap_staging(&auth, id),
        second.declare_bootstrap_staging(&auth, &descriptor, f.budget())
    );
    cancel.unwrap();
    let _ = declare;
    assert_eq!(
        f.server.bootstrap_staging_status(&auth, id).await.unwrap(),
        Status::Canceled
    );
    assert!(
        second
            .declare_bootstrap_staging(&auth, &descriptor, f.budget())
            .await
            .is_err()
    );
}

mod publication;
