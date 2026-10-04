use crate::{
    capability::Capability,
    driver::{Delivery, Driver, Request, exchange, required},
};
use aven_core::sync::{
    bootstrap_format::Package,
    client::bootstrap::components,
    seed_claim::{Publication, SeedAuthority},
};
use aven_protocol::wire::bootstrap::{self, Budget, Envelope, Operation, Reply};

fn control(seed: &SeedAuthority, operation: Operation) -> Request {
    Request::json(
        bootstrap::PATH,
        Some(seed.bearer()),
        &Envelope {
            vault: seed.genesis().context().vault_id,
            genesis: seed.genesis().commitment(),
            operation,
        },
    )
}

pub async fn completeness_and_retry(
    driver: &mut impl Driver,
    seed: &SeedAuthority,
    package: &Package,
    publication: &Publication,
) {
    required(driver, &[Capability::LostReply, Capability::Restart]);
    let binding = publication.binding();
    let parts = components(package);
    let budget = Budget {
        bytes: parts
            .iter()
            .flat_map(|(_, chunks)| chunks)
            .map(|bytes| bytes.len() as u64)
            .sum(),
        chunks: parts.iter().map(|(_, chunks)| chunks.len() as u64).sum(),
    };
    for _ in 0..2 {
        let Reply::Staging(status) = exchange(
            driver,
            control(
                seed,
                Operation::Declare {
                    descriptor: package.descriptor.clone(),
                    budget,
                },
            ),
        )
        .await
        .json() else {
            panic!("declaration not staged")
        };
        assert_eq!(status.budget, budget);
        assert_eq!(status.descriptor_commitment, binding.descriptor_commitment);
    }
    let publish = || {
        control(
            seed,
            Operation::Publish {
                bootstrap: binding.bootstrap_id,
                commitment: binding.descriptor_commitment,
                record: publication.record().to_vec(),
            },
        )
    };
    exchange(driver, publish())
        .await
        .refusal(400, "bootstrap-refused");
    for (component, chunks) in parts {
        for (index, bytes) in chunks.into_iter().enumerate() {
            let header = bootstrap::batch::Header {
                vault: seed.genesis().context().vault_id,
                genesis: seed.genesis().commitment(),
                bootstrap: binding.bootstrap_id,
                commitment: binding.descriptor_commitment,
                records: vec![bootstrap::batch::Slot {
                    component,
                    index: index as u64,
                    len: bytes.len() as u64,
                }],
            };
            let body = bootstrap::batch::encode(&header, &[bytes]).unwrap();
            for _ in 0..2 {
                let mut request = control(
                    seed,
                    Operation::Status {
                        bootstrap: binding.bootstrap_id,
                    },
                );
                request.content_type = bootstrap::batch::CONTENT_TYPE;
                request.body = body.clone();
                assert!(matches!(
                    exchange(driver, request).await.json::<Reply>(),
                    Reply::Stored
                ));
            }
        }
    }
    assert!(
        driver
            .request(publish(), Delivery::LoseReply)
            .await
            .unwrap()
            .is_none()
    );
    driver.restart().await.unwrap();
    for request in [
        publish(),
        control(
            seed,
            Operation::Status {
                bootstrap: binding.bootstrap_id,
            },
        ),
        control(
            seed,
            Operation::Cancel {
                bootstrap: binding.bootstrap_id,
            },
        ),
    ] {
        let Reply::Published(record) = exchange(driver, request).await.json() else {
            panic!("published retry lost immutable outcome")
        };
        assert_eq!(record, publication.record());
    }
}
