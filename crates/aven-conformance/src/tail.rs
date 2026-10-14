use crate::{
    capability::Capability,
    driver::{Delivery, Driver, Request, exchange, required},
};
use aven_core::sync::seed_claim::membership::{Joiner, Membership};
use aven_protocol::{
    claim::Secret,
    wire::tail::{self, Context, Envelope, Operation, Reply},
};

pub async fn admitted_peer(driver: &impl Driver, peer: &Joiner, membership: &Membership) {
    let binding = membership.publication().binding();
    let context = Context {
        vault: peer.vault(),
        genesis: membership.genesis().commitment(),
        device: peer.device(),
        head: membership.head(),
        stream: binding.stream_id,
        descriptor: binding.descriptor_commitment,
    };
    let request = |operation, secret: &Secret| {
        Request::json(
            tail::PATH,
            Some(secret),
            &Envelope {
                context: context.clone(),
                correlation: [4; 32],
                operation,
            },
        )
    };
    let response: Envelope<Reply> = exchange(driver, request(Operation::Features, peer.bearer()))
        .await
        .json();
    assert_eq!(response.context.head, membership.head());
    assert_eq!(response.correlation, [4; 32]);
    let Reply::Features(features) = response.operation else {
        panic!("tail features absent")
    };
    assert_eq!(features.count, tail::BATCH_COUNT);
    assert_eq!(features.bytes, tail::BATCH_BYTES);
    let after = binding.prefix_count as i64;
    let response: Envelope<Reply> = exchange(
        driver,
        request(
            Operation::Pull {
                after,
                limit: 1,
                watermark: None,
            },
            peer.bearer(),
        ),
    )
    .await
    .json();
    let Reply::Page(page) = response.operation else {
        panic!("tail pull absent")
    };
    assert_eq!(page.after, after);
    assert_eq!(page.cursor, after);
    assert_eq!(page.watermark, after);
    assert!(!page.has_more);
    assert!(page.records.is_empty());
    exchange(driver, request(Operation::Features, &Secret::new([0; 32])))
        .await
        .refusal(403, "encrypted-tail-unauthorized");
}

pub async fn append_retry(
    driver: &mut impl Driver,
    context: &Context,
    bearer: &Secret,
    record: &[u8],
    prefix: i64,
) {
    required(driver, &[Capability::LostReply, Capability::Restart]);
    let request = |operation| {
        Request::json(
            tail::PATH,
            Some(bearer),
            &Envelope {
                context: context.clone(),
                correlation: [5; 32],
                operation,
            },
        )
    };
    let append = || {
        request(Operation::Append {
            ticket: None,
            record: record.to_vec(),
        })
    };
    assert!(
        driver
            .request(append(), Delivery::LoseReply)
            .await
            .unwrap()
            .is_none()
    );
    driver.restart().await.unwrap();
    let response: Envelope<Reply> = exchange(driver, append()).await.json();
    let Reply::Appended(mapping) = response.operation else {
        panic!("append retry failed")
    };
    assert_eq!(mapping.sequence, prefix + 1);
    let response: Envelope<Reply> = exchange(
        driver,
        request(Operation::Lookup {
            operation_id: mapping.operation_id.clone(),
            expected: Some(mapping.clone()),
        }),
    )
    .await
    .json();
    let Reply::Found(accepted) = response.operation else {
        panic!("saved append absent")
    };
    assert!(accepted.mapping == mapping);
    assert_eq!(accepted.record, record);
    exchange(
        driver,
        request(Operation::Append {
            ticket: None,
            record: record[..20].to_vec(),
        }),
    )
    .await
    .refusal(400, "encrypted-tail-refused");
    let response: Envelope<Reply> = exchange(
        driver,
        request(Operation::Pull {
            after: prefix,
            limit: 10,
            watermark: None,
        }),
    )
    .await
    .json();
    assert!(response.context == *context);
    assert_eq!(response.correlation, [5; 32]);
    let Reply::Page(page) = response.operation else {
        panic!("accepted append not visible")
    };
    assert_eq!(page.cursor, mapping.sequence);
    assert_eq!(page.watermark, mapping.sequence);
    assert!(!page.has_more);
    assert_eq!(page.records.len(), 1);
    assert!(page.records[0].mapping == mapping);
    assert_eq!(page.records[0].record, record);
}
