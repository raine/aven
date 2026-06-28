mod common;

use common::{TestEnv, TestServer, contains_all, extract_ref, ok};

#[test]
fn config_db_path_and_sync_server_are_used() {
    let env = TestEnv::new();
    let server = TestServer::start(&env);
    let db = env.db("configured.sqlite");
    env.write_config(&format!(
        r#"
local:
  db_path: "{}"

sync:
  enabled: true
  server_url: "{}"
  interval_seconds: 30
"#,
        db.display(),
        server.url
    ));

    let task_ref = extract_ref(&ok(env.aven_config([
        "add",
        "configured task",
        "--project",
        "app",
    ])));
    let sync = ok(env.aven_config(["sync"]));
    contains_all(&sync, &["synced", "cursor="]);

    let shown = ok(env.aven_config(["show", &task_ref]));
    contains_all(&shown, &[&task_ref, "configured task"]);
}
