use super::domain::{Projection, strict_value};
use super::*;
use crate::sync::wire::ChangeWire;
use serde_json::json;
pub(super) fn authority() -> Authority {
    let (membership, keys) = crate::sync::seed_claim::membership::tests::content_authority();
    let b = membership.publication().binding();
    Authority {
        context: Context {
            vault: b.vault_id,
            genesis: membership.genesis().commitment(),
            device: [3; 32],
            credential_version: 1,
            head: membership.head(),
            stream: b.stream_id,
            descriptor: b.descriptor_commitment,
        },
        prefix: b.prefix_count as i64,
        membership,
        keys,
        association: "test".into(),
        sync_generation: 1,
    }
}

pub(super) fn change() -> ChangeWire {
    ChangeWire {
        change_id: "AAAAAAAAAAAAAAAA".into(),
        client_id: "origin".into(),
        local_seq: 1,
        entity_type: "task".into(),
        entity_id: "BBBBBBBBBBBBBBBB".into(),
        field: Some("deleted".into()),
        op_type: "set_field".into(),
        payload: json!({"workspace_id":"0000000000000000","workspace_key":"default","value":"1"}),
        base_version: Some("CCCCCCCCCCCCCCCC".into()),
        created_at: "2026-09-22T00:00:00Z".into(),
        server_seq: None,
    }
}
#[test]
fn exact_envelope_randomness_tamper_truncation_and_context() {
    let a = authority();
    let c = change();
    let record = codec::seal(&a, &c).unwrap();
    assert!(crate::sync::persistence::changes::canonical_equal(
        &c,
        &codec::open(&a, &record).unwrap()
    ));
    assert_ne!(record, codec::seal(&a, &c).unwrap());
    for n in 0..record.len() {
        assert!(codec::open(&a, &record[..n]).is_err());
    }
    for n in 0..record.len() {
        let mut bad = record.clone();
        bad[n] ^= 1;
        assert!(codec::open(&a, &bad).is_err());
    }
    let mut wrong = authority();
    wrong.context.stream = [9; 32];
    assert!(codec::open(&wrong, &record).is_err());
    let mut trailing = record.clone();
    trailing.push(0);
    assert!(codec::open(&a, &trailing).is_err());
    assert_eq!(record.len(), serde_json::to_vec(&c).unwrap().len() + 296);
}
#[test]
fn strict_json_and_numeric_equality() {
    for input in [
        r#"{"a":1,"a":2}"#,
        r#"{"p":{"value":1,"\u0076alue":1}}"#,
        "18446744073709551616",
        "-9223372036854775809",
        "1e999",
        "[] x",
    ] {
        assert!(strict_value(input.as_bytes()).is_err(), "{input}");
    }
    assert_eq!(
        strict_value(b"18446744073709551615").unwrap(),
        json!(u64::MAX)
    );
    assert_eq!(
        strict_value(b"-9223372036854775808").unwrap(),
        json!(i64::MIN)
    );
    assert_ne!(strict_value(b"1").unwrap(), strict_value(b"1.0").unwrap());
    assert_eq!(strict_value(b"1.0").unwrap(), strict_value(b"1e0").unwrap());
    assert_eq!(strict_value(b"-0").unwrap(), strict_value(b"0.0").unwrap());
    for depth in [64, 65] {
        let s = format!("{}0{}", "[".repeat(depth), "]".repeat(depth));
        assert_eq!(strict_value(s.as_bytes()).is_ok(), depth == 64);
    }
    let mut value = serde_json::to_value(change()).unwrap();
    value.as_object_mut().unwrap().remove("server_seq");
    assert!(domain::decode(&serde_json::to_vec(&value).unwrap()).is_err());
}
#[test]
fn parent_projection_limits_and_domain_binding() {
    for action in 0..=2 {
        let p = Projection::Parent {
            action,
            workspace: "w".repeat(256),
            task: "t".repeat(256),
            deleted: false,
            version: Some("v".repeat(256)),
        };
        let bytes = p.encode();
        assert_eq!(bytes.len(), 784);
        assert!(Projection::decode(&bytes).unwrap() == p);
    }
    let mut c = change();
    let p = domain::validate(&c).unwrap();
    assert!(
        p == Projection::Parent {
            action: 1,
            workspace: "0000000000000000".into(),
            task: c.entity_id.clone(),
            deleted: true,
            version: c.base_version.clone()
        }
    );
    c.op_type = "resolve_field".into();
    c.base_version = None;
    assert!(matches!(
        domain::validate(&c).unwrap(),
        Projection::Parent {
            action: 2,
            version: None,
            ..
        }
    ));
    c.payload["series_id"] = json!("x");
    assert!(domain::validate(&c).is_err());
}
#[test]
fn administration_operations_keep_strict_shapes_and_workspace_scope() {
    let admin = |op: &str, entity_type: &str, entity: &str, field: Option<&str>, payload| {
        let mut c = change();
        c.op_type = op.into();
        c.entity_type = entity_type.into();
        c.entity_id = entity.into();
        c.field = field.map(Into::into);
        c.base_version = None;
        c.payload = payload;
        c
    };
    let ws = json!({"workspace_id": "0000000000000000", "workspace_key": "default"});
    let with = |extra: serde_json::Value| {
        let mut p = ws.clone();
        p.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        p
    };
    let t = "2026-09-22T00:00:00Z";
    let valid = [
        admin(
            "set_project_metadata",
            "project",
            "PPPPPPPPPPPPPPPP",
            None,
            with(json!({"key": "core", "name": "Core", "prefix": "CORE", "updated_at": t})),
        ),
        admin(
            "project_delete",
            "project",
            "PPPPPPPPPPPPPPPP",
            None,
            with(json!({"deleted_at": t})),
        ),
        admin(
            "set_label_name",
            "label",
            "old",
            None,
            with(json!({"name": "old", "new_name": "new", "renamed_at": t})),
        ),
        admin(
            "label_delete",
            "label",
            "old",
            None,
            with(json!({"name": "old", "deleted_at": t})),
        ),
        admin(
            "label_restore",
            "label",
            "old",
            None,
            with(
                json!({"name": "old", "created_at": t, "task_ids": ["BBBBBBBBBBBBBBBB"],
                        "series_ids": [], "restored_at": t}),
            ),
        ),
        admin(
            "create_workspace",
            "workspace",
            "WWWWWWWWWWWWWWWW",
            None,
            json!({"key": "lab", "name": "Lab", "created_at": t}),
        ),
        admin(
            "set_workspace_field",
            "workspace",
            "WWWWWWWWWWWWWWWW",
            Some("name"),
            json!({"value": "Lab"}),
        ),
    ];
    for c in &valid {
        assert!(
            domain::validate(c).unwrap() == Projection::None,
            "{}",
            c.op_type
        );
        let bytes = serde_json::to_vec(c).unwrap();
        assert!(crate::sync::persistence::changes::canonical_equal(
            c,
            &domain::decode(&bytes).unwrap()
        ));
    }
    let mut rejected = Vec::new();
    for c in &valid {
        let mut extra = c.clone();
        extra.payload["unexpected"] = json!(1);
        let mut versioned = c.clone();
        versioned.base_version = Some("CCCCCCCCCCCCCCCC".into());
        let mut fielded = c.clone();
        fielded.field = if c.field.is_some() {
            None
        } else {
            Some("name".into())
        };
        rejected.extend([extra, versioned, fielded]);
        let mut unscoped = c.clone();
        if c.entity_type == "workspace" {
            unscoped.payload["workspace_id"] = json!("0000000000000000");
        } else {
            unscoped
                .payload
                .as_object_mut()
                .unwrap()
                .remove("workspace_id");
        }
        rejected.push(unscoped);
    }
    let mut same_name = valid[2].clone();
    same_name.payload["new_name"] = json!("old");
    let mut mismatched = valid[3].clone();
    mismatched.entity_id = "other".into();
    let mut workspace_field = valid[6].clone();
    workspace_field.field = Some("deleted".into());
    let mut workspace_id = valid[5].clone();
    workspace_id.entity_id = "not a workspace".into();
    let mut restore_ids = valid[4].clone();
    restore_ids.payload["task_ids"] = json!([1]);
    let mut unknown = valid[1].clone();
    unknown.op_type = "project_restore".into();
    rejected.extend([
        same_name,
        mismatched,
        workspace_field,
        workspace_id,
        restore_ids,
        unknown,
    ]);
    for c in rejected {
        assert!(domain::validate(&c).is_err(), "{c:?}");
    }
}
