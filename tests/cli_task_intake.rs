mod common;

use std::fs;

use common::{TestEnv, contains_all, extract_ref, fail, ok};

#[test]
fn add_assigns_cli_source() {
    let env = TestEnv::new();
    let db = env.db("cli-source.sqlite");

    ok(env.aven(&db, ["add", "CLI source", "--project", "app"]));

    assert_eq!(
        sqlite_scalar(&db, "SELECT source FROM tasks WHERE title = 'CLI source'"),
        "cli"
    );
}

#[test]
fn natural_add_uses_configured_task_intake_command() {
    let env = TestEnv::new();
    let db = env.db("natural.sqlite");
    let command = env.path("task-intake.sh");
    let prompt = env.path("prompt.txt");
    fs::write(
        &command,
        format!(
            "#!/bin/sh\ncat >'{}'\nprintf '%s\\n' '{{\"title\":\"fix slack dispatch\",\"description\":\"details from model\",\"project\":\"app\",\"priority\":\"high\",\"labels\":[],\"available_at\":\"in 2 weeks at 9am\",\"due_on\":\"in 3 weeks\"}}'\n",
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
            "available_at=",
            "due_on=",
            "description<<EOF",
            "details from model",
        ],
    );
    let shown_json: serde_json::Value =
        serde_json::from_str(&ok(env.aven_config(["show", &task_ref, "--json"]))).unwrap();
    let available_at = shown_json["available_at"].as_str().unwrap();
    assert_eq!(available_at.len(), 20);
    assert!(available_at.ends_with('Z'));
    let due_on = shown_json["due_on"].as_str().unwrap();
    assert_eq!(due_on.len(), 10);
    assert!(due_on > &available_at[..10]);
    assert_eq!(
        sqlite_scalar(
            &db,
            "SELECT source FROM tasks WHERE title = 'fix slack dispatch'",
        ),
        "cli"
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
            "#!/bin/sh\ncat >'{}'\nprintf '%s\\n' '{{\"title\":\"fix slack sync\",\"description\":\"from model\",\"project\":\"other\",\"priority\":\"none\",\"labels\":[]}}'\n",
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
    system_prompt: "Project={{inferred_project}} Selected={{selected_project}}"
"#,
        db.display(),
        command.display()
    ));

    ok(env.aven_config(["workspace", "create", "client"]));
    let client_workspace_id = workspace_id(&db, "client");
    ok(env.aven_config(["--workspace", "client", "project", "create", "app"]));
    ok(env.aven_config(["--workspace", "client", "project", "create", "other"]));

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
    assert!(prompt.contains("Project=app Selected=app"));
    assert_eq!(pending_undo_count(&db, &client_workspace_id), 0);
    assert_eq!(
        sqlite_scalar(
            &db,
            "SELECT source FROM tasks WHERE title = 'fix slack sync'",
        ),
        "tui"
    );
}

#[test]
fn internal_natural_add_can_record_tui_undo() {
    let env = TestEnv::new();
    let db = env.db("natural-internal-undo.sqlite");
    let command = env.path("task-intake-undo.sh");
    fs::write(
        &command,
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"title\":\"fix undoable sync\",\"description\":\"from model\",\"project\":null,\"priority\":\"none\",\"labels\":[]}'\n",
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
"#,
        db.display(),
        command.display()
    ));

    ok(env.aven_config(["workspace", "create", "client"]));
    let client_workspace_id = workspace_id(&db, "client");
    let out = ok(env.aven_config([
        "internal",
        "natural-add",
        "--workspace-id",
        &client_workspace_id,
        "--input",
        "make this undoable",
        "--tui-undo",
    ]));
    let task_ref = extract_ref(&out);

    let created = ok(env.aven_config(["--workspace", "client", "show", &task_ref, "--full"]));
    contains_all(&created, &["title=\"fix undoable sync\""]);
    assert_eq!(pending_undo_count(&db, &client_workspace_id), 1);
}

