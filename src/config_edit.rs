use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::AppConfig;

const MANAGED_MARKER: &str = "# aven-managed project path mapping";

#[derive(Debug, Clone)]
pub(crate) struct ProjectPathMappingEdit<'a> {
    pub(crate) workspace_id: &'a str,
    pub(crate) workspace: &'a str,
    pub(crate) project: &'a str,
    pub(crate) path: PathBuf,
}

#[derive(Debug, Clone)]
struct ManagedProjectOverride {
    workspace_id: String,
    workspace: String,
    project: String,
    paths: Vec<PathBuf>,
}

pub(crate) fn add_project_path(path: &Path, edit: ProjectPathMappingEdit<'_>) -> Result<()> {
    let text = read_config_text(path)?;
    let (text, mut entries) = remove_managed_entries(&text);
    for entry in &mut entries {
        if entry.workspace_id == edit.workspace_id {
            entry.paths.retain(|path| path != &edit.path);
        }
    }
    entries.retain(|entry| !entry.paths.is_empty());
    if let Some(entry) = entries.iter_mut().find(|entry| {
        entry.workspace_id == edit.workspace_id && entry.project == edit.project
    }) {
        entry.paths.push(edit.path);
    } else {
        entries.push(ManagedProjectOverride {
            workspace_id: edit.workspace_id.to_string(),
            workspace: edit.workspace.to_string(),
            project: edit.project.to_string(),
            paths: vec![edit.path],
        });
    }
    write_edited_config(path, append_managed_entries(&text, &entries)?)
}

pub(crate) fn remove_project_path(
    path: &Path,
    workspace_id: &str,
    project: &str,
    remove_paths: &[PathBuf],
) -> Result<()> {
    let text = read_config_text(path)?;
    let (text, mut entries) = remove_managed_entries(&text);
    for entry in &mut entries {
        if entry.workspace_id == workspace_id && entry.project == project {
            entry
                .paths
                .retain(|path| !remove_paths.iter().any(|remove_path| path == remove_path));
        }
    }
    entries.retain(|entry| !entry.paths.is_empty());
    write_edited_config(path, append_managed_entries(&text, &entries)?)
}

fn read_config_text(path: &Path) -> Result<String> {
    if path.exists() {
        return fs::read_to_string(path).with_context(|| format!("could not read {}", path.display()));
    }
    serde_yaml::to_string(&AppConfig::default()).context("could not serialize default config")
}

fn write_edited_config(path: &Path, text: String) -> Result<()> {
    serde_yaml::from_str::<AppConfig>(&text)
        .with_context(|| format!("edited config did not parse for {}", path.display()))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let tmp_path = path.with_extension("yaml.tmp");
    fs::write(&tmp_path, text).with_context(|| format!("could not write {}", tmp_path.display()))?;
    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "could not replace {} with {}",
            path.display(),
            tmp_path.display()
        )
    })?;
    Ok(())
}

fn remove_managed_entries(text: &str) -> (String, Vec<ManagedProjectOverride>) {
    let lines = split_lines(text);
    let mut output = Vec::new();
    let mut entries = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() != MANAGED_MARKER {
            output.push(lines[i].clone());
            i += 1;
            continue;
        }
        let marker_indent = indent_len(&lines[i]);
        let mut block = vec![lines[i].clone()];
        let mut seen_entry = false;
        i += 1;
        while i < lines.len() {
            let line = &lines[i];
            let trimmed = line.trim_start();
            let indent = indent_len(line);
            let starts_next_entry = seen_entry && indent == marker_indent && trimmed.starts_with("- ");
            let starts_sibling = indent < marker_indent && !trimmed.is_empty();
            if starts_sibling || starts_next_entry {
                break;
            }
            seen_entry |= indent == marker_indent && trimmed.starts_with("- ");
            block.push(line.clone());
            i += 1;
        }
        if let Some(entry) = parse_managed_entry(&block) {
            entries.push(entry);
        }
    }
    let mut text = output.join("\n");
    if text.ends_with('\n') {
        return (text, entries);
    }
    if !text.is_empty() && has_trailing_newline(text.as_str(), lines.last()) {
        text.push('\n');
    }
    (text, entries)
}

