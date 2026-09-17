use super::test_support::load_config;
use super::*;

#[test]
fn project_override_workspace_ids_are_validated() {
    let valid = load_config(
            "project:\n  overrides:\n    - workspace_id: 0123456789ABCDEF\n      project: app\n      paths: []\n",
        )
        .unwrap();
    assert_eq!(
        valid.project.overrides[0]
            .workspace_id
            .as_ref()
            .unwrap()
            .as_str(),
        "0123456789ABCDEF"
    );

    let invalid = load_config(
            "project:\n  overrides:\n    - workspace_id: invalid\n      project: app\n      paths: []\n",
        )
        .unwrap_err();
    assert!(format!("{invalid:#}").contains("workspace ID must be"));
}

#[test]
fn update_automatic_checks_parse_from_config() {
    let disabled: AppConfig =
        serde_saphyr::from_str("update:\n  automatic_checks: false\n").unwrap();
    let default: AppConfig = serde_saphyr::from_str("{}\n").unwrap();

    assert!(!disabled.update.automatic_checks);
    assert!(default.update.automatic_checks);
}

#[test]
fn update_check_environment_disable_takes_precedence() {
    for value in [Some("1"), Some("true"), Some("YES")] {
        assert!(!automatic_update_checks_enabled(true, value));
    }
    assert!(!automatic_update_checks_enabled(false, None));
    assert!(automatic_update_checks_enabled(true, Some("false")));
}

#[test]
fn sync_disabled_flag_distinguishes_true_and_other_values() {
    for (value, expected) in [
        (Some("1"), true),
        (Some("true"), true),
        (Some("yes"), true),
        (None, false),
        (Some("0"), false),
        (Some("false"), false),
        (Some(""), false),
    ] {
        assert_eq!(sync_disabled_value(value), expected, "{value:?}");
    }
}

#[test]
fn runtime_disable_override_is_not_serialized() {
    let mut config = AppConfig::default();
    config.sync.enabled = true;
    config.sync.disable_override = true;

    let text = serde_saphyr::to_string(&config).unwrap();

    assert!(text.contains("enabled: true"));
    assert!(!text.contains("disabled:"));
}

#[test]
fn effective_sync_predicates_distinguish_opt_in_and_override() {
    let mut disabled = AppConfig::default();
    disabled.sync.enabled = true;
    disabled.sync.disable_override = true;
    let mut enabled = AppConfig::default();
    enabled.sync.enabled = true;
    let unconfigured = AppConfig::default();

    assert!(!disabled.automatic_sync_is_enabled());
    assert!(enabled.automatic_sync_is_enabled());
    assert!(!unconfigured.automatic_sync_is_enabled());
    assert!(unconfigured.sync_is_allowed());
}

#[test]
fn disabled_sync_reports_actionable_error() {
    let mut config = AppConfig::default();
    config.sync.disable_override = true;
    let error = config.ensure_sync_allowed().unwrap_err();

    assert!(format!("{error:#}").contains("sync-disabled"));
    assert!(format!("{error:#}").contains("sync is disabled"));
}

#[test]
fn generated_client_config_preserves_sync_opt_in_and_omits_server_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.yaml");

    write_default_config(&path).unwrap();

    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("sync:\n  enabled: false\n"));
    assert!(!text.contains("disabled:"));
    assert!(!text.contains("server_grace_days"));
    assert!(!text.contains("server_workspace_quota_bytes"));
    let loaded = AppConfig::load_from_path(&path).unwrap();
    assert_eq!(
        loaded.local.attachment_lifecycle.server_grace_days,
        default_server_attachment_grace_days()
    );
    assert_eq!(
        loaded
            .local
            .attachment_lifecycle
            .server_workspace_quota_bytes,
        default_attachment_quota_bytes()
    );
}

#[test]
fn server_attachment_settings_remain_loadable_and_configurable() {
    let config = load_config(
            "local:\n  attachment_lifecycle:\n    server_grace_days: 12\n    server_workspace_quota_bytes: 345\n",
        )
        .unwrap();

    assert_eq!(config.local.attachment_lifecycle.server_grace_days, 12);
    assert_eq!(
        config
            .local
            .attachment_lifecycle
            .server_workspace_quota_bytes,
        345
    );
    let policy = config.local.attachment_lifecycle.server_policy();
    assert_eq!(
        policy.grace,
        std::time::Duration::from_secs(12 * 24 * 60 * 60)
    );
    assert_eq!(policy.quota_bytes, 345);
}

#[test]
fn ai_config_is_optional_and_bounded() {
    let config = load_config("ai:\n  provider: ollama\n  model: llama3.2\n").unwrap();
    assert_eq!(config.ai.provider.as_deref(), Some("ollama"));
    assert_eq!(config.ai.timeout_seconds(), 45);
    assert_eq!(config.ai.max_output_tokens(), 2048);

    assert!(load_config("ai:\n  provider: ollama\n").is_err());
    assert!(load_config("ai:\n  provider: ollama\n  model: m\n  max_output_tokens: 0\n").is_err());
    assert!(
        load_config("ai:\n  provider: ollama\n  model: m\n  endpoint: file:///tmp/model\n")
            .is_err()
    );
}

#[test]
fn local_inline_images_defaults_to_auto() {
    let config = AppConfig::default();

    assert_eq!(config.local.inline_images, InlineImagesConfig::Auto);
}

#[test]
fn local_image_optimization_defaults_to_off() {
    let config = AppConfig::default();

    assert_eq!(
        config.local.image_optimization,
        ImageOptimizationConfig::Off
    );
    assert!(!config.local.image_optimization.optimizes_pasted_images());
    assert!(!config.local.image_optimization.optimizes_file_attachments());
    let yaml = serde_saphyr::to_string(&config).unwrap();
    assert!(yaml.contains("image_optimization: \"off\""));

    for (value, expected) in [
        ("paste", ImageOptimizationConfig::Paste),
        ("on", ImageOptimizationConfig::On),
    ] {
        let parsed: AppConfig =
            serde_saphyr::from_str(&format!("local:\n  image_optimization: {value}\n")).unwrap();
        assert_eq!(parsed.local.image_optimization, expected);
    }
}
