mod common;

use clap::ValueEnum;
use clap_complete::Shell;
use common::{TestEnv, contains_all};

#[test]
fn every_shell_generates_without_config_database_or_file_logging() {
    let env = TestEnv::new();
    env.write_config("[invalid yaml");
    let file = env.path("not-a-directory");
    std::fs::write(&file, "file").unwrap();
    let db = file.join("must-not-open.sqlite");
    for shell in Shell::value_variants() {
        let output = common::command_with_db(&db)
            .env("AVEN_CONFIG_DIR", env.config_dir().join("aven"))
            .env("XDG_STATE_HOME", &file)
            .env("HOME", &file)
            .env("AVEN_LOG_FILE", file.join("aven.log"))
            .env_remove("AVEN_LOG")
            .args(["completions", &shell.to_string()])
            .output()
            .unwrap();
        assert!(output.status.success(), "{shell}: {output:?}");
        assert!(output.stderr.is_empty(), "{shell}: {output:?}");
        contains_all(
            &String::from_utf8_lossy(&output.stdout),
            &["aven", "server", "urgent"],
        );
        assert!(!db.exists());
    }
    assert!(!env.state_dir().exists());
}
