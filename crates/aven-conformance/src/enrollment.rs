use crate::{
    capability::Capability,
    driver::{Delivery, Driver, Request, exchange, required},
};
use aven_core::sync::{
    client::enrollment::{Context, Operation, Reply},
    seed_claim::{
        SeedAuthority,
        membership::{Device, Joiner, Membership, VerifiedKeys},
    },
};
use aven_protocol::{
    claim::Secret,
    wire::enrollment::{PATH, RegistrationStatus},
};

fn context(membership: &Membership, device: [u8; 32]) -> Context {
    Context {
        vault: membership.genesis().context().vault_id,
        genesis: membership.genesis().commitment(),
        head: membership.head(),
        device,
    }
}

pub async fn expiry_and_admission(
    driver: &mut impl Driver,
    seed: &SeedAuthority,
    membership: &Membership,
    keys: &VerifiedKeys,
    now: u64,
) -> (Joiner, Membership) {
    required(
        driver,
        &[
            Capability::ControlledClock,
            Capability::LostReply,
            Capability::Restart,
        ],
    );
    driver.set_time(now).unwrap();
    let device = Device::seed(seed);
    let ctx = context(membership, seed.genesis().device_id());
    let (expired, declaration) = device.prepare_invitation(membership, now + 10).unwrap();
    let register = |bytes: Vec<u8>| {
        Request::json(
            PATH,
            Some(seed.bearer()),
            &Operation::Register {
                context: ctx.clone(),
                declaration: bytes,
            },
        )
    };
    assert!(matches!(
        exchange(driver, register(declaration.record().to_vec()))
            .await
            .json::<Reply>(),
        Reply::Registered(RegistrationStatus::Open)
    ));
    let expired_peer = Joiner::generate(expired).unwrap();
    driver.set_time(now + 11).unwrap();
    exchange(
        driver,
        Request::json(
            PATH,
            None,
            &Operation::Post {
                vault: expired_peer.vault(),
                handle: expired_peer.handle(),
                request: expired_peer.request().to_vec(),
            },
        ),
    )
    .await
    .refusal(400, "enrollment-refused");
    let Reply::Mailbox(mailbox) = exchange(
        driver,
        Request::json(
            PATH,
            None,
            &Operation::Mailbox {
                vault: expired_peer.vault(),
                handle: expired_peer.handle(),
            },
        ),
    )
    .await
    .json() else {
        panic!("mailbox missing")
    };
    assert!(mailbox.request.is_none());
    assert!(matches!(
        exchange(driver, register(declaration.record().to_vec()))
            .await
            .json::<Reply>(),
        Reply::Registered(RegistrationStatus::Expired)
    ));

    let (invitation, declaration) = device.prepare_invitation(membership, now + 100).unwrap();
    assert!(matches!(
        exchange(driver, register(declaration.record().to_vec()))
            .await
            .json::<Reply>(),
        Reply::Registered(RegistrationStatus::Open)
    ));
    let peer = Joiner::generate(
        aven_core::sync::seed_claim::membership::Invitation::from_protected_storage(
            &invitation.protected_storage_bytes(),
        )
        .unwrap(),
    )
    .unwrap();
    let post = || {
        Request::json(
            PATH,
            None,
            &Operation::Post {
                vault: peer.vault(),
                handle: peer.handle(),
                request: peer.request().to_vec(),
            },
        )
    };
    for _ in 0..2 {
        assert!(matches!(
            exchange(driver, post()).await.json::<Reply>(),
            Reply::Done
        ));
    }
    let record = device
        .prepare_admission(membership, &declaration, &invitation, peer.request(), keys)
        .unwrap();
    let admit = || {
        Request::json(
            PATH,
            Some(seed.bearer()),
            &Operation::Admit {
                context: ctx.clone(),
                handle: peer.handle(),
                record: record.clone(),
            },
        )
    };
    assert!(
        driver
            .request(admit(), Delivery::LoseReply)
            .await
            .unwrap()
            .is_none()
    );
    driver.restart().await.unwrap();
    exchange(driver, admit())
        .await
        .refusal(409, "membership-stale");
    let current = membership
        .append(declaration.record(), peer.request(), &record)
        .unwrap();
    let retry = Request::json(
        PATH,
        Some(seed.bearer()),
        &Operation::Admit {
            context: context(&current, seed.genesis().device_id()),
            handle: peer.handle(),
            record: record.clone(),
        },
    );
    let Reply::Admitted(saved) = exchange(driver, retry).await.json() else {
        panic!("admission exact retry failed")
    };
    assert_eq!(saved, record);
    let peer_ctx = context(&current, peer.device());
    let Reply::Membership(evidence) = exchange(
        driver,
        Request::json(
            PATH,
            Some(peer.bearer()),
            &Operation::Membership { context: peer_ctx },
        ),
    )
    .await
    .json() else {
        panic!("admitted peer cannot read membership")
    };
    assert_eq!(evidence.verify().unwrap().head(), current.head());
    // A wrong credential is not entitled to the stale-head disclosure.
    exchange(
        driver,
        Request::json(
            PATH,
            Some(&Secret::new([0; 32])),
            &Operation::Membership {
                context: ctx.clone(),
            },
        ),
    )
    .await
    .refusal(403, "enrollment-unauthorized");
    let Reply::Membership(refreshed) = exchange(
        driver,
        Request::json(
            PATH,
            Some(seed.bearer()),
            &Operation::Membership {
                context: ctx.clone(),
            },
        ),
    )
    .await
    .json() else {
        panic!("stale member cannot refresh")
    };
    assert_eq!(refreshed.verify().unwrap().head(), current.head());
    exchange(
        driver,
        Request::json(
            PATH,
            Some(seed.bearer()),
            &Operation::PrepareManagement { context: ctx },
        ),
    )
    .await
    .refusal(409, "membership-stale");
    (peer, current)
}
