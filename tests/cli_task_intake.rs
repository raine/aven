mod common;

use std::fs;

use common::{TestEnv, contains_all, extract_ref, ok};

#[test]
fn natural_add_uses_configured_task_intake_command() {
    let env = TestEnv::new();
    let db = env.db("natural.sqlite");
    let command = env.path("task-intake.sh");
    let prompt = env.path("prompt.txt");
    fs::write(
        &command,
        format!(
            "#!/bin/sh\ncat >'{}'\nprintf '%s\\n' '{{\"title\":\"fix slack dispatch\",\"description\":\"details from model\",\"project\":\"app\",\"priority\":\"high\",\"labels\":[]}}'\n",
            prompt.display()
        ),
    )
    .unwrap();
    set_executable(&command);
    env.write_config(&format!(
        r#"
local:
  db_path: "{}"

agent:
  task_intake:
    command: "{}"
    args: []
    timeout_seconds: 5
    system_prompt: "custom task shaping"
"#,
        db.display(),
        command.display()
    ));

    ok(env.aven_config(["project", "create", "app"]));
    let task_ref = extract_ref(&ok(env.aven_config([
        "add",
        "in slack-agent, we need to fix dispatch",
        "--natural",
    ])));

    let shown = ok(env.aven_config(["show", &task_ref, "--full"]));
    contains_all(
        &shown,
        &[
            "title=\"fix slack dispatch\"",
            "project=app",
            "priority=high",
            "description<<EOF",
            "details from model",
        ],
    );
    let prompt = fs::read_to_string(prompt).unwrap();
    contains_all(
        &prompt,
        &[
            "custom task shaping",
            "Available projects:",
            "Raw intake text:",
        ],
    );
}

#[test]
fn tmux_add_task_popup_prints_binding() {
    let env = TestEnv::new();
    let db = env.db("tmux.sqlite");
    let output = ok(env.aven(
        &db,
        [
            "tmux",
            "add-task-popup",
            "--project",
            "app",
            "--print-binding",
        ],
    ));
    contains_all(
        &output,
        &[
            "bind-key A tmux display-popup -E",
            "-d '#{pane_current_path}'",
            "'aven tui --add-task-only --project app'",
        ],
    );
}

#[cfg(unix)]
fn set_executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

#[cfg(not(unix))]
fn set_executable(_path: &std::path::Path) {}
