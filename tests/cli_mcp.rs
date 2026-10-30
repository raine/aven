mod common;

use std::io::{BufRead, BufReader, Write};
use std::net::UdpSocket;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::thread;
use std::time::Duration;

use common::{TestEnv, ok};
use serde_json::{Value, json};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);
const NO_WAKE_WINDOW: Duration = Duration::from_millis(200);

/// Speaks newline-delimited JSON-RPC to an `aven mcp` child. Stdout is read
/// apart from stderr, so anything but protocol messages on stdout fails the test.
struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<Value>,
    next_id: i64,
}

impl Client {
    fn spawn(mut command: Command) -> Self {
        let mut child = command
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn aven mcp");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("child stdout");
        let (sender, messages) = channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let message = serde_json::from_str::<Value>(&line)
                    .unwrap_or_else(|error| panic!("non-protocol stdout {line:?}: {error}"));
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            messages,
            next_id: 0,
        }
    }

    fn started(env: &TestEnv, db: &Path) -> Self {
        let mut client = Self::spawn(env.command(db));
        client.handshake();
        client
    }

    fn send(&mut self, message: Value) {
        let stdin = self.stdin.as_mut().expect("child stdin");
        writeln!(stdin, "{message}").expect("write request");
        stdin.flush().expect("flush request");
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let message = self
                .messages
                .recv_timeout(RESPONSE_TIMEOUT)
                .unwrap_or_else(|error| panic!("no response to {method}: {error}"));
            if message["id"] == id {
                return message;
            }
        }
    }

    fn initialize(&mut self) -> Value {
        self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "cli_mcp", "version": "1"},
            }),
        )
    }

    fn handshake(&mut self) {
        let response = self.initialize();
        assert!(response["result"].is_object(), "{response}");
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    }

    fn call_raw(&mut self, tool: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": tool, "arguments": arguments}))
    }

    /// Returns the tool's JSON body and whether it reported an error.
    fn call(&mut self, tool: &str, arguments: Value) -> (bool, Value) {
        let response = self.call_raw(tool, arguments);
        let result = &response["result"];
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{tool} returned no text: {response}"));
        let body = serde_json::from_str(text)
            .unwrap_or_else(|error| panic!("{tool} returned non-JSON {text:?}: {error}"));
        (result["isError"] == true, body)
    }

    fn ok(&mut self, tool: &str, arguments: Value) -> Value {
        let (is_error, body) = self.call(tool, arguments);
        assert!(!is_error, "{tool} failed: {body}");
        body
    }

    fn error(&mut self, tool: &str, arguments: Value) -> Value {
        let (is_error, body) = self.call(tool, arguments);
        assert!(is_error, "{tool} should fail: {body}");
        assert!(body["message"].is_string(), "{body}");
        assert!(
            body["hint"].as_str().is_some_and(|hint| !hint.is_empty()),
            "{body}"
        );
        body
    }

    fn create(&mut self, arguments: Value) -> String {
        let body = self.ok("create_task", arguments);
        task_ref(&body["task"])
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn task_ref(row: &Value) -> String {
    row["ref"].as_str().expect("row ref").to_string()
}

fn titles(body: &Value) -> Vec<String> {
    let mut titles = body["tasks"]
        .as_array()
        .expect("task rows")
        .iter()
        .map(|task| task["title"].as_str().expect("title").to_string())
        .collect::<Vec<_>>();
    titles.sort();
    titles
}

#[test]
fn initialize_repeats_on_one_process_after_a_discovery_probe() {
    let env = TestEnv::new();
    let db = env.db("handshake.sqlite");
    let mut client = Client::spawn(env.command(&db));
    let pid = client.child.id();

    // ChatGPT probes first; the probe may be rejected but must not end the session.
    let probe = client.request("server/discover", json!({}));
    assert!(probe["id"].is_number(), "{probe}");

    let first = client.initialize();
    assert_eq!(first["result"]["serverInfo"]["name"], "aven", "{first}");
    assert!(
        first["result"]["instructions"]
            .as_str()
            .is_some_and(|text| text.contains("count_tasks")),
        "{first}"
    );
    client.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    let task = client.create(json!({"title": "First turn"}));

    for _ in 0..2 {
        let again = client.initialize();
        assert_eq!(again["result"]["serverInfo"]["name"], "aven", "{again}");
        client.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        let shown = client.ok("show_task", json!({"ref": task}));
        assert_eq!(shown["task"]["title"], "First turn");
    }
    assert_eq!(client.child.id(), pid);

    client.stdin = None;
    let status = client.child.wait().expect("wait for exit");
    assert!(status.success(), "{status}");
}

#[test]
fn serves_the_message_order_chatgpt_sends_through_the_tunnel() {
    let env = TestEnv::new();
    let db = env.db("tunnel.sqlite");
    let mut client = Client::spawn(env.command(&db));
    let initialize = json!({
        "protocolVersion": "2025-11-25",
        "capabilities": {"experimental": {"openai/visibility": {"enabled": true}}},
        "clientInfo": {"name": "openai-mcp", "version": "1.0.0"},
    });

    let discovered = client.request(
        "server/discover",
        json!({"_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientInfo": {"name": "openai-mcp", "version": "1.0.0"},
            "io.modelcontextprotocol/clientCapabilities": {},
        }}),
    );
    assert!(discovered["id"].is_number(), "{discovered}");

    // Two initializes arrive before the first initialized notification.
    for _ in 0..2 {
        let response = client.request("initialize", initialize.clone());
        assert_eq!(
            response["result"]["serverInfo"]["name"], "aven",
            "{response}"
        );
    }
    client.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    let listed = client.request("tools/list", json!({}));
    assert_eq!(
        listed["result"]["tools"].as_array().expect("tools").len(),
        12
    );

    // Each later chat turn repeats the handshake, then calls a tool with metadata.
    for _ in 0..2 {
        let response = client.request("initialize", initialize.clone());
        assert_eq!(
            response["result"]["serverInfo"]["name"], "aven",
            "{response}"
        );
        client.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        let called = client.request(
            "tools/call",
            json!({
                "_meta": {"openai/userAgent": "test"},
                "name": "list_tasks",
                "arguments": {},
            }),
        );
        assert_eq!(called["result"]["isError"], false, "{called}");
    }
}

