use super::*;

fn paths_in(dir: &Path) -> ServerPaths {
    ServerPaths::from_parts(dir.join("sync-server.sqlite"), None, None)
}

fn load_text(text: &str, env: &[(&str, &str)]) -> Result<ServerConfig> {
    let dir = tempfile::tempdir()?;
    let paths = paths_in(dir.path());
    fs::write(&paths.config_file, text)?;
    ServerConfig::load_with_env(paths, |name| {
        env.iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.to_string())
    })
}

fn error_text(result: Result<ServerConfig>) -> String {
    format!("{:#}", result.unwrap_err())
}

#[test]
fn config_lives_beside_the_data_unless_overridden() {
    let paths = ServerPaths::from_parts(PathBuf::from("/data/sync-server.sqlite"), None, None);
    assert_eq!(paths.config_file, Path::new("/data/server.yaml"));
    assert_eq!(paths.ai_credentials_dir(), Path::new("/data/private/ai"));

    let relative = ServerPaths::from_parts(PathBuf::from("sync.sqlite"), None, None);
    assert_eq!(relative.config_file, Path::new("./server.yaml"));

    let from_env = ServerPaths::from_parts(
        PathBuf::from("/data/sync-server.sqlite"),
        None,
        Some("/etc/aven/server.yaml".into()),
    );
    assert_eq!(from_env.config_file, Path::new("/etc/aven/server.yaml"));
    assert_eq!(
        from_env.ai_credentials_dir(),
        Path::new("/etc/aven/private/ai")
    );

    let from_flag = ServerPaths::from_parts(
        PathBuf::from("/data/sync-server.sqlite"),
        Some("/srv/aven.yaml".into()),
        Some("/etc/aven/server.yaml".into()),
    );
    assert_eq!(from_flag.config_file, Path::new("/srv/aven.yaml"));
}

#[test]
fn missing_config_means_defaults_and_unconfigured_ai() {
    let dir = tempfile::tempdir().unwrap();
    let config = ServerConfig::load_with_env(paths_in(dir.path()), |_| None).unwrap();
    assert!(config.ai.provider.is_none() && config.ai.model.is_none());
    assert_eq!(config.ai_sources.provider, SettingSource::Unset);
    let policy = config.attachments.policy();
    assert_eq!(policy.grace, Duration::from_secs(30 * 24 * 60 * 60));
    assert_eq!(policy.quota_bytes, DEFAULT_ORIGINAL_QUOTA_BYTES);
}

#[test]
fn reads_ai_and_attachment_settings() {
    let config = load_text(
        "ai:\n  provider: ollama\n  model: llama3.2\nattachments:\n  grace_days: 12\n  workspace_quota_bytes: 345\n",
        &[],
    )
    .unwrap();
    assert_eq!(config.ai.provider.as_deref(), Some("ollama"));
    assert_eq!(config.ai.timeout_seconds(), 45);
    assert_eq!(config.ai_sources.model, SettingSource::File);
    let policy = config.attachments.policy();
    assert_eq!(policy.grace, Duration::from_secs(12 * 24 * 60 * 60));
    assert_eq!(policy.quota_bytes, 345);
}

#[test]
fn invalid_settings_name_the_key() {
    for (text, key) in [
        ("ai:\n  provider: ollama\n", "ai.provider and ai.model"),
        (
            "ai:\n  provider: ollama\n  model: m\n  max_output_tokens: 0\n",
            "ai.max_output_tokens",
        ),
        (
            "ai:\n  provider: ollama\n  model: m\n  endpoint: file:///tmp/model\n",
            "ai.endpoint",
        ),
        ("ai:\n  provder: ollama\n", "provder"),
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
        let error = error_text(load_text(text, &[]));
        assert!(error.contains(key), "{text:?} -> {error}");
        assert!(error.contains("server.yaml"), "{text:?} -> {error}");
    }
}

#[test]
fn environment_overrides_win_and_must_still_pair_provider_and_model() {
    let config = load_text(
        "ai:\n  provider: ollama\n  model: llama3.2\n",
        &[
            ("AVEN_AI_MODEL", "qwen3"),
            ("AVEN_AI_ENDPOINT", "http://127.0.0.1:9"),
        ],
    )
    .unwrap();
    assert_eq!(config.ai.model.as_deref(), Some("qwen3"));
    assert_eq!(config.ai.endpoint.as_deref(), Some("http://127.0.0.1:9"));
    assert_eq!(config.ai_sources.provider, SettingSource::File);
    assert_eq!(config.ai_sources.model, SettingSource::Env);
    assert_eq!(config.ai_sources.endpoint, SettingSource::Env);

    let error = error_text(load_text("", &[("AVEN_AI_PROVIDER", "ollama")]));
    assert!(error.contains("ai.provider and ai.model"), "{error}");
    assert!(error.contains("AVEN_AI_PROVIDER"), "{error}");

    let empty = load_text("", &[("AVEN_AI_PROVIDER", "")]).unwrap();
    assert_eq!(empty.ai_sources.provider, SettingSource::Unset);
}

#[test]
fn template_is_written_once_and_loads_as_defaults_or_uncommented() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths_in(&dir.path().join("nested"));
    assert!(write_template_if_missing(&paths.config_file).unwrap());
    let text = fs::read_to_string(&paths.config_file).unwrap();
    assert!(text.contains("restart"));
    let config = ServerConfig::load_with_env(paths.clone(), |_| None).unwrap();
    assert!(config.ai.provider.is_none() && config.ai.model.is_none());

    fs::write(&paths.config_file, "ai:\n  provider: ollama\n  model: m\n").unwrap();
    assert!(!write_template_if_missing(&paths.config_file).unwrap());
    assert!(
        fs::read_to_string(&paths.config_file)
            .unwrap()
            .contains("model: m")
    );

    let uncommented: String = text
        .lines()
        .filter_map(|line| line.strip_prefix("# "))
        .filter(|line| line.starts_with("  ") || line.ends_with(':'))
        .map(|line| format!("{line}\n"))
        .collect();
    let config = load_text(&uncommented, &[]).unwrap();
    assert_eq!(config.ai.provider.as_deref(), Some("ollama"));
    assert_eq!(
        config.ai.endpoint.as_deref(),
        Some("http://127.0.0.1:11434")
    );
    assert_eq!(config.attachments.grace_days, 30);
}
