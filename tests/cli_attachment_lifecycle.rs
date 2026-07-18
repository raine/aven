mod common;

use std::process::Command;

use common::{TestEnv, contains_all, contains_none, extract_ref, fail, ok, png_bytes};

#[test]
fn attachment_prune_dry_run_and_apply_are_stable_and_private() {
    let env = TestEnv::new();
    let db = env.db("prune.sqlite");
    env.write_config(
        "local:\n  attachment_lifecycle:\n    grace_days: 0\n    maintenance_limit: 16\n",
    );
    let task = extract_ref(&ok(env.aven(&db, ["add", "private task title"]))).to_string();
    let image = env.path("private-filename.png");
    std::fs::write(&image, png_bytes(2, 2)).unwrap();
    let added = ok(env.aven(
        &db,
        [
            "attachment",
            "add",
            &task,
            image.to_str().unwrap(),
            "--no-optimize",
        ],
    ));
    let attachment_id = added
        .split_whitespace()
        .find_map(|part| part.strip_prefix("attachment_id="))
        .unwrap()
        .to_string();
    ok(env.aven(&db, ["attachment", "delete", &attachment_id]));
    let status = Command::new("sqlite3")
        .arg(&db)
        .arg("UPDATE changes SET server_seq = local_seq WHERE op_type = 'attachment_add';")
        .status()
        .unwrap();
    assert!(status.success());

    let dry_run = ok(env.aven(&db, ["attachment", "prune", "--dry-run"]));
    contains_all(
        &dry_run,
        &["mode=dry-run", "eligible_count=1", "pruned_count=0"],
    );
    contains_none(
        &dry_run,
        &["private task title", "private-filename", &attachment_id],
    );

    let apply = ok(env.aven(&db, ["attachment", "prune", "--apply"]));
    contains_all(
        &apply,
        &["mode=apply", "eligible_count=1", "pruned_count=1"],
    );
    contains_none(
        &apply,
        &["private task title", "private-filename", &attachment_id],
    );

    let empty = ok(env.aven(&db, ["attachment", "prune"]));
    contains_all(&empty, &["mode=dry-run", "eligible_count=0"]);
}

#[test]
fn quota_failure_recovers_after_reference_pruning() {
    let env = TestEnv::new();
    let db = env.db("quota-prune-recovery.sqlite");
    let first_bytes = png_bytes(2, 2);
    let second_bytes = png_bytes(3, 3);
    let quota = first_bytes.len().max(second_bytes.len());
    env.write_config(&format!(
        "local:\n  attachment_lifecycle:\n    grace_days: 0\n    quota_bytes: {quota}\n    maintenance_limit: 16\n",
    ));
    let first_task = extract_ref(&ok(env.aven(&db, ["add", "first", "--project", "app"])));
    let second_task = extract_ref(&ok(env.aven(&db, ["add", "second", "--project", "app"])));
    let first = env.path("first.png");
    let second = env.path("second.png");
    std::fs::write(&first, first_bytes).unwrap();
    std::fs::write(&second, second_bytes).unwrap();
    let added = ok(env.aven(
        &db,
        ["attachment", "add", &first_task, first.to_str().unwrap()],
    ));
    let attachment_id = added
        .split_whitespace()
        .find_map(|part| part.strip_prefix("attachment_id="))
        .unwrap();

    let rejected = fail(env.aven(
        &db,
        ["attachment", "add", &second_task, second.to_str().unwrap()],
    ));
    contains_all(&rejected, &["attachment-quota-exceeded"]);

    ok(env.aven(&db, ["attachment", "delete", attachment_id]));
    let status = Command::new("sqlite3")
        .arg(&db)
        .arg("UPDATE changes SET server_seq = local_seq WHERE op_type = 'attachment_add';")
        .status()
        .unwrap();
    assert!(status.success());
    contains_all(
        &ok(env.aven(&db, ["attachment", "prune", "--apply"])),
        &["pruned_count=1"],
    );
    contains_all(
        &ok(env.aven(
            &db,
            ["attachment", "add", &second_task, second.to_str().unwrap()],
        )),
        &["attachment-added"],
    );
}

#[test]
fn doctor_reports_attachment_lifecycle_categories() {
    let env = TestEnv::new();
    let db = env.db("doctor-lifecycle.sqlite");
    let output = ok(env.aven(&db, ["doctor"]));
    contains_all(
        &output,
        &[
            "Attachment lifecycle",
            "referenced",
            "protected",
            "grace period",
            "eligible",
            "staging",
            "trash",
            "reservations",
            "quota",
            "inconsistencies",
        ],
    );
}