#[test]
fn tools_list_has_the_twelve_tools_with_explicit_annotations() {
    let env = TestEnv::new();
    let mut client = Client::started(&env, &env.db("catalog.sqlite"));

    let response = client.request("tools/list", json!({}));
    let tools = response["result"]["tools"].as_array().expect("tools");
    let mut seen = tools
        .iter()
        .map(|tool| {
            let annotations = &tool["annotations"];
            assert_eq!(annotations["openWorldHint"], false, "{tool}");
            (
                tool["name"].as_str().expect("name"),
                annotations["readOnlyHint"].as_bool().expect("readOnlyHint"),
                annotations["destructiveHint"].as_bool(),
            )
        })
        .collect::<Vec<_>>();
    seen.sort();
    assert_eq!(
        seen,
        [
            ("add_note", false, Some(false)),
            ("count_tasks", true, None),
            ("create_task", false, Some(false)),
            ("delete_task", false, Some(true)),
            ("edit_recurring", false, Some(true)),
            ("list_projects", true, None),
            ("list_recurring", true, None),
            ("list_tasks", true, None),
            ("list_workspaces", true, None),
            ("search_tasks", true, None),
            ("show_task", true, None),
            ("update_task", false, Some(true)),
        ]
    );

    // Tool schemas are read by the client's model on every conversation.
    let size = serde_json::to_string(tools).expect("serialize tools").len();
    assert!(size < 12_000, "tool catalog is {size} bytes");
}

