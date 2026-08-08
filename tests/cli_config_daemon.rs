mod common;

use common::{TestEnv, fail, ok};

#[test]
fn config_help_lists_scalar_commands_and_non_secret_keys() {
    let env = TestEnv::new();
    let help = ok(env.aven_config(["config", "--help"]));
    assert!(help.contains("get"));
    assert!(help.contains("set"));

    for command in ["get", "set"] {
        let help = ok(env.aven_config(["config", command, "--help"]));
        for key in [
            "sync.enabled",
            "sync.server_url",
            "sync.interval_seconds",
            "update.automatic_checks",
            "local.db_path",
            "local.image_optimization",
        ] {
            assert!(help.contains(key), "{command} help omitted {key}");
        }
        assert!(!help.contains("auth_token"));
    }
}

#[test]
fn config_get_reports_every_supported_scalar_without_exposing_secrets() {
    let env = TestEnv::new();
    env.write_config(
        r#"
local:
  db_path: "/tmp/aven tasks.sqlite"
  image_optimization: paste
sync:
  enabled: true
  server_url: "https://sync.example.com/v1"
  interval_seconds: 45
  auth_token: "top-secret-token"
update:
  automatic_checks: false
"#,
    );

    for (key, expected) in [
        ("sync.enabled", "true"),
        ("sync.server_url", "\"https://sync.example.com/v1\""),
        ("sync.interval_seconds", "45"),
        ("update.automatic_checks", "false"),
        ("local.db_path", "\"/tmp/aven tasks.sqlite\""),
        ("local.image_optimization", "paste"),
    ] {
        let output = env.aven_config(["config", "get", key]);
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!combined.contains("top-secret-token"));
        assert_eq!(ok(output).trim(), expected, "key {key}");
    }
}

#[test]
fn config_get_reports_defaults_and_unset_optional_values() {
    let env = TestEnv::new();

    for (key, expected) in [
        ("sync.enabled", "false"),
        ("sync.server_url", "null"),
        ("sync.interval_seconds", "30"),
        ("update.automatic_checks", "true"),
        ("local.db_path", "null"),
        ("local.image_optimization", "off"),
    ] {
        assert_eq!(
            ok(env.aven_config(["config", "get", key])).trim(),
            expected,
            "key {key}"
        );
    }
}

#[test]
fn config_set_persists_every_supported_scalar_and_preserves_user_text() {
    let env = TestEnv::new();
    env.write_config(
        r#"# personal configuration
sync:
    enabled: false # keep this explanation
    auth_token: "top-secret-token"

project:
  overrides: [] # unrelated setting
"#,
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(env.config_file(), std::fs::Permissions::from_mode(0o600))
            .unwrap();
    }

    for (key, value, expected) in [
        ("sync.enabled", "true", "true"),
        (
            "sync.server_url",
            "https://sync.example.com/v1",
            "\"https://sync.example.com/v1\"",
        ),
        ("sync.interval_seconds", "90", "90"),
        ("update.automatic_checks", "false", "false"),
        (
            "local.db_path",
            "~/Aven Tasks/db.sqlite",
            "\"~/Aven Tasks/db.sqlite\"",
        ),
        ("local.image_optimization", "on", "on"),
    ] {
        let output = ok(env.aven_config(["config", "set", key, value]));
        assert_eq!(output.trim(), format!("updated-config key={key}"));
        assert_eq!(
            ok(env.aven_config(["config", "get", key])).trim(),
            expected,
            "key {key}"
        );
    }

    let text = std::fs::read_to_string(env.config_file()).unwrap();
    assert!(text.contains("# personal configuration"));
    assert!(text.contains("    enabled: true # keep this explanation"));
    assert!(text.contains("    auth_token: \"top-secret-token\""));
    assert!(text.contains("    server_url:"));
    assert!(text.contains("overrides: [] # unrelated setting"));
    assert!(!env.config_file().with_extension("yaml.tmp").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(env.config_file())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    for key in ["sync.server_url", "local.db_path"] {
        ok(env.aven_config(["config", "set", key, "null"]));
        assert_eq!(ok(env.aven_config(["config", "get", key])).trim(), "null");
    }
}

#[test]
fn config_set_rejects_invalid_keys_and_values_without_changing_the_file() {
    let env = TestEnv::new();
    let original = "sync:\n  enabled: false\n  auth_token: top-secret-token\n";
    env.write_config(original);

    let invalid = [
        ("sync.enabled", "yes"),
        ("sync.server_url", "file:///tmp/server"),
        ("sync.interval_seconds", "0"),
        ("update.automatic_checks", "sometimes"),
        ("local.db_path", ""),
        ("local.image_optimization", "auto"),
    ];
    for (key, value) in invalid {
        let error = fail(env.aven_config(["config", "set", key, value]));
        assert!(error.contains("invalid value"), "{error}");
        assert!(!error.contains("top-secret-token"));
        assert_eq!(
            std::fs::read_to_string(env.config_file()).unwrap(),
            original
        );
    }

    let error = fail(env.aven_config(["config", "get", "sync.auth_token"]));
    assert!(error.contains("invalid value"));
    assert!(!error.contains("top-secret-token"));

    let error = fail(env.aven_config(["config", "set", "sync.auth_token", "replacement-secret"]));
    assert!(error.contains("invalid value"));
    assert!(!error.contains("top-secret-token"));
    assert_eq!(
        std::fs::read_to_string(env.config_file()).unwrap(),
        original
    );
}

#[test]
fn config_set_rejects_inline_mappings_instead_of_reformatting_them() {
    let env = TestEnv::new();
    let original = "sync: { enabled: false, auth_token: top-secret-token }\n";
    env.write_config(original);

    let error = fail(env.aven_config(["config", "set", "sync.enabled", "true"]));

    assert!(error.contains("must use a YAML block mapping"));
    assert!(!error.contains("top-secret-token"));
    assert_eq!(
        std::fs::read_to_string(env.config_file()).unwrap(),
        original
    );
}