fn append_managed_entries(text: &str, entries: &[ManagedProjectOverride]) -> Result<String> {
    if entries.is_empty() {
        return Ok(text.to_string());
    }
    let mut lines = split_lines(text);
    let block = render_managed_entries(entries);
    if let Some(project_line) = find_top_level_key(&lines, "project") {
        let project_end = find_section_end(&lines, project_line, 0);
        if let Some(overrides_line) = find_child_key(&lines, project_line + 1, project_end, 2, "overrides") {
            let overrides_end = find_section_end(&lines, overrides_line, 2);
            insert_lines(&mut lines, overrides_end, block);
        } else {
            let insert_at = project_line + 1;
            let mut insert = vec!["  overrides:".to_string()];
            insert.extend(block);
            insert_lines(&mut lines, insert_at, insert);
        }
    } else {
        if !lines.is_empty() && !lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push("project:".to_string());
        lines.push("  overrides:".to_string());
        lines.extend(block);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    Ok(out)
}

fn parse_managed_entry(lines: &[String]) -> Option<ManagedProjectOverride> {
    let mut workspace_id = None;
    let mut workspace = None;
    let mut project = None;
    let mut paths = Vec::new();
    let mut in_paths = false;
    for line in lines {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("- workspace_id:") {
            workspace_id = Some(parse_scalar(value));
            in_paths = false;
        } else if let Some(value) = trimmed.strip_prefix("workspace_id:") {
            workspace_id = Some(parse_scalar(value));
            in_paths = false;
        } else if let Some(value) = trimmed.strip_prefix("workspace:") {
            workspace = Some(parse_scalar(value));
            in_paths = false;
        } else if let Some(value) = trimmed.strip_prefix("project:") {
            project = Some(parse_scalar(value));
            in_paths = false;
        } else if trimmed == "paths:" {
            in_paths = true;
        } else if in_paths
            && let Some(value) = trimmed.strip_prefix("- ")
        {
            paths.push(PathBuf::from(parse_scalar(value)));
        }
    }
    Some(ManagedProjectOverride {
        workspace_id: workspace_id?,
        workspace: workspace?,
        project: project?,
        paths,
    })
    .filter(|entry| !entry.paths.is_empty())
}

fn render_managed_entries(entries: &[ManagedProjectOverride]) -> Vec<String> {
    let mut lines = Vec::new();
    for entry in entries {
        lines.push(format!("    {MANAGED_MARKER}"));
        lines.push(format!("    - workspace_id: {}", yaml_scalar(&entry.workspace_id)));
        lines.push(format!("      workspace: {}", yaml_scalar(&entry.workspace)));
        lines.push(format!("      project: {}", yaml_scalar(&entry.project)));
        lines.push("      paths:".to_string());
        for path in &entry.paths {
            lines.push(format!("        - {}", yaml_scalar(&path.display().to_string())));
        }
    }
    lines
}

fn yaml_scalar(value: &str) -> String {
    serde_yaml::to_string(value)
        .unwrap_or_else(|_| format!("\"{}\"", value.replace('"', "\\\"")))
        .trim()
        .trim_end_matches("...")
        .trim()
        .to_string()
}

fn parse_scalar(value: &str) -> String {
    let value = value.trim();
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value)
        .to_string()
}

fn split_lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

fn insert_lines(lines: &mut Vec<String>, index: usize, new_lines: Vec<String>) {
    for (offset, line) in new_lines.into_iter().enumerate() {
        lines.insert(index + offset, line);
    }
}

fn find_top_level_key(lines: &[String], key: &str) -> Option<usize> {
    let prefix = format!("{key}:");
    lines.iter().position(|line| {
        indent_len(line) == 0 && (line.trim() == prefix || line.trim().starts_with(&format!("{prefix} ")))
    })
}

fn find_child_key(
    lines: &[String],
    start: usize,
    end: usize,
    indent: usize,
    key: &str,
) -> Option<usize> {
    let prefix = format!("{key}:");
    (start..end).find(|index| {
        let line = &lines[*index];
        indent_len(line) == indent
            && (line.trim() == prefix || line.trim().starts_with(&format!("{prefix} ")))
    })
}

fn find_section_end(lines: &[String], start: usize, indent: usize) -> usize {
    for (index, line) in lines.iter().enumerate().skip(start + 1) {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if indent_len(line) <= indent {
            return index;
        }
    }
    lines.len()
}

fn indent_len(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn has_trailing_newline(_text: &str, last_line: Option<&String>) -> bool {
    last_line.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_preserves_unrelated_comments() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.yaml");
        fs::write(
            &path,
            "# top\nlocal:\n  # db comment\n  db_path: /tmp/db.sqlite\n\nproject:\n  # manual comment\n  overrides:\n    # keep manual\n    - project: Manual\n      paths: [/tmp/manual]\n",
        )
        .unwrap();

        add_project_path(
            &path,
            ProjectPathMappingEdit {
                workspace_id: "workspace-id",
                workspace: "default",
                project: "app",
                path: PathBuf::from("/tmp/app"),
            },
        )
        .unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# top"));
        assert!(text.contains("# db comment"));
        assert!(text.contains("# manual comment"));
        assert!(text.contains(MANAGED_MARKER));
        assert!(text.contains("workspace_id: workspace-id"));
        assert!(text.contains("project: app"));
        assert!(text.contains("/tmp/app"));
    }

    #[test]
    fn remove_only_updates_managed_entries() {
        let text = "project:\n  overrides:\n    # manual\n    - project: app\n      paths: [/tmp/app]\n    # aven-managed project path mapping\n    - workspace_id: workspace-id\n      workspace: default\n      project: app\n      paths:\n        - /tmp/app\n        - /tmp/other\n";
        let (text, mut entries) = remove_managed_entries(text);
        assert!(text.contains("# manual"));
        assert!(text.contains("/tmp/app"));
        entries[0].paths.retain(|path| path != Path::new("/tmp/app"));
        let text = append_managed_entries(&text, &entries).unwrap();
        assert!(text.contains("# manual"));
        assert!(text.contains("/tmp/app"));
        assert!(text.contains("/tmp/other"));
    }
}
