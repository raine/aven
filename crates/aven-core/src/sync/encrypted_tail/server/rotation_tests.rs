use super::*;
use crate::sync::encrypted_tail::attachments::{self as image, codec::Descriptor};
use crate::sync::seed_claim::{
    Secret,
    membership::{Device, Joiner, Membership, test_support::Fixture},
};
use crate::sync::wire::ChangeWire;
const WORKSPACE: &str = "0000000000000000";

struct Peers {
    f: Fixture,
    peer: Joiner,
    third: Joiner,
    m: Membership,
}
impl Peers {
    async fn new() -> Self {
        let f = Fixture::new().await;
        let mut m = Membership::from_publication(
            f.seed.genesis(),
            &f.package.descriptor,
            f.publication.record(),
        )
        .unwrap();
        let peer = enroll(&f, &mut m).await;
        let third = enroll(&f, &mut m).await;
        Self { f, peer, third, m }
    }
    fn authority(&self, m: &Membership, device: [u8; 32]) -> Authority {
        Authority {
            context: Context {
                vault: m.genesis().context().vault_id,
                genesis: m.genesis().commitment(),
                device,
                head: m.head(),
                stream: m.publication().binding().stream_id,
                descriptor: m.publication().binding().descriptor_commitment,
            },
            membership: m.clone(),
            keys: Membership::from_publication(
                self.f.seed.genesis(),
                &self.f.package.descriptor,
                self.f.publication.record(),
            )
            .unwrap()
            .verify_initial_key(&self.f.key)
            .unwrap(),
            prefix: m.publication().binding().prefix_count as i64,
            association: "test".into(),
            sync_generation: 1,
        }
    }
}
async fn enroll(f: &Fixture, m: &mut Membership) -> Joiner {
    let expiry = chrono::Utc::now().timestamp() as u64 + 3600;
    let (inv, d) = Device::seed(&f.seed).prepare_invitation(m, expiry).unwrap();
    let peer = Joiner::generate(
        crate::sync::seed_claim::membership::Invitation::from_protected_storage(
            &inv.protected_storage_bytes(),
        )
        .unwrap(),
    )
    .unwrap();
    let record = Device::seed(&f.seed)
        .prepare_admission(
            m,
            &d,
            &inv,
            peer.request(),
            &m.verify_initial_key(&f.key).unwrap(),
        )
        .unwrap();
    let auth = crate::sync::seed_claim::peer::Authentication {
        vault: m.genesis().context().vault_id,
        genesis: m.genesis().commitment(),
        device: f.seed.genesis().device_id(),
        head: m.head(),
        bearer: f.seed.bearer(),
    };
    let now = crate::sync::seed_claim::membership::now().unwrap();
    f.db.register_membership_invitation_at(&auth, d.record(), now)
        .await
        .unwrap();
    f.db.post_membership_request_at(auth.vault, d.handle(), peer.request(), now)
        .await
        .unwrap();
    f.db.admit_membership_device_at(&auth, d.handle(), &record, now)
        .await
        .unwrap();
    *m = m.append(d.record(), peer.request(), &record).unwrap();
    peer
}
fn change(id: &str) -> ChangeWire {
    let mut c = super::super::tests::change();
    c.change_id = id.into();
    c
}
async fn append(
    db: &Database,
    a: &Authority,
    b: &Secret,
    record: &[u8],
    ticket: Option<image::Ticket>,
) -> Result<Reply> {
    db.encrypted_tail_exchange(
        &a.context,
        b,
        Operation::Append {
            record: record.to_vec(),
            ticket,
        },
    )
    .await
}
async fn img(
    db: &Database,
    a: &Authority,
    b: &Secret,
    op: image::Operation,
) -> Result<image::Reply> {
    db.encrypted_image_exchange(&a.context, b, op, Default::default())
        .await
}
fn declare(d: &Descriptor) -> image::Operation {
    image::Operation::Declare {
        workspace: WORKSPACE.into(),
        descriptor: d.encode().unwrap(),
    }
}
fn status(d: &Descriptor) -> image::Operation {
    image::Operation::Status {
        workspace: WORKSPACE.into(),
        object: d.object,
        descriptor_commitment: hash(&d.encode().unwrap()),
    }
}
fn read(d: &Descriptor) -> image::Operation {
    image::Operation::Read {
        workspace: WORKSPACE.into(),
        object: d.object,
        descriptor_commitment: hash(&d.encode().unwrap()),
        index: 0,
    }
}
fn mutations(d: &Descriptor, t: &image::Ticket, chunk: &[u8]) -> Vec<image::Operation> {
    let commitment = hash(&d.encode().unwrap());
    vec![
        declare(d),
        image::Operation::Put {
            workspace: WORKSPACE.into(),
            object: d.object,
            descriptor_commitment: commitment,
            reservation: t.reservation,
            index: 0,
            record: chunk.to_vec(),
        },
        image::Operation::Complete {
            workspace: WORKSPACE.into(),
            object: d.object,
            descriptor_commitment: commitment,
            reservation: t.reservation,
        },
        image::Operation::Release {
            workspace: WORKSPACE.into(),
            object: d.object,
            descriptor_commitment: commitment,
            reservation: t.reservation,
        },
    ]
}
async fn upload(
    db: &Database,
    a: &Authority,
    b: &Secret,
    d: &Descriptor,
    chunks: &[Vec<u8>],
) -> image::Ticket {
    let image::Reply::Status(s) = img(db, a, b, declare(d)).await.unwrap() else {
        panic!()
    };
    let t = image::Ticket {
        reservation: s.reservation.unwrap(),
    };
    let ops = mutations(d, &t, &chunks[0]);
    img(db, a, b, ops[1].clone()).await.unwrap();
    img(db, a, b, ops[2].clone()).await.unwrap();
    t
}
fn reference(a: &Authority, d: &Descriptor, id: &str, reference: &str) -> Vec<u8> {
    let mut c = change(id);
    c.op_type = "attachment_add".into();
    c.field = Some("attachments".into());
    c.base_version = None;
    c.payload = serde_json::json!({"workspace_id":WORKSPACE,"workspace_key":"default","attachment_id":reference,"filename":"test.png","media_type":"image/png","byte_size":d.artifact.total,"sha256":"a".repeat(64),"alt_text":null,"width":1,"height":1,"created_at":"2026-09-23T00:00:00Z"});
    crate::sync::wire::validate_local_change_shape(&c).unwrap();
    let p = domain::Projection::Ref {
        workspace: WORKSPACE.into(),
        task: c.entity_id.clone(),
        reference: reference.into(),
        descriptor: d.encode().unwrap(),
        deleted: false,
        version: None,
    };
    codec::seal_projection(a, &c, &p).unwrap()
}