#[test]
fn tasks_move_through_create_update_note_and_delete() {
    let env = TestEnv::new();
    let db = env.db("lifecycle.sqlite");
    ok(env.aven(&db, ["project", "create", "app"]));
    ok(env.aven(&db, ["project", "create", "home"]));
    let mut client = Client::started(&env, &db);

    // No project means the default project, which a fresh database lacks.
    let inbox = client.ok("create_task", json!({"title": "Unfiled"}));
    assert_eq!(inbox["task"]["project"], "default");

    let created = client.ok(
        "create_task",
        json!({
            "title": "Fix login",
            "description": "Sessions expire early",
            "project": "app",
            "priority": "high",
            "status": "todo",
            "labels": ["bug"],
            "due": "2031-02-03",
        }),
    );
    let task = task_ref(&created["task"]);
    assert_eq!(created["task"]["due_on"], "2031-02-03");
    assert_eq!(created["task"]["priority"], "high");

    // Searches cover every project without one being named.
    let found = client.ok("search_tasks", json!({"query": "login"}));
    assert_eq!(titles(&found), ["Fix login"]);
    assert_eq!(found["truncated"], false);
    assert_eq!(found["tasks"][0]["labels"], json!(["bug"]));
    assert_eq!(found["tasks"][0]["description"], Value::Null);

    let in_home = client.ok("search_tasks", json!({"query": "login", "project": "home"}));
    assert_eq!(titles(&in_home), Vec::<String>::new());

    let noted = client.ok("add_note", json!({"ref": task, "body": "Seen on iOS"}));
    assert!(noted["note"].is_string());

    let updated = client.ok(
        "update_task",
        json!({
            "ref": task,
            "title": "Fix login expiry",
            "project": "home",
            "status": "active",
            "due": "none",
            "labels_add": ["urgent-fix"],
            "labels_remove": ["bug"],
        }),
    );
    assert_eq!(updated["changed"], true);
    assert_eq!(updated["task"]["status"], "active");
    assert_eq!(updated["task"]["project"], "home");
    assert!(updated["task"].get("due_on").is_none());

    // Omitted and null fields stay as they are.
    let untouched = client.ok(
        "update_task",
        json!({"ref": task, "priority": "low", "due": null, "title": null}),
    );
    assert_eq!(untouched["task"]["title"], "Fix login expiry");

    let shown = client.ok("show_task", json!({"ref": task}));
    assert_eq!(shown["description"], "Sessions expire early");
    assert_eq!(shown["notes"][0]["body"], "Seen on iOS");
    assert_eq!(shown["task"]["labels"], json!(["urgent-fix"]));
    assert_eq!(shown["task"]["priority"], "low");

    let projects = client.ok("list_projects", json!({}));
    let keys = projects["projects"]
        .as_array()
        .expect("projects")
        .iter()
        .map(|project| project["key"].as_str().expect("key"))
        .collect::<Vec<_>>();
    assert!(keys.contains(&"app") && keys.contains(&"home"), "{keys:?}");

    let deleted = client.ok("delete_task", json!({"ref": task}));
    assert_eq!(deleted["task"]["deleted"], true);
    let gone = client.ok("search_tasks", json!({"query": "login"}));
    assert_eq!(titles(&gone), Vec::<String>::new());
    let listed = client.ok("list_tasks", json!({"view": "deleted"}));
    assert_eq!(titles(&listed), ["Fix login expiry"]);
}

