use super::test_support::{make_change_wire, test_workspace};
use super::*;
use crate::change_log::{ChangePayload, op_type};

#[test]
fn sync_timestamps_accept_fractional_utc_precision() {
    validate_timestamp_value("created_at", "2026-07-28T06:41:44Z").unwrap();
    validate_timestamp_value("created_at", "2026-07-28T06:41:44.000000Z").unwrap();
    validate_timestamp_value("created_at", "2026-07-28T06:41:44.123456789Z").unwrap();
}

#[test]
fn sync_timestamps_reject_offsets_and_invalid_calendar_values() {
    for value in ["2026-07-28T08:41:44+02:00", "2026-13-28T06:41:44Z", "today"] {
        assert!(validate_timestamp_value("created_at", value).is_err());
    }
}

fn attachment_add_payload() -> Value {
    ChangePayload::workspace(&test_workspace())
        .set("attachment_id", "7KQ9A1X4MV2P8D6R")
        .set(
            "sha256",
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        )
        .set("byte_size", 12_i64)
        .set("media_type", "image/png")
        .set("filename", None::<&str>)
        .set("alt_text", None::<&str>)
        .set("width", 320_i64)
        .set("height", 240_i64)
        .set("created_at", "2026-06-01T00:00:00Z")
        .into_value()
}

fn attachment_change(op_type: &str, payload: Value) -> ChangeWire {
    let mut change = make_change_wire(op_type, "task", "BBBBBBBBBBBBBBBB", payload);
    change.field = Some("attachments".to_string());
    change
}

#[test]
fn constructed_attachment_payloads_parse_without_json_or_protocol_changes() {
    let add_value = attachment_add_payload();
    let add = AttachmentAddPayload::from_change(&attachment_change(
        op_type::ATTACHMENT_ADD,
        add_value.clone(),
    ))
    .unwrap();
    assert_eq!(serde_json::to_value(add).unwrap(), add_value);

    let delete_value = ChangePayload::workspace(&test_workspace())
        .set("attachment_id", "7KQ9A1X4MV2P8D6R")
        .set("deleted_at", "2026-06-01T00:00:00Z")
        .into_value();
    let delete = AttachmentDeletePayload::from_change(&attachment_change(
        op_type::ATTACHMENT_DELETE,
        delete_value.clone(),
    ))
    .unwrap();
    assert_eq!(serde_json::to_value(delete).unwrap(), delete_value);
}

#[test]
fn attachment_payloads_accept_extensions_and_missing_optional_text() {
    let mut payload = attachment_add_payload();
    let object = payload.as_object_mut().unwrap();
    object.remove("filename");
    object.remove("alt_text");
    object.insert("extension".to_string(), serde_json::json!({ "version": 1 }));

    AttachmentAddPayload::from_change(&attachment_change(op_type::ATTACHMENT_ADD, payload))
        .unwrap();
}

#[test]
fn attachment_payload_authority_rejects_malformed_fields() {
    let cases = [
        ("workspace_id", Value::Null, "workspace_id"),
        (
            "attachment_id",
            Value::String("short".to_string()),
            "attachment_id",
        ),
        (
            "sha256",
            Value::String("short".to_string()),
            "invalid-sha256",
        ),
        ("byte_size", Value::String("12".to_string()), "byte_size"),
        ("byte_size", Value::from(-1), "invalid-attachment-size"),
        (
            "media_type",
            Value::String("image/svg+xml".to_string()),
            "unsupported-attachment-media-type",
        ),
        (
            "filename",
            Value::String("bad/name.png".to_string()),
            "invalid-attachment-filename",
        ),
        (
            "alt_text",
            Value::String("bad\nalt".to_string()),
            "invalid-attachment-alt-text",
        ),
        ("height", Value::Null, "invalid-attachment-dimensions"),
        ("width", Value::from(0), "invalid-attachment-dimensions"),
        (
            "created_at",
            Value::String("today".to_string()),
            "invalid-timestamp",
        ),
    ];
    for (key, value, expected) in cases {
        let mut payload = attachment_add_payload();
        payload[key] = value;
        let error =
            AttachmentAddPayload::from_change(&attachment_change(op_type::ATTACHMENT_ADD, payload))
                .unwrap_err();
        assert!(
            error.to_string().contains(expected),
            "{key} error did not contain {expected}: {error}"
        );
    }

    let mut wrong_field = attachment_change(op_type::ATTACHMENT_ADD, attachment_add_payload());
    wrong_field.field = Some("description".to_string());
    assert!(
        AttachmentAddPayload::from_change(&wrong_field)
            .unwrap_err()
            .to_string()
            .contains("field=attachments")
    );

    let mut invalid_entity = attachment_change(op_type::ATTACHMENT_ADD, attachment_add_payload());
    invalid_entity.entity_id = "invalid".to_string();
    assert!(
        AttachmentAddPayload::from_change(&invalid_entity)
            .unwrap_err()
            .to_string()
            .contains("entity_id")
    );
}
