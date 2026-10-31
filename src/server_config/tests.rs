use super::*;

fn paths_in(dir: &Path) -> ServerPaths {
    ServerPaths::from_parts(dir.join("sync-server.sqlite"), None, None)
}

fn load_text(text: &str) -> Result<ServerConfig> {
    let dir = tempfile::tempdir()?;
    let paths = paths_in(dir.path());
    fs::write(&paths.config_file, text)?;
    ServerConfig::load(paths)
}

#[test]
fn config_lives_beside_the_data_unless_overridden() {
    let paths = ServerPaths::from_parts(PathBuf::from("/data/sync-server.sqlite"), None, None);
    assert_eq!(paths.config_file, Path::new("/data/server.yaml"));

    let relative = ServerPaths::from_parts(PathBuf::from("sync.sqlite"), None, None);
    assert_eq!(relative.config_file, Path::new("./server.yaml"));

    let from_env = ServerPaths::from_parts(
        PathBuf::from("/data/sync-server.sqlite"),
        None,
        Some("/etc/aven/server.yaml".into()),
    );
    assert_eq!(from_env.config_file, Path::new("/etc/aven/server.yaml"));

    let from_flag = ServerPaths::from_parts(
        PathBuf::from("/data/sync-server.sqlite"),
        Some("/srv/aven.yaml".into()),
        Some("/etc/aven/server.yaml".into()),
    );
    assert_eq!(from_flag.config_file, Path::new("/srv/aven.yaml"));
}

#[test]
fn missing_config_means_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let config = ServerConfig::load(paths_in(dir.path())).unwrap();
    let policy = config.attachments.policy();
    assert_eq!(policy.grace, Duration::from_secs(30 * 24 * 60 * 60));
    assert_eq!(policy.quota_bytes, DEFAULT_ORIGINAL_QUOTA_BYTES);
}

#[test]
fn reads_attachment_settings() {
    let config =
        load_text("attachments:\n  grace_days: 12\n  workspace_quota_bytes: 345\n").unwrap();
    let policy = config.attachments.policy();
    assert_eq!(policy.grace, Duration::from_secs(12 * 24 * 60 * 60));
    assert_eq!(policy.quota_bytes, 345);
}

#[test]
fn invalid_settings_name_the_key_and_file() {
    for (text, key) in [
        ("ai:\n  provider: ollama\n", "ai"),
        ("attachments:\n  grace: 3\n", "grace"),
        (
            "attachments:\n  workspace_quota_bytes: -1\n",
            "workspace_quota_bytes",
        ),
        (
            "attachments:\n  workspace_quota_bytes: 9223372036854775808\n",
            "attachments.workspace_quota_bytes",
        ),
        ("local:\n  db_path: x\n", "local"),
    ] {
        let error = format!("{:#}", load_text(text).unwrap_err());
        assert!(error.contains(key), "{text:?} -> {error}");
        assert!(error.contains("server.yaml"), "{text:?} -> {error}");
    }
}

#[test]
fn template_is_written_once_and_loads_as_defaults_or_uncommented() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths_in(&dir.path().join("nested"));
    assert!(write_template_if_missing(&paths.config_file).unwrap());
    let text = fs::read_to_string(&paths.config_file).unwrap();
    assert!(text.contains("restart"));
    assert!(!text.contains("ai:"));
    let config = ServerConfig::load(paths.clone()).unwrap();
    assert_eq!(config.attachments.grace_days, 30);

    let custom = "attachments:\n  grace_days: 3\n";
    fs::write(&paths.config_file, custom).unwrap();
    assert!(!write_template_if_missing(&paths.config_file).unwrap());
    assert_eq!(fs::read_to_string(&paths.config_file).unwrap(), custom);

    let uncommented: String = text
        .lines()
        .filter_map(|line| line.strip_prefix("# "))
        .filter(|line| line.starts_with("  ") || line.ends_with(':'))
        .map(|line| format!("{line}\n"))
        .collect();
    let config = load_text(&uncommented).unwrap();
    assert_eq!(config.attachments.grace_days, 30);
    assert_eq!(config.attachments.workspace_quota_bytes, 10737418240);
}