#[test]
fn lists_are_capped_while_counts_are_not() {
    let env = TestEnv::new();
    let db = env.db("limits.sqlite");
    let mut client = Client::started(&env, &db);
    for index in 0..3 {
        client.create(json!({"title": format!("Task {index}"), "status": "todo"}));
    }
    let done = client.create(json!({"title": "Finished", "status": "todo"}));
    client.ok("update_task", json!({"ref": done, "status": "done"}));

    let capped = client.ok("list_tasks", json!({"limit": 2}));
    assert_eq!(capped["tasks"].as_array().expect("rows").len(), 2);
    assert_eq!(capped["truncated"], true);

    let exact = client.ok("list_tasks", json!({"limit": 3}));
    assert_eq!(exact["tasks"].as_array().expect("rows").len(), 3);
    assert_eq!(exact["truncated"], false);

    let searched = client.ok("search_tasks", json!({"query": "Task", "limit": 1}));
    assert_eq!(searched["tasks"].as_array().expect("rows").len(), 1);
    assert_eq!(searched["truncated"], true);

    assert_eq!(client.ok("count_tasks", json!({}))["count"], 3);
    assert_eq!(client.ok("count_tasks", json!({"view": "all"}))["count"], 4);
    assert_eq!(
        client.ok("count_tasks", json!({"status": "done"}))["count"],
        1
    );

    client.error("list_tasks", json!({"limit": 0}));
    client.error("list_tasks", json!({"limit": 201}));
}

#[test]
fn overdue_uses_the_callers_date() {
    let env = TestEnv::new();
    let db = env.db("overdue.sqlite");
    let mut client = Client::started(&env, &db);
    client.create(json!({"title": "Taxes", "status": "todo", "due": "2030-06-15"}));

    let view = |today: &str| json!({"view": "overdue", "today": today});
    assert_eq!(client.ok("count_tasks", view("2030-06-15"))["count"], 0);
    assert_eq!(client.ok("count_tasks", view("2030-06-16"))["count"], 1);
    let listed = client.ok("list_tasks", view("2030-06-16"));
    assert_eq!(titles(&listed), ["Taxes"]);

    let missing = client.error("list_tasks", json!({"view": "overdue"}));
    assert_eq!(missing["error"], "today-required");
}

#[test]
fn recurring_series_complete_skip_and_change_state() {
    let env = TestEnv::new();
    let db = env.db("recurring.sqlite");
    let mut client = Client::started(&env, &db);

    let created = client.ok(
        "create_task",
        json!({
            "title": "Water plants",
            "repeat": "every Friday",
            "time_zone": "Europe/Helsinki",
            "repeat_at": "09:00",
            "repeat_start_on": "2026-01-02",
            "labels": ["home"],
        }),
    );
    let series = created["series"].as_str().expect("series ref").to_string();
    let first = task_ref(&created["task"]);
    assert_eq!(created["task"]["project"], "default");

    let listed = client.ok("list_recurring", json!({}));
    assert_eq!(listed["series"][0]["ref"], series.as_str());
    assert_eq!(listed["series"][0]["state"], "active");
    assert_eq!(listed["series"][0]["time_zone"], "Europe/Helsinki");
    assert_eq!(listed["series"][0]["current_task"], first.as_str());

    // Completing the occurrence is an ordinary status update.
    client.ok("update_task", json!({"ref": first, "status": "done"}));
    let after_done = client.ok("list_recurring", json!({}));
    let second = after_done["series"][0]["current_task"]
        .as_str()
        .expect("successor")
        .to_string();
    assert_ne!(second, first);

    let edited = client.ok(
        "edit_recurring",
        json!({"ref": series, "title": "Water all plants", "repeat_at": "none"}),
    );
    assert_eq!(edited["state"], "active");

    let skipped = client.ok("edit_recurring", json!({"ref": series, "action": "skip"}));
    assert!(skipped["skipped_slot"].is_string(), "{skipped}");
    let third = skipped["successor"].as_str().expect("successor");
    assert_ne!(third, second);
    let shown = client.ok("show_task", json!({"ref": third}));
    assert_eq!(shown["task"]["title"], "Water all plants");

    let pause = json!({"ref": series, "action": "pause"});
    assert_eq!(client.ok("edit_recurring", pause)["state"], "paused");
    let resume = json!({"ref": series, "action": "resume"});
    assert_eq!(client.ok("edit_recurring", resume)["state"], "active");
    let stop = json!({"ref": series, "action": "stop", "skip_current": true});
    assert_eq!(client.ok("edit_recurring", stop)["state"], "stopped");
    let remaining = client.ok("list_recurring", json!({}));
    assert_eq!(remaining["series"][0]["current_task"], Value::Null);
}

