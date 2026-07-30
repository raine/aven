mod common;

use std::fs;
use std::process::Command;

use common::{TestEnv, command, contains_all, contains_none, fail, ok};

#[test]
fn skill_install_targets_explicit_agent() {
    let env = TestEnv::new();
    let home = env.path("home");
    fs::create_dir_all(&home).unwrap();

    let output = ok(skill_command(
        &home,
        ["skill", "install", "--agent", "claude"],
    ));
    contains_all(
        &output,
        &["installed aven skill for Claude Code at ~/.claude/skills/aven/SKILL.md"],
    );

    let skill_path = home.join(".claude/skills/aven/SKILL.md");
    let installed = fs::read_to_string(skill_path).unwrap();
    let printed = ok(skill_command(&home, ["skill"]));
    assert!(installed.ends_with(&printed));
    contains_all(
        &installed,
        &[
            "`list`, `show`, and `search` JSON use the same flat task object",
            "Search adds",
            "`score`",
            "`matched_field`",
            "`snippet`",
            "dependency, conflict, timestamp, and recurrence state",
        ],
    );
    assert!(!home.join(".codex/skills/aven/SKILL.md").exists());
}

#[test]
fn skill_install_defaults_to_detected_user_and_workspace_agents() {
    let env = TestEnv::new();
    let home = env.path("home");
    let repo = env.path("repo");
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::create_dir_all(repo.join(".opencode")).unwrap();

    let output = ok(skill_command_in(&home, &repo, ["skill", "install"]));
    contains_all(
        &output,
        &[
            "installed aven skill for Claude Code at ~/.claude/skills/aven/SKILL.md",
            "installed aven skill for OpenCode at ~/.config/opencode/skills/aven/SKILL.md",
            "installed aven skill for Codex at ~/.codex/skills/aven/SKILL.md",
        ],
    );

    assert!(home.join(".claude/skills/aven/SKILL.md").exists());
    assert!(home.join(".config/opencode/skills/aven/SKILL.md").exists());
    assert!(home.join(".codex/skills/aven/SKILL.md").exists());
}

#[test]
fn skill_install_reports_no_detected_agents() {
    let env = TestEnv::new();
    let home = env.path("home");
    fs::create_dir_all(&home).unwrap();

    let output = fail(skill_command(&home, ["skill", "install"]));
    contains_all(
        &output,
        &[
            "no supported coding agents detected",
            "use --agent to choose a target",
        ],
    );
}

#[test]
fn skill_install_rejects_unsupported_agent() {
    let env = TestEnv::new();
    let home = env.path("home");
    fs::create_dir_all(&home).unwrap();

    let output = fail(skill_command(
        &home,
        ["skill", "install", "--agent", "unknown"],
    ));
    contains_all(&output, &["invalid value 'unknown'"]);
    contains_none(&output, &["installed aven skill"]);
}

fn skill_command<const N: usize>(home: &std::path::Path, args: [&str; N]) -> std::process::Output {
    let cwd = home;
    skill_command_in(home, cwd, args)
}

fn skill_command_in<const N: usize>(
    home: &std::path::Path,
    cwd: &std::path::Path,
    args: [&str; N],
) -> std::process::Output {
    let mut cmd: Command = command();
    cmd.env("HOME", home)
        .env("AVEN_CONFIG_DIR", home.join("config").join("aven"))
        .env("XDG_STATE_HOME", home.join("state"))
        .current_dir(cwd)
        .args(args);
    cmd.output().expect("run aven skill install")
}
