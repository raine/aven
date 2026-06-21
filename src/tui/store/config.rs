use anyhow::Result;

use crate::operations::{
    init_config as init_config_operation, show_config as show_config_operation,
    show_config_paths as show_config_paths_operation,
    show_config_status as show_config_status_operation,
};

use super::TuiStore;

impl TuiStore {
    pub(crate) fn config_status_lines(&self) -> Result<Vec<String>> {
        Ok(show_config_status_operation()?.lines)
    }

    pub(crate) fn config_info_lines(&self) -> Result<Vec<String>> {
        let outcome = show_config_operation()?;
        let mut lines = vec![
            format!("config path: {}", outcome.path.display()),
            String::new(),
        ];
        lines.extend(outcome.text.lines().map(str::to_string));
        Ok(lines)
    }

    pub(crate) fn config_path_lines(&self) -> Result<Vec<String>> {
        Ok(show_config_paths_operation()?.lines)
    }

    pub(crate) fn init_config(&self) -> Result<String> {
        let outcome = init_config_operation()?;
        Ok(format!("created config {}", outcome.path.display()))
    }
}
