mod common;

use common::{TestEnv, TestProcess, TestServer, contains_all, contains_none, ok};

#[test]
fn server_logging_does_not_require_writable_state() {
    let env = TestEnv::new();
    let unusable_state = env.path("state-file");
    std::fs::write(&unusable_state, "not a directory").expect("write state file");

    let server = TestServer::start_configured_with_env(
        &env,
        "server.sqlite",
        [("XDG_STATE_HOME", unusable_state.to_str().unwrap())],
    );

    contains_all(&server.output(), &["sync server starting", "bind="]);
}

#[test]
fn daemon_logging_does_not_require_writable_state() {
    let env = TestEnv::new();
    let db = env.db("client.sqlite");
    let wake_addr = env.free_loopback_addr();
    let unusable_state = env.path("state-file");
    std::fs::write(&unusable_state, "not a directory").expect("write state file");
    env.write_daemon_config(&db, &wake_addr, 3600);

    let daemon = TestProcess::start_daemon_with_env(
        &env,
        [("XDG_STATE_HOME", unusable_state.to_str().unwrap())],
    );

    contains_all(&daemon.output(), &["daemon db=", "wake="]);
}

#[test]
fn logging_writes_to_default_state_file_without_affecting_output() {
    let env = TestEnv::new();
    let db = env.db("tasks.sqlite");
    let state_home = env.path("state");
    let output = common::command_with_db(&db)
        .env_remove("AVEN_LOG")
        .env_remove("AVEN_LOG_FILE")
        .env("XDG_STATE_HOME", &state_home)
        .args([
            "add",
            "default log secret title",
            "--description",
            "default log secret body",
            "--project",
            "app",
        ])
        .output()
        .expect("run aven");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    contains_all(&stdout, &["created", "default log secret title"]);
    assert_eq!(stderr, "");

    let logs = std::fs::read_to_string(state_home.join("aven").join("aven.log"))
        .expect("read default logs");
    contains_all(&logs, &["task created", "task_id"]);
    contains_none(
        &logs,
        &["default log secret title", "default log secret body"],
    );
}

#[test]
fn file_logging_records_local_action_without_user_content() {
    let env = TestEnv::new();
    let db = env.db("tasks.sqlite");
    let log = env.path("aven.log");
    let mut command = common::command_with_db(&db);
    command
        .env("AVEN_LOG", "aven=debug")
        .env("AVEN_LOG_FILE", &log)
        .args([
            "add",
            "secret task title",
            "--description",
            "secret body",
            "--project",
            "app",
        ]);
    let output = command.output().expect("run logged aven");
    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");

    let logs = std::fs::read_to_string(log).expect("read logs");
    contains_all(&logs, &["task created", "task_id", "project_key"]);
    contains_none(&logs, &["secret task title", "secret body"]);
}

#[test]
fn delete_operation_logging_redacts_user_authored_names() {
    let env = TestEnv::new();
    let db = env.db("delete.sqlite");
    let log = env.path("delete.log");
    ok(env.aven(&db, ["project", "create", "secret delete project"]));
    ok(env.aven(&db, ["label", "create", "secret-delete-label"]));

    let run_delete = |args: &[&str]| {
        let mut cmd = common::command_with_db(&db);
        cmd.env("XDG_STATE_HOME", env.state_dir())
            .env("AVEN_CONFIG_DIR", env.config_dir().join("aven"))
            .env_remove("AVEN_DB")
            .env("AVEN_LOG", "aven=debug")
            .env("AVEN_LOG_FILE", &log);
        for arg in args {
            cmd.arg(arg);
        }
        ok(cmd.output().expect("run delete command"))
    };

    run_delete(&["project", "delete", "secret-delete-project"]);
    run_delete(&["label", "delete", "secret-delete-label"]);

    let logs = std::fs::read_to_string(log).expect("read delete logs");
    contains_all(&logs, &["label deleted", "project deleted"]);
    contains_none(&logs, &["secret delete project", "secret-delete-label"]);
}

#[test]
fn unusable_file_logging_does_not_mask_command_results() {
    let env = TestEnv::new();
    let file = env.path("not-a-directory");
    std::fs::write(&file, "file").unwrap();
    for explicit in [false, true] {
        let db = env.db(if explicit {
            "explicit.sqlite"
        } else {
            "default.sqlite"
        });
        let mut command = common::command_with_db(&db);
        command
            .env("AVEN_CONFIG_DIR", env.config_dir())
            .env_remove("AVEN_LOG")
            .env_remove("AVEN_LOG_FILE")
            .env("XDG_STATE_HOME", &file);
        if explicit {
            command.env("AVEN_LOG_FILE", file.join("aven.log"));
        }
        let output = command
            .args(["add", "Logging is optional"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        assert!(output.stderr.is_empty());
    }
    let output = common::command_with_db(&file.join("db.sqlite"))
        .env("AVEN_CONFIG_DIR", env.config_dir())
        .env("AVEN_LOG_FILE", file.join("aven.log"))
        .args(["list"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    contains_all(&stderr, &["Error: could not create", "os error"]);
    contains_none(&stderr, &["log directory", "log file"]);

    let output = common::command_with_db(&env.db("invalid-filter.sqlite"))
        .env("AVEN_LOG", "aven=not-a-level")
        .env("AVEN_LOG_FILE", file.join("aven.log"))
        .args(["list"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    contains_all(
        &String::from_utf8_lossy(&output.stderr),
        &["invalid AVEN_LOG filter"],
    );
}

#[test]
fn unprepared_server_logs_storage_before_readable_error() {
    let env = TestEnv::new();
    let data = env.db("unprepared.sqlite");
    let output = common::command()
        .env("AVEN_CONFIG_DIR", env.config_dir())
        .env_remove("AVEN_LOG")
        .args(["server", "--data", data.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    contains_all(
        &stderr,
        &[
            &format!("storage={}", data.display()),
            "Error: Server storage isn't prepared. [server-storage-unprepared]",
            "Next: Run `aven server setup --url URL` first.",
        ],
    );
    assert!(stderr.find("storage=").unwrap() < stderr.find("Error:").unwrap());
    contains_none(&stderr, &["sync server starting"]);
    assert!(!data.exists());
}