#[test]
fn recurring_inputs_are_rejected_without_writing() {
    let env = TestEnv::new();
    let db = env.db("recurring-errors.sqlite");
    let mut client = Client::started(&env, &db);

    let no_zone = client.error("create_task", json!({"title": "A", "repeat": "daily"}));
    assert_eq!(no_zone["error"], "time-zone-required");
    let no_repeat = client.error("create_task", json!({"title": "A", "time_zone": "UTC"}));
    assert_eq!(no_repeat["error"], "recurrence-fields-require-repeat");
    client.error(
        "create_task",
        json!({"title": "A", "repeat": "daily", "time_zone": "UTC", "due": "2031-01-01"}),
    );
    client.error(
        "create_task",
        json!({"title": "A", "repeat": "sometimes", "time_zone": "UTC"}),
    );
    client.error(
        "create_task",
        json!({"title": "A", "repeat": "daily", "time_zone": "UTC", "status": "later"}),
    );
    client.error(
        "create_task",
        json!({"title": "A", "repeat": "daily", "time_zone": "UTC", "priority": "huge"}),
    );
    assert_eq!(client.ok("count_tasks", json!({"view": "all"}))["count"], 0);

    let created = client.ok(
        "create_task",
        json!({"title": "B", "repeat": "daily", "time_zone": "UTC"}),
    );
    let series = created["series"].as_str().expect("series ref");
    let both = client.error(
        "edit_recurring",
        json!({"ref": series, "action": "skip", "title": "C"}),
    );
    assert_eq!(both["error"], "recurring-edit-conflict");
    let neither = client.error("edit_recurring", json!({"ref": series}));
    assert_eq!(neither["error"], "recurring-edit-conflict");
    let shown = client.ok("list_recurring", json!({}));
    assert_eq!(shown["series"][0]["title"], "B");
}

#[test]
fn workspace_selection_ignores_the_working_directory() {
    let env = TestEnv::new();
    let db = env.db("workspaces.sqlite");
    let routed = env.path("routed");
    std::fs::create_dir_all(&routed).expect("create routed dir");
    ok(env.aven(&db, ["workspace", "create", "alpha"]));
    ok(env.aven(&db, ["workspace", "create", "beta"]));
    env.write_config(&format!(
        r#"
local:
  db_path: "{}"
workspace:
  default: "beta"
  routes:
    - workspace: "alpha"
      paths: ["{}"]
"#,
        db.display(),
        routed.display()
    ));

    // A route that would select alpha for the CLI has no effect here.
    let mut command = env.command(&db);
    command.current_dir(&routed);
    let mut client = Client::spawn(command);
    client.handshake();

    let in_default = client.ok("create_task", json!({"title": "Beta task"}));
    let beta_ref = task_ref(&in_default["task"]);
    assert_eq!(client.ok("list_tasks", json!({}))["workspace"], "beta");

    let in_alpha = client.ok(
        "create_task",
        json!({"title": "Alpha task", "workspace": "alpha"}),
    );
    let alpha_ref = task_ref(&in_alpha["task"]);
    let alpha_list = client.ok("list_tasks", json!({"workspace": "alpha"}));
    assert_eq!(alpha_list["workspace"], "alpha");
    assert_eq!(titles(&alpha_list), ["Alpha task"]);
    assert_eq!(titles(&client.ok("list_tasks", json!({}))), ["Beta task"]);

    // Refs resolve only inside the selected workspace.
    let wrong = client.error("update_task", json!({"ref": alpha_ref, "status": "done"}));
    assert_eq!(wrong["error"], "unknown-ref");
    client.ok("show_task", json!({"ref": beta_ref}));

    let listed = client.ok("list_workspaces", json!({}));
    let default = listed["workspaces"]
        .as_array()
        .expect("workspaces")
        .iter()
        .find(|workspace| workspace["default"] == true)
        .expect("default workspace");
    assert_eq!(default["key"], "beta");

    let unknown = client.error("list_tasks", json!({"workspace": "missing"}));
    assert_eq!(unknown["error"], "unknown-workspace");
    assert!(
        unknown["hint"]
            .as_str()
            .expect("hint")
            .contains("list_workspaces")
    );
    client.ok("list_tasks", json!({}));
}