#[test]
fn natural_add_creates_recurring_series_and_documents_contract() {
    let env = TestEnv::new();
    let db = env.db("natural-recurrence.sqlite");
    let command = env.path("task-intake-recurrence.sh");
    let prompt = env.path("recurrence-prompt.txt");
    fs::write(
        &command,
        format!(
            "#!/bin/sh\ncat >'{}'\nprintf '%s\\n' '{{\"title\":\"Review metrics\",\"description\":\"\",\"project\":\"app\",\"priority\":\"high\",\"labels\":[],\"repeat\":\"every Monday and Thursday\",\"repeat_at\":\"09:30\",\"repeat_due\":\"none\",\"time_zone\":\"Europe/Stockholm\",\"repeat_start_on\":\"2026-08-03\"}}'\n",
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
"#,
        db.display(),
        command.display()
    ));
    ok(env.aven_config(["project", "create", "app"]));

    let output = ok(env.aven_config([
        "add",
        "Review metrics every Monday and Thursday",
        "--natural",
    ]));
    contains_all(&output, &["created RCR-", "status=todo"]);
    assert_eq!(
        sqlite_scalar(
            &db,
            "SELECT frequency || '|' || interval || '|' || weekdays || '|' || timezone || '|' || start_on || '|' || available_local_time || '|' || due_policy FROM recurrence_series"
        ),
        "weekly|1|mon,thu|Europe/Stockholm|2026-08-03|09:30:00|none"
    );

    let prompt = fs::read_to_string(prompt).unwrap();
    contains_all(
        &prompt,
        &[
            "daily, every Friday, or every Monday and Thursday",
            "Ambiguous timing remains one-off",
            "Recurrence defaults are no availability time, same-day due, the local time zone, and today",
        ],
    );
}

#[test]
fn internal_natural_add_persists_recurrence_defaults() {
    let env = TestEnv::new();
    let db = env.db("natural-recurrence-internal.sqlite");
    let command = env.path("task-intake-recurrence-internal.sh");
    fs::write(
        &command,
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"title\":\"Review metrics\",\"description\":\"\",\"project\":null,\"priority\":\"none\",\"labels\":[],\"repeat\":\"daily\"}'\n",
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
"#,
        db.display(),
        command.display()
    ));
    ok(env.aven_config(["list"]));
    let workspace_id = sqlite_scalar(&db, "SELECT id FROM workspaces LIMIT 1");

    let output = ok(env.aven_config([
        "internal",
        "natural-add",
        "--workspace-id",
        &workspace_id,
        "--input",
        "Review metrics daily",
    ]));
    contains_all(&output, &["created ", "status=todo"]);
    let persisted = sqlite_scalar(
        &db,
        "SELECT frequency || '|' || available_local_time || '|' || due_policy || '|' || timezone || '|' || start_on FROM recurrence_series",
    );
    let fields = persisted.split('|').collect::<Vec<_>>();
    assert_eq!(fields[0], "daily");
    assert_eq!(fields[1], "");
    assert_eq!(fields[2], "same_day");
    assert!(!fields[3].is_empty());
    assert_eq!(fields[4].len(), 10);
    assert_eq!(
        sqlite_scalar(&db, "SELECT count(*) FROM recurrence_occurrences"),
        "1"
    );
}

#[test]
fn invalid_model_recurrence_output_fails_without_creating_task() {
    let env = TestEnv::new();
    let db = env.db("natural-recurrence-invalid.sqlite");
    let command = env.path("task-intake-recurrence-invalid.sh");
    fs::write(
        &command,
        "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{\"title\":\"Review metrics\",\"description\":\"\",\"project\":null,\"priority\":\"none\",\"labels\":[],\"repeat\":\"whenever convenient\"}'\n",
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
"#,
        db.display(),
        command.display()
    ));

    let error = fail(env.aven_config(["add", "Review metrics whenever convenient", "--natural"]));
    contains_all(&error, &["task-intake-recurrence-invalid", "Try daily"]);
    assert_eq!(sqlite_scalar(&db, "SELECT count(*) FROM tasks"), "0");
    assert_eq!(
        sqlite_scalar(&db, "SELECT count(*) FROM recurrence_series"),
        "0"
    );
}

fn workspace_id(db: &std::path::Path, key: &str) -> String {
    sqlite_scalar(
        db,
        &format!("SELECT id FROM workspaces WHERE key = '{key}'"),
    )
}

fn pending_undo_count(db: &std::path::Path, workspace_id: &str) -> i64 {
    sqlite_scalar(
        db,
        &format!(
            "SELECT count(*) FROM tui_undo_entries WHERE workspace_id = '{workspace_id}' AND undone_at IS NULL"
        ),
    )
    .parse()
    .unwrap()
}

fn sqlite_scalar(db: &std::path::Path, query: &str) -> String {
    let output = std::process::Command::new("sqlite3")
        .arg(db)
        .arg(query)
        .output()
        .expect("read sqlite scalar");
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
