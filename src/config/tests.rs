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
    AppConfig::load_from_path(&path).unwrap();
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

#[test]
fn removed_shared_auth_token_is_ignored_and_not_serialized() {
    let config: AppConfig =
        serde_saphyr::from_str("sync:\n  enabled: true\n  auth_token: obsolete-secret\n").unwrap();
    assert!(config.sync.enabled);
    let serialized = serde_saphyr::to_string(&config).unwrap();
    assert!(!serialized.contains("auth_token"));
    assert!(!serialized.contains("obsolete-secret"));
}

#[cfg(unix)]
#[test]
fn protected_destinations_are_never_replaced_or_followed() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    for (target_mode, relative, dangling) in [
        (0o600, false, false),
        (0o444, false, false),
        (0o644, true, false),
        (0o644, true, true),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("source.yaml");
        let path = dir.path().join("config.yaml");
        if !dangling {
            fs::write(&target, "# private\nsync:\n  enabled: false\n").unwrap();
            fs::set_permissions(&target, fs::Permissions::from_mode(target_mode)).unwrap();
        }
        let link = if relative {
            PathBuf::from("source.yaml")
        } else {
            target.clone()
        };
        symlink(&link, &path).unwrap();
        for result in [
            write_config_text(&path, "replaced".into()),
            crate::config_edit::set_scalar(&path, "sync", "enabled", "true"),
            write_default_config(&path),
        ] {
            assert!(
                result
                    .unwrap_err()
                    .downcast_ref::<ConfigWriteRefusal>()
                    .is_some()
            );
            assert!(
                fs::symlink_metadata(&path)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert_eq!(fs::read_link(&path).unwrap(), link);
            assert!(!path.with_extension("yaml.tmp").exists());
            if dangling {
                assert!(!target.exists());
            } else {
                assert_eq!(
                    fs::read_to_string(&target).unwrap(),
                    "# private\nsync:\n  enabled: false\n"
                );
                assert_eq!(
                    fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                    target_mode
                );
            }
        }
        assert!(!crate::server_config::write_template_if_missing(&path).unwrap());
        assert_eq!(fs::read_link(&path).unwrap(), link);
    }
}

#[cfg(unix)]
#[test]
fn config_write_modes_and_parent_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let dir = tempfile::tempdir().unwrap();
    let parent = dir.path().join("directory");
    fs::create_dir(&parent).unwrap();
    let link = dir.path().join("link");
    symlink(&parent, &link).unwrap();
    let path = link.join("config.yaml");
    write_config_text(&path, "original".into()).unwrap();
    for mode in [0o444, 0o600, 0o644] {
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        let result = write_config_text(&path, format!("mode {mode}"));
        if mode == 0o444 {
            assert!(
                result
                    .unwrap_err()
                    .downcast_ref::<ConfigWriteRefusal>()
                    .is_some()
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), "original");
        } else {
            result.unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), format!("mode {mode}"));
        }
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            mode
        );
        assert!(!path.with_extension("yaml.tmp").exists());
    }
    assert!(
        check_config_write(&parent)
            .unwrap_err()
            .downcast_ref::<ConfigWriteRefusal>()
            .is_some()
    );
}
