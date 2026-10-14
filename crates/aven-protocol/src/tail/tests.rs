use super::*;
use crate::claim::membership::{Device, Unauthorized, test_support};

fn context(membership: &Membership) -> Context {
    let binding = membership.publication().binding();
    Context {
        vault: binding.vault_id,
        genesis: binding.genesis_commitment,
        device: membership.genesis().device_id(),
        head: membership.head(),
        stream: binding.stream_id,
        descriptor: binding.descriptor_commitment,
    }
}

#[test]
fn current_credential_precedes_tail_binding_and_saved_head_checks() {
    let (seed, membership, _) = test_support::publication();
    let mut context = context(&membership);
    authenticate(&membership, &context, seed.bearer()).unwrap();
    context.stream = [0; 32];
    context.head = [0; 32];
    let error = authenticate(&membership, &context, &Secret::new([0; 32])).unwrap_err();
    assert!(error.is::<Unauthorized>());
    assert_eq!(
        authenticate(&membership, &context, seed.bearer())
            .unwrap_err()
            .to_string(),
        "error membership-context-unknown"
    );
    context.head = membership.head();
    assert_eq!(
        authenticate(&membership, &context, seed.bearer())
            .unwrap_err()
            .to_string(),
        "error encrypted-tail-invalid"
    );
}

#[test]
fn rotation_fences_fresh_content_but_not_current_authentication_or_exact_repair() {
    let (seed, membership, key) = test_support::publication();
    let old = membership.current_generation().id;
    admit_generations(&membership, [old]).unwrap();
    assert!(eligible_image(&membership, [0; 32], true).is_err());
    let freeze = Device::seed(&seed)
        .prepare_revoke(&membership, &[])
        .unwrap();
    let frozen = membership.append(&[], &[], &freeze).unwrap();
    authenticate(&frozen, &context(&frozen), seed.bearer()).unwrap();
    assert_eq!(
        admit_generations(&frozen, [old]).unwrap_err().to_string(),
        "error membership-rotation-pending"
    );
    let keys = membership.verify_initial_key(&key).unwrap();
    let rotation = Device::seed(&seed)
        .prepare_rotation(&frozen, &keys, frozen.publication().binding().prefix_count)
        .unwrap();
    let rotated = frozen.append(&[], &[], &rotation).unwrap();
    assert!(admit_generations(&rotated, [old]).is_err());
    assert!(eligible_image(&rotated, old, false).is_err());
    eligible_image(&rotated, old, true).unwrap();
    eligible_image(&rotated, rotated.current_generation().id, false).unwrap();
}