#[tokio::test]
async fn three_peers_frozen_outcomes_and_historical_images_survive_rotation() {
    let p = Peers::new().await;
    let seed = p.f.seed.genesis().device_id();
    let old = p.authority(&p.m, seed);
    let accepted = codec::seal(&old, &change("DDDDDDDDDDDDDDDD")).unwrap();
    let unknown = codec::seal(&old, &change("EEEEEEEEEEEEEEEE")).unwrap();
    append(&p.f.db, &old, p.f.seed.bearer(), &accepted, None)
        .await
        .unwrap();
    let (d, chunks) = Descriptor::seal(&old, b"synthetic image ciphertext").unwrap();
    let t = upload(&p.f.db, &old, p.f.seed.bearer(), &d, &chunks).await;
    let r = reference(&old, &d, "FFFFFFFFFFFFFFFF", "GGGGGGGGGGGGGGGG");
    append(&p.f.db, &old, p.f.seed.bearer(), &r, Some(t.clone()))
        .await
        .unwrap();
    let (orphan, orphan_chunks) = Descriptor::seal(&old, b"unadmitted bytes").unwrap();
    let orphan_ticket = upload(&p.f.db, &old, p.f.seed.bearer(), &orphan, &orphan_chunks).await;
    let survivor = p.authority(&p.m, p.peer.device());
    let revoke = p.peer.authority().prepare_revoke(&p.m, &[seed]).unwrap();
    p.f.db
        .apply_membership_management(&survivor.context.authentication(p.peer.bearer()), &revoke)
        .await
        .unwrap();
    let pending = p.m.append(&[], &[], &revoke).unwrap();
    let frozen = p.authority(&pending, p.peer.device());
    for operation in [
        Operation::Append {
            record: accepted.clone(),
            ticket: None,
        },
        Operation::Lookup {
            operation_id: "DDDDDDDDDDDDDDDD".into(),
            expected: None,
        },
        Operation::Pull {
            after: old.prefix,
            limit: 16,
            watermark: None,
        },
    ] {
        assert!(
            p.f.db
                .encrypted_tail_exchange(&old.context, p.f.seed.bearer(), operation.clone())
                .await
                .err()
                .unwrap()
                .to_string()
                .contains("unauthorized")
        );
        p.f.db
            .encrypted_tail_exchange(&frozen.context, p.peer.bearer(), operation)
            .await
            .unwrap();
    }
    assert!(
        append(&p.f.db, &frozen, p.peer.bearer(), &unknown, None)
            .await
            .is_err()
    );
    for op in mutations(&orphan, &orphan_ticket, &orphan_chunks[0]) {
        assert!(
            img(&p.f.db, &old, p.f.seed.bearer(), op.clone())
                .await
                .err()
                .unwrap()
                .to_string()
                .contains("unauthorized")
        );
        assert!(img(&p.f.db, &frozen, p.peer.bearer(), op).await.is_err());
    }
    img(&p.f.db, &frozen, p.peer.bearer(), read(&d))
        .await
        .unwrap();
    img(&p.f.db, &frozen, p.peer.bearer(), status(&orphan))
        .await
        .unwrap();
    assert!(
        img(&p.f.db, &old, p.f.seed.bearer(), read(&d))
            .await
            .is_err()
    );
    let tickets: i64 =
        sqlx::query_scalar("SELECT count(*) FROM server_e2ee_image_tickets WHERE device=?")
            .bind(seed.as_slice())
            .fetch_one(&mut *p.f.db.acquire_reader().await.unwrap())
            .await
            .unwrap();
    assert_eq!(tickets, 0);
    let prep =
        p.f.db
            .prepare_membership_management(&frozen.context.authentication(p.peer.bearer()))
            .await
            .unwrap();
    let keys = pending.verify_initial_key(&p.f.key).unwrap();
    let wrong = p
        .peer
        .authority()
        .prepare_rotation(&pending, &keys, prep.high_water - 1)
        .unwrap();
    assert!(
        p.f.db
            .apply_membership_management(&frozen.context.authentication(p.peer.bearer()), &wrong)
            .await
            .unwrap_err()
            .to_string()
            .contains("cutoff")
    );
    let rotation = p
        .peer
        .authority()
        .prepare_rotation(&pending, &keys, prep.high_water)
        .unwrap();
    p.f.db
        .apply_membership_management(&frozen.context.authentication(p.peer.bearer()), &rotation)
        .await
        .unwrap();
    let m = pending.append(&[], &[], &rotation).unwrap();
    let keys = p
        .peer
        .authority()
        .receive_rotation(&pending, &rotation, &keys)
        .unwrap();
    let mut active = p.authority(&m, p.peer.device());
    active.keys = keys;
    append(&p.f.db, &active, p.peer.bearer(), &accepted, None)
        .await
        .unwrap();
    assert!(
        append(&p.f.db, &active, p.peer.bearer(), &unknown, None)
            .await
            .is_err()
    );
    assert!(
        img(&p.f.db, &active, p.peer.bearer(), status(&orphan))
            .await
            .is_err()
    );
    for op in mutations(&orphan, &orphan_ticket, &orphan_chunks[0])
        .into_iter()
        .take(4)
        .chain([read(&orphan)])
    {
        assert!(img(&p.f.db, &active, p.peer.bearer(), op).await.is_err());
    }
    let stale_ref = reference(&active, &orphan, "HHHHHHHHHHHHHHHH", "PPPPPPPPPPPPPPPP");
    assert!(
        append(
            &p.f.db,
            &active,
            p.peer.bearer(),
            &stale_ref,
            Some(orphan_ticket)
        )
        .await
        .is_err()
    );
    let reuse = reference(&active, &d, "JJJJJJJJJJJJJJJJ", "KKKKKKKKKKKKKKKK");
    append(&p.f.db, &active, p.peer.bearer(), &reuse, None)
        .await
        .unwrap();
    // Missing stored bytes do not erase authenticated admission provenance.
    sqlx::query("DELETE FROM server_e2ee_image_chunks WHERE object=?")
        .bind(d.object.as_slice())
        .execute(&mut *p.f.db.acquire_writer().await.unwrap())
        .await
        .unwrap();
    sqlx::query("UPDATE server_e2ee_images SET complete=0 WHERE object=?")
        .bind(d.object.as_slice())
        .execute(&mut *p.f.db.acquire_writer().await.unwrap())
        .await
        .unwrap();
    upload(&p.f.db, &active, p.peer.bearer(), &d, &chunks).await;
    let image::Reply::Chunk(repaired) = img(&p.f.db, &active, p.peer.bearer(), read(&d))
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        d.open(&old, &[repaired]).unwrap().as_slice(),
        b"synthetic image ciphertext"
    );
    let fresh = codec::seal(&active, &change("QQQQQQQQQQQQQQQQ")).unwrap();
    append(&p.f.db, &active, p.peer.bearer(), &fresh, None)
        .await
        .unwrap();
    let (new_image, new_chunks) = Descriptor::seal(&active, b"new generation image").unwrap();
    let t = upload(&p.f.db, &active, p.peer.bearer(), &new_image, &new_chunks).await;
    let r = reference(&active, &new_image, "MMMMMMMMMMMMMMMM", "NNNNNNNNNNNNNNNN");
    append(&p.f.db, &active, p.peer.bearer(), &r, Some(t))
        .await
        .unwrap();
    let third_keys = p
        .third
        .authority()
        .receive_rotation(
            &pending,
            &rotation,
            &pending.verify_initial_key(&p.f.key).unwrap(),
        )
        .unwrap();
    let mut third = p.authority(&m, p.third.device());
    third.keys = third_keys;
    let Reply::Page(page) =
        p.f.db
            .encrypted_tail_exchange(
                &third.context,
                p.third.bearer(),
                Operation::Pull {
                    after: old.prefix,
                    limit: 16,
                    watermark: None,
                },
            )
            .await
            .unwrap()
    else {
        panic!()
    };
    assert_eq!(page.records.len(), 5);
    assert_eq!(
        codec::open(&third, &fresh).unwrap().change_id,
        "QQQQQQQQQQQQQQQQ"
    );
    let image::Reply::Chunk(bytes) = img(&p.f.db, &third, p.third.bearer(), read(&new_image))
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        new_image.open(&third, &[bytes]).unwrap().as_slice(),
        b"new generation image"
    );
    assert_eq!(
        p.f.db
            .apply_membership_management(&third.context.authentication(p.third.bearer()), &rotation)
            .await
            .unwrap(),
        rotation
    );
}

