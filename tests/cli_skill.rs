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
            "# Aven CLI Primer",
            "Do not create tasks unless the user asks",
            "aven list --ready",
            "aven context APP-7KQ9",
            "aven edit APP-7KQ9 --status active",
        ],
    );
    assert!(!home.join(".codex/skills/aven/SKILL.md").exists());
}

#[test]
fn skill_install_targets_pi_idempotently() {
    let env = TestEnv::new();
    let home = env.path("home");
    fs::create_dir_all(&home).unwrap();

    let args = ["skill", "install", "--agent", "pi"];
    let first_output = ok(skill_command(&home, args));
    contains_all(
        &first_output,
        &["installed aven skill for Pi at ~/.pi/agent/skills/aven/SKILL.md"],
    );

    let skill_path = home.join(".pi/agent/skills/aven/SKILL.md");
    let first_content = fs::read_to_string(&skill_path).unwrap();
    contains_all(
        &first_content,
        &[
            "---\nname: aven\ndescription: Use aven to find tasks, update status, and leave durable handoff context.\n---",
            "# Aven CLI Primer",
            "aven list --ready",
            "aven edit APP-7KQ9 --status active",
        ],
    );

    let second_output = ok(skill_command(&home, args));
    contains_all(
        &second_output,
        &["installed aven skill for Pi at ~/.pi/agent/skills/aven/SKILL.md"],
    );
    assert_eq!(fs::read_to_string(skill_path).unwrap(), first_content);
    assert!(!home.join(".claude/skills/aven/SKILL.md").exists());
    assert!(!home.join(".codex/skills/aven/SKILL.md").exists());
}

#[test]
fn skill_install_defaults_to_detected_user_and_workspace_agents() {
    let env = TestEnv::new();
    let home = env.path("home");
    let repo = env.path("repo");
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::create_dir_all(home.join(".pi/agent")).unwrap();
    fs::create_dir_all(repo.join(".opencode")).unwrap();

    let output = ok(skill_command_in(&home, &repo, ["skill", "install"]));
    contains_all(
        &output,
        &[
            "installed aven skill for Claude Code at ~/.claude/skills/aven/SKILL.md",
            "installed aven skill for OpenCode at ~/.config/opencode/skills/aven/SKILL.md",
            "installed aven skill for Codex at ~/.codex/skills/aven/SKILL.md",
            "installed aven skill for Pi at ~/.pi/agent/skills/aven/SKILL.md",
        ],
    );

    assert!(home.join(".claude/skills/aven/SKILL.md").exists());
    assert!(home.join(".config/opencode/skills/aven/SKILL.md").exists());
    assert!(home.join(".codex/skills/aven/SKILL.md").exists());
    assert!(home.join(".pi/agent/skills/aven/SKILL.md").exists());
}

#[test]
fn skill_install_detects_pi_workspace_config() {
    let env = TestEnv::new();
    let home = env.path("home");
    let repo = env.path("repo");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(repo.join(".pi")).unwrap();

    let output = ok(skill_command_in(&home, &repo, ["skill", "install"]));
    contains_all(
        &output,
        &["installed aven skill for Pi at ~/.pi/agent/skills/aven/SKILL.md"],
    );
    assert!(home.join(".pi/agent/skills/aven/SKILL.md").exists());
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
            "~/.pi/agent",
            "use --agent to choose a target",
        ],
    );
}

#[test]
fn skill_install_help_lists_pi_target() {
    let env = TestEnv::new();
    let home = env.path("home");
    fs::create_dir_all(&home).unwrap();

    let output = ok(skill_command(&home, ["skill", "install", "--help"]));
    contains_all(&output, &["--agent <AGENT>", "pi"]);
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
