use super::super::*;
use crate::{
    db::Database,
    sync::{bootstrap_format, bootstrap_staging as staging},
};

pub(crate) struct Fixture {
    pub(crate) dir: tempfile::TempDir,
    pub(crate) db: Database,
    pub(crate) seed: SeedAuthority,
    pub(crate) key: LocalSharedStatePackageKey,
    pub(crate) package: bootstrap_format::Package,
    pub(crate) publication: Publication,
}
impl Fixture {
    pub(crate) async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let source = Database::open(&dir.path().join("source.db")).await.unwrap();
        let context = LocalSharedStatePackageContext {
            vault_id: [1; 32],
            generation_id: [2; 32],
        };
        let key = LocalSharedStatePackageKey::new([3; 32]);
        let seed = SeedAuthority::generate(context, &key, [4; 32]).unwrap();
        source
            .capture_local_shared_state_never_dispatched(dir.path())
            .await
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
        let p = seed.prepare_bootstrap_publication(&package, &key).unwrap();
        let db = Database::open(&dir.path().join("server.db")).await.unwrap();
        let setup_secret = Secret::new([5; 32]);
        let setup = SetupAuthority::from_verifier(
            [4; 32],
            SetupAuthority::verifier([4; 32], &setup_secret),
        );
        db.admit_seed_claim(
            &seed.genesis().claim_bytes(),
            Some(&setup),
            ClaimAuthentication::SetupSecret(&setup_secret),
        )
        .await
        .unwrap();
        let auth = staging::Authentication {
            vault_id: context.vault_id,
            genesis_commitment: seed.genesis().commitment(),
            bearer: seed.bearer(),
        };
        let mut components = Vec::new();
        for (i, c) in [
            staging::Component::DataCatalog,
            staging::Component::PrefixCatalog,
            staging::Component::ImageCatalog,
        ]
        .into_iter()
        .enumerate()
        {
            components.push((c, package.catalogs[i].chunks(1048576).collect::<Vec<_>>()));
        }
        components.push((
            staging::Component::Manifest,
            package.manifest.iter().map(Vec::as_slice).collect(),
        ));
        components.push((
            staging::Component::State,
            package.state.iter().map(Vec::as_slice).collect(),
        ));
        let budget = staging::Budget {
            bytes: components
                .iter()
                .flat_map(|(_, v)| v)
                .map(|v| v.len() as u64)
                .sum(),
            chunks: components.iter().map(|(_, v)| v.len() as u64).sum(),
        };
        let status = db
            .declare_bootstrap_staging(&auth, &package.descriptor, budget)
            .await
            .unwrap();
        for (component, records) in components {
            for (index, bytes) in records.iter().enumerate() {
                db.put_bootstrap_chunk(
                    &auth,
                    staging::PutChunk {
                        bootstrap_id: p.binding().bootstrap_id,
                        descriptor_commitment: p.binding().descriptor_commitment,
                        epoch: status.epoch,
                        component,
                        index: index as u64,
                        bytes,
                    },
                )
                .await
                .unwrap();
            }
        }
        db.publish_bootstrap(
            &auth,
            staging::PublishBootstrap {
                bootstrap_id: p.binding().bootstrap_id,
                descriptor_commitment: p.binding().descriptor_commitment,
                epoch: status.epoch,
                record: p.record(),
            },
            Default::default(),
        )
        .await
        .unwrap();
        Self {
            dir,
            db,
            seed,
            key,
            package,
            publication: p,
        }
    }
}
