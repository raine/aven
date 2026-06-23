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
    assert_eq!(prompt, "custom task shaping");
}

#[test]
fn natural_add_expands_custom_task_intake_prompt_placeholders() {
    let env = TestEnv::new();
    let db = env.db("natural-template.sqlite");
    let command = env.path("task-intake-template.sh");
    let prompt = env.path("prompt-template.txt");
    fs::write(
        &command,
        format!(
            "#!/bin/sh\ncat >'{}'\nprintf '%s\n' '{{\"title\":\"fix slack dispatch\",\"description\":\"\",\"project\":null,\"priority\":\"none\",\"labels\":[]}}'\n",
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
    system_prompt: |
      Input={{input}}
      Priorities={{priorities}}
      Inferred={{inferred_project}}
      Projects:
      {{projects}}
      Labels:
      {{labels}}
"#,
        db.display(),
        command.display()
    ));

    ok(env.aven_config(["project", "create", "App"]));
    ok(env.aven_config(["label", "create", "Bug"]));
    let task_ref = extract_ref(&ok(env.aven_config([
        "add",
        "in slack-agent, we need to fix dispatch",
        "--natural",
    ])));

    let shown = ok(env.aven_config(["show", &task_ref, "--full"]));
    contains_all(&shown, &["title=\"fix slack dispatch\"", "priority=none"]);
    let prompt = fs::read_to_string(prompt).unwrap();
    contains_all(
        &prompt,
        &[
            "Input=in slack-agent, we need to fix dispatch",
            "Priorities=none, low, medium, high, urgent",
            "Inferred=aven",
            "Projects:\n- app (App)",
            "Labels:\n- bug",
        ],
    );
}

#[test]
fn internal_natural_add_uses_explicit_workspace_id_and_project_context() {
    let env = TestEnv::new();
    let db = env.db("natural-internal.sqlite");
    let command = env.path("task-intake.sh");
    let prompt = env.path("prompt.txt");
    fs::write(
        &command,
        format!(
            "#!/bin/sh\ncat >'{}'\nprintf '%s\\n' '{{\"title\":\"fix slack sync\",\"description\":\"from model\",\"project\":null,\"priority\":\"none\",\"labels\":[]}}'\n",
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
    system_prompt: "Project={{inferred_project}}"
"#,
        db.display(),
        command.display()
    ));

    ok(env.aven_config(["workspace", "create", "client"]));
    let client_workspace_id = workspace_id(&db, "client");
    ok(env.aven_config(["--workspace", "client", "project", "create", "app"]));

    let out = ok(env.aven_config([
        "internal",
        "natural-add",
        "--workspace-id",
        &client_workspace_id,
        "--project",
        "app",
        "--input",
        "in slack, we need to fix sync",
    ]));
    let task_ref = extract_ref(&out);

    let created = ok(env.aven_config(["--workspace", "client", "show", &task_ref, "--full"]));
    contains_all(
        &created,
        &[
            "project=app",
            "title=\"fix slack sync\"",
            "description<<EOF",
        ],
    );
    let default_list = ok(env.aven_config(["list"]));
    assert!(!default_list.contains("fix slack sync"));

    let prompt = fs::read_to_string(prompt).unwrap();
    assert!(prompt.contains("Project=app"));
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

fn workspace_id(db: &std::path::Path, key: &str) -> String {
    let output = std::process::Command::new("sqlite3")
        .arg(db)
        .arg(format!("SELECT id FROM workspaces WHERE key = '{key}'"))
        .output()
        .expect("read workspace id");
    assert!(output.status.success(), "sqlite failed");
    String::from_utf8(output.stdout).unwrap().trim().to_string()
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
