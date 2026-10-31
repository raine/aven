#![cfg(unix)]
mod common;

use common::{TestEnv, contains_all, ok, scalar_i64};
use std::os::unix::fs::{PermissionsExt, symlink};

#[test]
fn protected_config_refuses_mutations_without_partial_projects_or_success_output() {
    for symlinked in [false, true] {
        let env = TestEnv::new();
        let db = env.db("tasks.sqlite");
        let directory = env.path("mapped ' path: special");
        std::fs::create_dir(&directory).unwrap();
        ok(env.aven(
            &db,
            [
                "project",
                "create",
                "app",
                "--path",
                directory.to_str().unwrap(),
            ],
        ));
        let original = std::fs::read_to_string(env.config_file()).unwrap();
        let source = env.path("source.yaml");
        if symlinked {
            std::fs::rename(env.config_file(), &source).unwrap();
            symlink(&source, env.config_file()).unwrap();
        } else {
            std::fs::set_permissions(env.config_file(), std::fs::Permissions::from_mode(0o444))
                .unwrap();
        }
        for args in [
            vec![
                "project",
                "create",
                "new",
                "--path",
                directory.to_str().unwrap(),
            ],
            vec!["project", "rename", "app", "renamed"],
            vec!["project", "path", "add", "app", directory.to_str().unwrap()],
            vec![
                "project",
                "path",
                "remove",
                "app",
                directory.to_str().unwrap(),
            ],
            vec!["config", "set", "sync.enabled", "true"],
        ] {
            let output = env.aven(&db, args);
            assert!(!output.status.success());
            assert!(output.stdout.is_empty(), "{:?}", output);
            let error = String::from_utf8(output.stderr).unwrap();
            contains_all(
                &error,
                &[
                    "refusing to modify",
                    "configuration source",
                    if symlinked { "symlinked" } else { "read-only" },
                ],
            );
            assert_eq!(
                std::fs::read_to_string(env.config_file()).unwrap(),
                original
            );
            assert!(!env.config_file().with_extension("yaml.tmp").exists());
        }
        assert_eq!(
            scalar_i64(
                &db,
                "SELECT count(*) FROM projects WHERE key IN ('new', 'renamed')"
            ),
            0
        );
        assert_eq!(
            scalar_i64(&db, "SELECT count(*) FROM projects WHERE key = 'app'"),
            1
        );
        if symlinked {
            assert_eq!(std::fs::read_link(env.config_file()).unwrap(), source);
        }
        ok(env.aven(&db, ["project", "create", "unmapped"]));
        ok(env.aven(&db, ["project", "rename", "unmapped", "allowed"]));
    }
}

#[test]
fn late_creation_mapping_failure_discloses_created_project() {
    let env = TestEnv::new();
    let db = env.db("tasks.sqlite");
    env.write_config("{}\n");
    std::fs::create_dir(env.config_file().with_extension("yaml.tmp")).unwrap();
    let output = env.aven(
        &db,
        [
            "project",
            "create",
            "partial",
            "--path",
            env.path("").to_str().unwrap(),
        ],
    );
    assert!(!output.status.success());
    contains_all(
        &String::from_utf8(output.stderr).unwrap(),
        &["Project partial was created", "mapping was not saved"],
    );
    assert_eq!(
        scalar_i64(&db, "SELECT count(*) FROM projects WHERE key = 'partial'"),
        1
    );
}