#[test]
fn workspace_flag_changes_the_default_without_restricting_access() {
    let env = TestEnv::new();
    let db = env.db("workspace-flag.sqlite");
    ok(env.aven(&db, ["workspace", "create", "alpha"]));
    ok(env.aven(&db, ["workspace", "create", "beta"]));

    let mut command = env.command(&db);
    command.args(["--workspace", "alpha"]);
    let mut client = Client::spawn(command);
    client.handshake();

    assert_eq!(client.ok("list_tasks", json!({}))["workspace"], "alpha");
    let beta = client.ok("list_tasks", json!({"workspace": "beta"}));
    assert_eq!(beta["workspace"], "beta");
}

#[test]
fn task_errors_are_tool_results_and_protocol_errors_stay_json_rpc() {
    let env = TestEnv::new();
    let db = env.db("errors.sqlite");
    let mut client = Client::started(&env, &db);
    let task = client.create(json!({"title": "Real task"}));

    let unknown_ref = client.error("show_task", json!({"ref": "NOPE-0000"}));
    assert_eq!(unknown_ref["error"], "unknown-ref");
    assert!(
        unknown_ref["hint"]
            .as_str()
            .expect("hint")
            .contains("search_tasks")
    );

    let unknown_project = client.error("create_task", json!({"title": "X", "project": "nope"}));
    assert_eq!(unknown_project["error"], "unknown-project");
    assert!(
        unknown_project["hint"]
            .as_str()
            .expect("hint")
            .contains("list_projects")
    );

    let unknown_label = client.error(
        "update_task",
        json!({"ref": task, "labels_remove": ["missing"]}),
    );
    assert_eq!(unknown_label["error"], "unknown-label");

    assert_eq!(
        client.error("create_task", json!({"title": "X", "status": "later"}))["error"],
        "invalid-status"
    );
    client.error("create_task", json!({"title": "X", "due": "tomorrow"}));
    client.error("update_task", json!({"ref": task, "due": ""}));
    for arguments in [
        json!({"ref": task, "title": "Y", "status": "later"}),
        json!({"ref": task, "title": "Y", "priority": "huge"}),
        json!({"ref": task, "title": "Y", "due": "tomorrow"}),
        json!({"ref": task, "title": "Y", "available_at": "soon"}),
    ] {
        client.error("update_task", arguments);
    }
    assert_ne!(
        client.ok("show_task", json!({"ref": task}))["task"]["title"],
        "Y"
    );

    for arguments in [
        json!({"title": "X", "invented": true}),
        json!({"title": 42}),
        json!({}),
    ] {
        let error = client.error("create_task", arguments);
        assert_eq!(error["error"], "invalid-arguments");
    }
    let bad_view = client.error("list_tasks", json!({"view": "someday"}));
    assert_eq!(bad_view["error"], "invalid-arguments");
    assert_eq!(client.ok("count_tasks", json!({"view": "all"}))["count"], 1);

    let unknown_tool = client.call_raw("not_a_tool", json!({}));
    assert!(unknown_tool["error"]["code"].is_number(), "{unknown_tool}");
    let unknown_method = client.request("no/such/method", json!({}));
    assert!(
        unknown_method["error"]["code"].is_number(),
        "{unknown_method}"
    );

    // The process keeps serving after every failure above.
    client.ok("show_task", json!({"ref": task}));
}

