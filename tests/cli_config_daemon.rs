mod common;

use common::{TestEnv, ok};

#[test]
fn config_get_reports_resolved_sync_enabled_value() {
    let env = TestEnv::new();
    assert_eq!(
        ok(env.aven_config(["config", "get", "sync.enabled"])).trim(),
        "false"
    );

    env.write_config(
        r#"
sync:
  enabled: true
"#,
    );
    assert_eq!(
        ok(env.aven_config(["config", "get", "sync.enabled"])).trim(),
        "true"
    );
}
