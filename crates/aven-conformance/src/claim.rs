use crate::{
    capability::Capability,
    driver::{Delivery, Driver, Request, exchange, required},
};
use aven_protocol::{
    claim::{Genesis, Secret},
    wire::bootstrap::{Envelope, Operation, PATH, Reply},
};

fn claim(genesis: &Genesis, secret: &Secret, bearer: bool) -> Request {
    Request::json(
        PATH,
        Some(secret),
        &Envelope {
            vault: genesis.context().vault_id,
            genesis: genesis.commitment(),
            operation: if bearer {
                Operation::ClaimBearer {
                    bytes: genesis.claim_bytes(),
                }
            } else {
                Operation::ClaimSetup {
                    bytes: genesis.claim_bytes(),
                }
            },
        },
    )
}

pub async fn reissue_before_claim(driver: &impl Driver, genesis: &Genesis, setup: &Secret) {
    required(driver, &[Capability::Barrier]);
    driver
        .provision(genesis.setup_id(), setup, u64::MAX)
        .await
        .unwrap();
    driver.arm_barrier().unwrap();
    let (reply, ()) = tokio::join!(exchange(driver, claim(genesis, setup, false)), async {
        driver.wait_barrier().await.unwrap();
        driver
            .provision(genesis.setup_id(), &Secret::new([8; 32]), u64::MAX)
            .await
            .unwrap();
        driver.release_barrier().unwrap();
    });
    reply.refusal(403, "bootstrap-setup-invitation-rejected");
    driver
        .provision(genesis.setup_id(), setup, u64::MAX)
        .await
        .unwrap();
}

pub async fn retry_after_lost_reply(
    driver: &mut impl Driver,
    genesis: &Genesis,
    setup: &Secret,
    bearer: &Secret,
) {
    required(driver, &[Capability::LostReply, Capability::Restart]);
    exchange(driver, claim(genesis, &Secret::new([0; 32]), true))
        .await
        .refusal(403, "bootstrap-setup-invitation-rejected");
    assert!(
        driver
            .request(claim(genesis, setup, false), Delivery::LoseReply)
            .await
            .unwrap()
            .is_none()
    );
    driver.restart().await.unwrap();
    for (secret, bearer_auth) in [(setup, false), (bearer, true)] {
        let Reply::Claimed {
            vault,
            claim: id,
            genesis: commitment,
        } = exchange(driver, claim(genesis, secret, bearer_auth))
            .await
            .json()
        else {
            panic!("claim retry changed outcome")
        };
        assert_eq!(vault, genesis.context().vault_id);
        assert_eq!(id, genesis.claim_id());
        assert_eq!(commitment, genesis.commitment());
    }
    exchange(driver, claim(genesis, &Secret::new([0; 32]), true))
        .await
        .refusal(409, "bootstrap-storage-already-claimed");
}