#[tokio::test]
async fn independent_pool_revoke_races_content_put_and_ref_at_one_boundary() {
    for kind in 0..3 {
        let p = Peers::new().await;
        let old = p.authority(&p.m, p.f.seed.genesis().device_id());
        let survivor = p.authority(&p.m, p.peer.device());
        let other = Database::open(&p.f.dir.path().join("server.db"))
            .await
            .unwrap();
        let (d, chunks) = Descriptor::seal(&old, b"race image").unwrap();
        let ticket = upload(&p.f.db, &old, p.f.seed.bearer(), &d, &chunks).await;
        let revoke = p
            .peer
            .authority()
            .prepare_revoke(&p.m, &[old.context.device])
            .unwrap();
        let auth = survivor.context.authentication(p.peer.bearer());
        let content = async {
            if kind == 1 {
                img(
                    &other,
                    &old,
                    p.f.seed.bearer(),
                    mutations(&d, &ticket, &chunks[0])[1].clone(),
                )
                .await
                .map(|_| ())
            } else {
                let record = if kind == 0 {
                    codec::seal(&old, &change("RRRRRRRRRRRRRRRR")).unwrap()
                } else {
                    reference(&old, &d, "RRRRRRRRRRRRRRRR", "SSSSSSSSSSSSSSSS")
                };
                append(
                    &other,
                    &old,
                    p.f.seed.bearer(),
                    &record,
                    (kind == 2).then(|| ticket.clone()),
                )
                .await
                .map(|_| ())
            }
        };
        let (r, c) = tokio::join!(p.f.db.apply_membership_management(&auth, &revoke), content);
        r.unwrap();
        if let Err(e) = &c {
            assert!(e.to_string().contains("unauthorized"), "kind {kind}: {e:#}");
        }
        let m = p.m.append(&[], &[], &revoke).unwrap();
        let active = p.authority(&m, p.peer.device());
        let prep =
            p.f.db
                .prepare_membership_management(&active.context.authentication(p.peer.bearer()))
                .await
                .unwrap();
        assert_eq!(
            prep.high_water,
            old.prefix as u64 + u64::from(kind != 1 && c.is_ok())
        );
        let origin: Option<String> =
            sqlx::query_scalar("SELECT origin FROM server_e2ee_images WHERE object=?")
                .bind(d.object.as_slice())
                .fetch_one(&mut *p.f.db.acquire_reader().await.unwrap())
                .await
                .unwrap();
        assert_eq!(origin.is_some(), kind == 2 && c.is_ok());
        assert!(
            img(&other, &old, p.f.seed.bearer(), status(&d))
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn failed_ticket_revocation_restores_head_credential_and_all_tickets() {
    let p = Peers::new().await;
    let old = p.authority(&p.m, p.f.seed.genesis().device_id());
    let survivor = p.authority(&p.m, p.peer.device());
    let (d, chunks) = Descriptor::seal(&old, b"ticket rollback").unwrap();
    let ticket = upload(&p.f.db, &old, p.f.seed.bearer(), &d, &chunks).await;
    let revoke = p
        .peer
        .authority()
        .prepare_revoke(&p.m, &[old.context.device])
        .unwrap();
    sqlx::query("CREATE TRIGGER fail_ticket BEFORE DELETE ON server_e2ee_image_tickets BEGIN SELECT RAISE(ABORT,'fault'); END").execute(&mut *p.f.db.acquire_writer().await.unwrap()).await.unwrap();
    assert!(
        p.f.db
            .apply_membership_management(&survivor.context.authentication(p.peer.bearer()), &revoke)
            .await
            .is_err()
    );
    sqlx::query("DROP TRIGGER fail_ticket")
        .execute(&mut *p.f.db.acquire_writer().await.unwrap())
        .await
        .unwrap();
    assert_eq!(
        p.f.db
            .membership_evidence(&old.context.authentication(p.f.seed.bearer()))
            .await
            .unwrap()
            .verify()
            .unwrap()
            .head(),
        p.m.head()
    );
    let image::Reply::Status(s) = img(&p.f.db, &old, p.f.seed.bearer(), status(&d))
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(s.reservation, Some(ticket.reservation));
    let second = Database::open(&p.f.dir.path().join("server.db"))
        .await
        .unwrap();
    img(
        &second,
        &old,
        p.f.seed.bearer(),
        mutations(&d, &ticket, &chunks[0])[1].clone(),
    )
    .await
    .unwrap();
}
