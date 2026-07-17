use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::cli::{CodingAgentArg, SkillInstallArgs};

const SKILL_NAME: &str = "aven";
const SKILL_BODY: &str = include_str!("../skill.md");
const SKILL_DESCRIPTION: &str =
    "Use aven to find tasks, update status, and leave durable handoff context.";

struct CodingAgent {
    arg: CodingAgentArg,
    name: &'static str,
    user_parent: PathBuf,
    skill_dir: PathBuf,
    workspace_markers: &'static [&'static str],
}

impl CodingAgent {
    fn new(
        arg: CodingAgentArg,
        name: &'static str,
        user_parent: PathBuf,
        workspace_markers: &'static [&'static str],
    ) -> Self {
        let skill_dir = user_parent.join("skills").join(SKILL_NAME);
        Self {
            arg,
            name,
            user_parent,
            skill_dir,
            workspace_markers,
        }
    }

    fn is_detected(&self, cwd: &Path) -> bool {
        self.user_parent.is_dir()
            || ancestors(cwd).any(|dir| {
                self.workspace_markers
                    .iter()
                    .any(|marker| dir.join(marker).exists())
            })
    }
}

pub(crate) fn print() -> Result<()> {
    print!("{SKILL_BODY}");
    Ok(())
}

pub(crate) fn install(args: SkillInstallArgs) -> Result<()> {
    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?;
    let cwd = std::env::current_dir()?;
    let agents = all_agents(&home);
    let targets: Vec<&CodingAgent> = if args.agent.is_empty() {
        agents
            .iter()
            .filter(|agent| agent.is_detected(&cwd))
            .collect()
    } else {
        agents
            .iter()
            .filter(|agent| args.agent.contains(&agent.arg))
            .collect()
    };

    if targets.is_empty() {
        bail!(
            "no supported coding agents detected (expected ~/.claude, ~/.config/opencode, ~/.codex, or workspace agent config); use --agent to choose a target"
        );
    }

    let content = skill_content();
    for target in targets {
        let path = target.skill_dir.join("SKILL.md");
        fs::create_dir_all(&target.skill_dir)?;
        fs::write(&path, content.as_bytes())?;
        println!(
            "installed {} skill for {} at {}",
            SKILL_NAME,
            target.name,
            shrink_home(&path, &home)
        );
    }
    Ok(())
}

fn all_agents(home: &Path) -> Vec<CodingAgent> {
    vec![
        CodingAgent::new(
            CodingAgentArg::Claude,
            "Claude Code",
            home.join(".claude"),
            &[".claude"],
        ),
        CodingAgent::new(
            CodingAgentArg::Opencode,
            "OpenCode",
            home.join(".config").join("opencode"),
            &[".opencode"],
        ),
        CodingAgent::new(
            CodingAgentArg::Codex,
            "Codex",
            home.join(".codex"),
            &[".codex"],
        ),
    ]
}

fn ancestors(path: &Path) -> impl Iterator<Item = &Path> {
    path.ancestors()
}

fn skill_content() -> String {
    format!("---\nname: {SKILL_NAME}\ndescription: {SKILL_DESCRIPTION}\n---\n\n{SKILL_BODY}")
}

fn shrink_home(path: &Path, home: &Path) -> String {
    path.strip_prefix(home)
        .map(|relative| format!("~/{}", relative.display()))
        .unwrap_or_else(|_| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_content_has_frontmatter_and_primer() {
        let content = skill_content();
        assert!(content.starts_with("---\nname: aven\n"));
        assert!(content.contains("# Aven CLI Primer"));
    }
}