#[test]
fn ambiguous_refs_list_candidates_without_touching_stdout() {
    let env = TestEnv::new();
    let db = env.db("ambiguous.sqlite");
    ok(env.aven(&db, ["project", "create", "ambig"]));
    common::execute_sql(
        &db,
        "INSERT INTO tasks(id,title,description,project_id,status,priority,created_at,updated_at)
         VALUES ('7KQ1111111111111', 'One', '', (SELECT id FROM projects WHERE key = 'ambig'),
                 'inbox', 'none', 't', 't'),
                ('7KQ2222222222222', 'Two', '', (SELECT id FROM projects WHERE key = 'ambig'),
                 'inbox', 'none', 't', 't')",
    );
    let mut client = Client::started(&env, &db);

    let error = client.error("show_task", json!({"ref": "7KQ"}));
    assert_eq!(error["error"], "ambiguous-ref");
    let message = error["message"].as_str().expect("message");
    assert!(
        message.contains("One") && message.contains("Two"),
        "{message}"
    );
}

#[test]
fn every_write_wakes_the_daemon_and_nothing_else_does() {
    let env = TestEnv::new();
    let db = env.db("wake.sqlite");
    let socket = UdpSocket::bind("127.0.0.1:0").expect("bind wake socket");
    socket
        .set_read_timeout(Some(RESPONSE_TIMEOUT))
        .expect("set read timeout");
    let addr = socket.local_addr().expect("wake addr").to_string();
    env.write_daemon_config(&db, &addr, 3600);

    let mut client = Client::started(&env, &db);
    let expect_wake = |label: &str| {
        let mut buffer = [0; 8];
        let (size, _) = socket
            .recv_from(&mut buffer)
            .unwrap_or_else(|error| panic!("no wake after {label}: {error}"));
        assert_eq!(&buffer[..size], b"1", "{label}");
    };
    let expect_quiet = |label: &str| {
        socket
            .set_read_timeout(Some(NO_WAKE_WINDOW))
            .expect("set quiet timeout");
        let mut buffer = [0; 8];
        assert!(socket.recv_from(&mut buffer).is_err(), "wake after {label}");
        socket
            .set_read_timeout(Some(RESPONSE_TIMEOUT))
            .expect("restore timeout");
    };

    let task = client.create(json!({"title": "Waking", "status": "todo"}));
    expect_wake("create_task");
    client.ok("update_task", json!({"ref": task, "priority": "high"}));
    expect_wake("update_task");
    client.ok("add_note", json!({"ref": task, "body": "Noted"}));
    expect_wake("add_note");
    let created = client.ok(
        "create_task",
        json!({"title": "Series", "repeat": "daily", "time_zone": "UTC"}),
    );
    expect_wake("create_task with repeat");
    let series = created["series"].as_str().expect("series ref").to_string();
    client.ok("edit_recurring", json!({"ref": series, "title": "Renamed"}));
    expect_wake("edit_recurring template");
    for action in ["skip", "pause", "resume", "stop"] {
        client.ok("edit_recurring", json!({"ref": series, "action": action}));
        expect_wake(action);
    }
    client.ok("delete_task", json!({"ref": task}));
    expect_wake("delete_task");

    client.ok("list_tasks", json!({}));
    client.ok("search_tasks", json!({"query": "Waking"}));
    client.ok("count_tasks", json!({}));
    client.ok("show_task", json!({"ref": task}));
    client.ok("list_projects", json!({}));
    client.ok("list_workspaces", json!({}));
    client.ok("list_recurring", json!({}));
    expect_quiet("reads");
    client.error("update_task", json!({"ref": "NOPE-0000", "title": "X"}));
    expect_quiet("a rejected write");
}
