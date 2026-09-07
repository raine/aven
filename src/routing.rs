use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow};

use crate::config::AppConfig;

/// Invocation inputs shared by workspace selection and project inference.
/// Path discovery is lazy so explicit arguments bypass unused path failures.
pub(crate) struct InvocationRouting<'a> {
    pub(crate) config: &'a AppConfig,
    invocation_cwd: std::io::Result<PathBuf>,
    cwd: OnceLock<Result<PathBuf>>,
    git_root: OnceLock<Result<Option<PathBuf>>>,
}

impl<'a> InvocationRouting<'a> {
    pub(crate) fn new(config: &'a AppConfig) -> Self {
        Self {
            config,
            invocation_cwd: std::env::current_dir(),
            cwd: OnceLock::new(),
            git_root: OnceLock::new(),
        }
    }

    pub(crate) fn invocation_cwd(&self) -> Result<&Path> {
        self.invocation_cwd
            .as_deref()
            .map_err(|error| anyhow!("{error}"))
    }

    pub(crate) fn cwd(&self) -> Result<&Path> {
        self.cwd
            .get_or_init(|| {
                std::fs::canonicalize(self.invocation_cwd()?).context("could not resolve cwd")
            })
            .as_ref()
            .map(PathBuf::as_path)
            .map_err(|error| anyhow!("{error:#}"))
    }

    pub(crate) fn git_root(&self) -> Result<Option<&Path>> {
        self.git_root
            .get_or_init(|| crate::projects::git_root(self.cwd()?))
            .as_ref()
            .map(|root| root.as_deref())
            .map_err(|error| anyhow!("{error:#}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects::{
        inferred_existing_project_key_with_routing, inferred_project_key_for_add_with_routing,
    };
    use crate::workspaces::{Workspace, resolve_active_workspace_with_routing};

    #[tokio::test]
    async fn snapshot_retains_loaded_config_cwd_and_discovered_git_root() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let config_path = dir.path().join("config.yaml");
        std::fs::write(
            &config_path,
            format!(
                "project:\n  overrides:\n    - project: captured\n      paths: ['{}']\n",
                repo.display()
            ),
        )
        .unwrap();
        let config = AppConfig::load_from_path(&config_path).unwrap();
        let routing = InvocationRouting {
            invocation_cwd: Ok(repo.clone()),
            ..InvocationRouting::new(&config)
        };
        let cwd = routing.cwd().unwrap().to_path_buf();
        assert_eq!(routing.git_root().unwrap(), Some(cwd.as_path()));
        std::fs::remove_dir(repo.join(".git")).unwrap();
        std::fs::write(&config_path, "project:\n  overrides: []\n").unwrap();
        let database = aven_core::db::Database::open(&dir.path().join("db.sqlite"))
            .await
            .unwrap();
        let workspace = resolve_active_workspace_with_routing(&database, None, &routing)
            .await
            .unwrap();
        assert_eq!(workspace.id, Workspace::default().id);
        assert_eq!(
            inferred_project_key_for_add_with_routing(&database, &workspace, &routing)
                .await
                .unwrap(),
            Some("captured".to_string())
        );
        assert_eq!(
            inferred_existing_project_key_with_routing(&database, &workspace, &routing)
                .await
                .unwrap(),
            None
        );
        assert_eq!(routing.cwd().unwrap(), cwd);
        assert_eq!(routing.git_root().unwrap(), Some(cwd.as_path()));
    }

    #[tokio::test]
    async fn explicit_workspace_bypasses_invalid_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let config = AppConfig::default();
        let routing = InvocationRouting {
            invocation_cwd: Ok(dir.path().join("missing")),
            ..InvocationRouting::new(&config)
        };
        let database = aven_core::db::Database::open(&dir.path().join("db.sqlite"))
            .await
            .unwrap();
        assert!(
            resolve_active_workspace_with_routing(&database, Some("default"), &routing)
                .await
                .is_ok()
        );
        assert!(
            resolve_active_workspace_with_routing(&database, None, &routing)
                .await
                .is_err()
        );
    }
}
