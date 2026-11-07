use std::collections::{BTreeMap, HashSet};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::sync::Mutex;

use tempfile::TempDir;

use super::*;

/// launchd and codesign, for one fixture home.
#[derive(Default)]
struct Fake {
    signed: Mutex<HashSet<PathBuf>>,
    agent: Mutex<AgentState>,
    fail_start: Mutex<bool>,
    calls: Mutex<Vec<String>>,
    background: Mutex<Option<BackgroundIntent>>,
}

impl Fake {
    fn sign(&self, path: &Path) {
        self.signed
            .lock()
            .unwrap()
            .insert(path.canonicalize().unwrap());
    }

    fn agent_is(&self, loaded: bool, disabled: bool) {
        *self.agent.lock().unwrap() = AgentState { loaded, disabled };
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl System for Fake {
    fn signing_identifier(&self, path: &Path) -> Option<String> {
        let path = path.canonicalize().ok()?;
        self.signed
            .lock()
            .unwrap()
            .contains(&path)
            .then(|| AVEN_BUNDLE_ID.to_string())
    }

    fn agent(&self) -> Result<AgentState> {
        Ok(*self.agent.lock().unwrap())
    }

    fn stop_agent(&self) -> Result<()> {
        self.calls.lock().unwrap().push("stop".into());
        self.agent.lock().unwrap().loaded = false;
        Ok(())
    }

    fn start_agent(&self) -> Result<()> {
        self.calls.lock().unwrap().push("start".into());
        if std::mem::take(&mut *self.fail_start.lock().unwrap()) {
            bail!("bootstrap failed");
        }
        self.agent.lock().unwrap().loaded = true;
        Ok(())
    }

    fn unregister(&self, bundle: &Path) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("unregister {}", bundle.display()));
        Ok(())
    }

    fn set_background(&self, intent: BackgroundIntent) -> Result<()> {
        *self.background.lock().unwrap() = Some(intent);
        Ok(())
    }
}

struct Mac {
    _root: TempDir,
    root: PathBuf,
    layout: Layout,
    system: Fake,
}

impl Mac {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let home = root.join("home");
        let app = root.join("Applications/Aven.app");
        bundle(&app, AVEN_BUNDLE_ID);
        let layout = Layout {
            running: app.join(BUNDLE_EXECUTABLE),
            app,
            path: Some(vec![
                home.join(".local/bin"),
                root.join("brew/bin"),
                "/usr/bin".into(),
            ]),
            brew_prefixes: vec![root.join("brew")],
            agent_plist: service::agent_plist_path(&home),
            state_file: home.join(".local/state/aven/desktop-setup.json"),
            home,
        };
        std::fs::create_dir_all(layout.home.join(".local/bin")).unwrap();
        std::fs::create_dir_all(root.join("brew/bin")).unwrap();
        Self {
            _root: dir,
            root,
            layout,
            system: Fake::default(),
        }
    }

    fn exe(&self) -> PathBuf {
        self.layout.executable()
    }

    /// The install script's bundle and its command link.
    fn script_install(&self) -> PathBuf {
        let bundle = self.layout.script_bundle();
        crate::desktop_install::tests::bundle(&bundle, AVEN_BUNDLE_ID);
        let link = self.layout.command_link();
        symlink(bundle.join(BUNDLE_EXECUTABLE), &link).unwrap();
        bundle.join(BUNDLE_EXECUTABLE)
    }

    /// The formula's keg with Homebrew's bin and opt links.
    fn formula_install(&self) -> PathBuf {
        let keg = self.root.join("brew/Cellar/aven/0.1.38");
        bundle(&keg.join(BUNDLE_NAME), AVEN_BUNDLE_ID);
        std::fs::create_dir_all(self.root.join("brew/opt")).unwrap();
        symlink(&keg, self.root.join("brew/opt/aven")).unwrap();
        symlink(
            keg.join(BUNDLE_NAME).join(BUNDLE_EXECUTABLE),
            self.root.join("brew/bin/aven"),
        )
        .unwrap();
        self.root
            .join("brew/opt/aven")
            .join(BUNDLE_NAME)
            .join(BUNDLE_EXECUTABLE)
    }

    fn uninstall_formula(&self) {
        std::fs::remove_dir_all(self.root.join("brew/Cellar/aven")).unwrap();
        std::fs::remove_file(self.root.join("brew/opt/aven")).unwrap();
        std::fs::remove_file(self.root.join("brew/bin/aven")).unwrap();
    }

    fn agent(&self, program: &Path, loaded: bool, disabled: bool) {
        std::fs::create_dir_all(self.layout.agent_plist.parent().unwrap()).unwrap();
        std::fs::write(&self.layout.agent_plist, agent_plist(program)).unwrap();
        self.system.agent_is(loaded, disabled);
    }

    fn survey(&self) -> Survey {
        survey(&self.layout, &self.system).unwrap()
    }

    fn plan(&self) -> Plan {
        plan(&self.layout, &self.survey())
    }

    fn switch_plan(&self) -> Switch {
        match self.plan() {
            Plan::Switch(switch) => switch,
            other => panic!("expected a switch, got {other:?}"),
        }
    }

    fn switch(&self) -> Result<Outcome> {
        switch(&self.layout, &self.system, &self.switch_plan())
    }

    /// Every path under the fixture root, with link targets and file text.
    fn tree(&self) -> BTreeMap<PathBuf, String> {
        fn walk(dir: &Path, into: &mut BTreeMap<PathBuf, String>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                let meta = path.symlink_metadata().unwrap();
                let value = if meta.is_symlink() {
                    format!("-> {}", std::fs::read_link(&path).unwrap().display())
                } else if meta.is_dir() {
                    walk(&path, into);
                    "dir".to_string()
                } else {
                    std::fs::read_to_string(&path).unwrap_or_default()
                };
                into.insert(path, value);
            }
        }
        let mut tree = BTreeMap::new();
        walk(&self.root, &mut tree);
        tree
    }
}

fn bundle(path: &Path, id: &str) {
    std::fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
    std::fs::write(
        path.join("Contents/Info.plist"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
             <key>CFBundleIdentifier</key><string>{id}</string></dict></plist>\n"
        ),
    )
    .unwrap();
    let exe = path.join(BUNDLE_EXECUTABLE);
    std::fs::write(&exe, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A plist as `aven daemon install` writes it, with a custom database and
/// config directory that a switch must keep.
fn agent_plist(program: &Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<plist version=\"1.0\">
<dict>
  <key>Label</key>
  <string>com.raine.aven.daemon</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
    <string>--db</string>
    <string>/Volumes/Data/tasks &amp; more.sqlite</string>
    <string>daemon</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>/custom/bin:/usr/bin</string>
    <key>AVEN_CONFIG_DIR</key>
    <string>/Volumes/Data/config</string>
  </dict>
  <key>KeepAlive</key>
  <true/>
  <key>ThrottleInterval</key>
  <integer>45</integer>
</dict>
</plist>
",
        program.display()
    )
}

fn extract(plist: &Path, key: &str) -> String {
    plutil(&["-extract", key, "raw", "-o", "-"], plist)
        .unwrap()
        .trim_end()
        .to_string()
}

fn assert_settings_kept(plist: &Path) {
    assert_eq!(extract(plist, "ProgramArguments.1"), "--db");
    assert_eq!(
        extract(plist, "ProgramArguments.2"),
        "/Volumes/Data/tasks & more.sqlite"
    );
    assert_eq!(extract(plist, "ProgramArguments.3"), "daemon");
    assert_eq!(
        extract(plist, "EnvironmentVariables.PATH"),
        "/custom/bin:/usr/bin"
    );
    assert_eq!(
        extract(plist, "EnvironmentVariables.AVEN_CONFIG_DIR"),
        "/Volumes/Data/config"
    );
    assert_eq!(extract(plist, "ThrottleInterval"), "45");
    assert_eq!(extract(plist, "Label"), "com.raine.aven.daemon");
}

fn keep_alive(plist: &Path) -> serde_json::Value {
    let json = plutil(&["-extract", "KeepAlive", "json", "-o", "-"], plist).unwrap();
    serde_json::from_str(&json).unwrap()
}

fn path_state(program: &Path) -> serde_json::Value {
    serde_json::json!({ "PathState": { program.to_str().unwrap(): true } })
}

fn report(outcome: Outcome) -> Report {
    match outcome {
        Outcome::Done(report) => report,
        other => panic!("expected done, got {other:?}"),
    }
}

#[test]
fn script_install_moves_command_and_daemon_then_retires_its_bundle() {
    let mac = Mac::new();
    let old = mac.script_install();
    mac.agent(&old, true, false);
    assert!(needs_setup(&mac.layout));

    let plan = mac.switch_plan();
    assert!(plan.script && !plan.formula && !plan.resuming);
    assert_eq!(plan.repoint, vec![mac.layout.command_link()]);
    assert_eq!(plan.retire, Some(mac.layout.script_bundle()));

    let report = report(mac.switch().unwrap());
    assert!(report.switched);
    assert!(!report.linked, "the existing link is reused");
    assert_eq!(report.command, Some(mac.layout.command_link()));
    assert!(!report.needs_attention());
    assert_eq!(
        std::fs::read_link(mac.layout.command_link()).unwrap(),
        mac.exe()
    );
    assert_eq!(plist_program(&mac.layout.agent_plist).unwrap(), mac.exe());
    assert_eq!(
        extract(&mac.layout.agent_plist, "AssociatedBundleIdentifiers.0"),
        AVEN_BUNDLE_ID
    );
    // launchd stops respawning the daemon once Aven.app is gone.
    assert_eq!(keep_alive(&mac.layout.agent_plist), path_state(&mac.exe()));
    assert_settings_kept(&mac.layout.agent_plist);
    assert!(!mac.layout.script_bundle().exists());
    assert!(!mac.layout.state_file.exists());
    assert_eq!(
        mac.system.calls(),
        vec![
            "stop".to_string(),
            "start".to_string(),
            format!("unregister {}", mac.layout.script_bundle().display()),
        ]
    );
    assert!(!needs_setup(&mac.layout));
    assert_eq!(mac.plan(), Plan::Ready);
    assert_eq!(
        *mac.system.background.lock().unwrap(),
        Some(BackgroundIntent::On)
    );
}

#[test]
fn a_disabled_daemon_is_moved_but_stays_off() {
    let mac = Mac::new();
    let old = mac.script_install();
    mac.agent(&old, false, true);

    let report = report(mac.switch().unwrap());
    assert_eq!(
        report.agent,
        Some(AgentReport {
            running: false,
            disabled: true
        })
    );
    assert_eq!(plist_program(&mac.layout.agent_plist).unwrap(), mac.exe());
    assert_settings_kept(&mac.layout.agent_plist);
    assert!(
        mac.system
            .calls()
            .iter()
            .all(|call| call != "stop" && call != "start")
    );
    assert_eq!(
        *mac.system.background.lock().unwrap(),
        Some(BackgroundIntent::Off)
    );
}

#[test]
fn a_stopped_but_enabled_daemon_keeps_the_background_on() {
    let mac = Mac::new();
    let old = mac.script_install();
    mac.agent(&old, false, false);

    report(mac.switch().unwrap());
    assert_eq!(
        *mac.system.background.lock().unwrap(),
        Some(BackgroundIntent::On)
    );
    assert!(mac.system.calls().iter().all(|call| call != "start"));
}

#[test]
fn a_switch_follows_the_program_in_path_state_and_keeps_a_custom_keep_alive() {
    let mac = Mac::new();
    let old = mac.script_install();
    mac.agent(&old, false, false);
    let plist = &mac.layout.agent_plist;
    let previous = path_state(&old).to_string();
    plutil(&["-replace", "KeepAlive", "-json", &previous], plist).unwrap();
    retarget_agent(plist, &mac.exe()).unwrap();
    assert_eq!(keep_alive(plist), path_state(&mac.exe()));

    let custom = r#"{"SuccessfulExit":false}"#;
    plutil(&["-replace", "KeepAlive", "-json", custom], plist).unwrap();
    retarget_agent(plist, &old).unwrap();
    assert_eq!(plist_program(plist).unwrap(), old);
    assert_eq!(
        keep_alive(plist),
        serde_json::json!({ "SuccessfulExit": false })
    );
}

#[test]
fn surveying_and_planning_change_nothing() {
    let mac = Mac::new();
    let old = mac.script_install();
    mac.formula_install();
    mac.agent(&old, true, false);
    let before = mac.tree();
    assert!(matches!(mac.plan(), Plan::Switch(_)));
    assert!(needs_setup(&mac.layout));
    assert_eq!(mac.tree(), before);
    assert!(mac.system.calls().is_empty());
}

#[test]
fn the_switch_knows_every_executable_of_the_terminal_installation() {
    let mac = Mac::new();
    let old = mac.script_install();
    let keg = mac.formula_install();
    mac.agent(&old, true, false);
    let bare = mac.root.join("brew/Cellar/aven/0.1.30/bin/aven");
    std::fs::create_dir_all(bare.parent().unwrap()).unwrap();
    std::fs::write(&bare, "#!/bin/sh\n").unwrap();
    let mut expected: Vec<PathBuf> = [old, keg, bare]
        .iter()
        .map(|path| path.canonicalize().unwrap())
        .collect();
    expected.sort();
    let mut found = mac.switch_plan().executables;
    found.sort();
    assert_eq!(found, expected);
}

#[test]
fn the_switch_waits_for_the_terminal_installations_processes() {
    let mac = Mac::new();
    let old = mac.script_install();
    mac.agent(&old, true, false);
    // A real executable, so the process is found by the path it runs.
    std::fs::copy("/bin/sleep", &old).unwrap();
    let mut tui = Command::new(&old).arg("30").spawn().unwrap();
    let plan = mac.switch_plan();
    let before = mac.tree();
    let running = (0..50)
        .find_map(|_| match switch(&mac.layout, &mac.system, &plan).unwrap() {
            Outcome::Waiting(running) => Some(running),
            _ => {
                std::thread::sleep(Duration::from_millis(20));
                None
            }
        })
        .unwrap_or_default();
    tui.kill().unwrap();
    tui.wait().unwrap();
    assert_eq!(
        running
            .iter()
            .map(|process| process.pid)
            .collect::<Vec<_>>(),
        vec![tui.id()]
    );
    assert_eq!(mac.tree(), before, "nothing changes while it runs");
    assert!(mac.system.calls().is_empty());

    let report = report(switch(&mac.layout, &mac.system, &plan).unwrap());
    assert!(report.switched);
    assert!(!mac.layout.script_bundle().exists());
}

#[test]
fn formula_waits_for_its_removal_and_resumes_after_a_restart() {
    let mac = Mac::new();
    let program = mac.formula_install();
    mac.agent(&program, true, false);

    let plan = mac.switch_plan();
    assert!(plan.formula && !plan.script && plan.repoint.is_empty());
    assert_eq!(
        mac.switch().unwrap(),
        Outcome::AwaitFormula {
            cask: false,
            sync_paused: true
        }
    );
    // The daemon stops before its executable can go, and moves to Desktop.
    assert_eq!(mac.system.calls(), vec!["stop"]);
    assert_eq!(plist_program(&mac.layout.agent_plist).unwrap(), mac.exe());
    assert_settings_kept(&mac.layout.agent_plist);
    // Setup never touches Homebrew's files.
    assert!(mac.root.join("brew/bin/aven").is_symlink());
    assert!(mac.root.join("brew/Cellar/aven").exists());
    assert!(mac.layout.state_file.exists());

    // Checking again before the user ran the command still waits.
    let plan = mac.switch_plan();
    assert!(plan.resuming && plan.agent.is_none());
    assert_eq!(
        switch(&mac.layout, &mac.system, &plan).unwrap(),
        Outcome::AwaitFormula {
            cask: false,
            sync_paused: true
        }
    );

    mac.uninstall_formula();
    // A relaunch finds the record and resumes without asking again.
    assert!(needs_setup(&mac.layout));
    let plan = mac.switch_plan();
    assert!(plan.resuming && !plan.formula);
    let report = report(switch(&mac.layout, &mac.system, &plan).unwrap());
    assert!(report.switched && report.linked);
    assert_eq!(report.command, Some(mac.layout.command_link()));
    assert_eq!(
        std::fs::read_link(mac.layout.command_link()).unwrap(),
        mac.exe()
    );
    assert_eq!(mac.system.calls(), vec!["stop", "start"]);
    assert!(mac.system.agent().unwrap().loaded);
    assert!(!mac.layout.state_file.exists());
    assert!(!needs_setup(&mac.layout));
}

#[test]
fn a_cask_install_keeps_homebrews_link_and_gets_no_second_one() {
    let mac = Mac::new();
    let program = mac.formula_install();
    mac.agent(&program, false, false);
    std::fs::create_dir_all(mac.root.join("brew/Caskroom/aven/0.2.0")).unwrap();

    assert_eq!(
        mac.switch().unwrap(),
        Outcome::AwaitFormula {
            cask: true,
            sync_paused: false
        }
    );
    mac.uninstall_formula();
    let report = report(mac.switch().unwrap());
    assert!(report.cask_unlinked && report.needs_attention());
    assert!(!report.linked);
    assert!(mac.layout.command_link().symlink_metadata().is_err());

    // Once Homebrew links the cask's command, nothing is left to do.
    symlink(mac.exe(), mac.root.join("brew/bin/aven")).unwrap();
    assert!(!needs_setup(&mac.layout));
    let survey = mac.survey();
    assert_eq!(survey.commands[0].owner, Owner::Cask);
    assert_eq!(mac.plan(), Plan::Ready);
    let report = complete(&mac.layout, &mac.system).unwrap();
    assert!(!report.linked && !report.cask_unlinked && !report.needs_attention());
}

#[test]
fn an_older_bare_binary_from_the_install_script_becomes_a_link() {
    let mac = Mac::new();
    let bare = mac.layout.command_link();
    std::fs::write(&bare, "binary").unwrap();
    mac.system.sign(&bare);
    assert_eq!(mac.survey().commands[0].owner, Owner::Script);

    let plan = mac.switch_plan();
    assert_eq!(plan.repoint, vec![bare.clone()]);
    assert_eq!(plan.retire, None);
    let report = report(mac.switch().unwrap());
    assert_eq!(report.command, Some(bare.clone()));
    assert_eq!(std::fs::read_link(&bare).unwrap(), mac.exe());
}

#[test]
fn an_aven_build_in_an_unknown_place_stops_setup() {
    let mac = Mac::new();
    let elsewhere = mac.root.join("opt/Aven.app");
    bundle(&elsewhere, AVEN_BUNDLE_ID);
    mac.system.sign(&elsewhere.join(BUNDLE_EXECUTABLE));
    symlink(elsewhere.join(BUNDLE_EXECUTABLE), mac.layout.command_link()).unwrap();
    let before = mac.tree();
    assert_eq!(
        mac.plan(),
        Plan::Blocked(Blocked::UnknownAven {
            paths: vec![mac.layout.command_link()]
        })
    );
    assert_eq!(mac.tree(), before);
}

#[test]
fn a_daemon_running_an_unrecognized_program_stops_setup() {
    let mac = Mac::new();
    let other = mac.root.join("dev/aven");
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::fs::write(&other, "dev build").unwrap();
    mac.agent(&other, true, false);
    assert_eq!(
        mac.plan(),
        Plan::Blocked(Blocked::UnknownAgent {
            plist: mac.layout.agent_plist.clone(),
            program: Some(other),
        })
    );
}

#[test]
fn other_programs_named_aven_are_reported_and_left_alone() {
    let mac = Mac::new();
    let other = mac.root.join("brew/bin/aven");
    std::fs::write(&other, "unrelated").unwrap();
    let mut layout = mac.layout.clone();
    // The unrelated one comes first on PATH.
    layout.path = Some(vec![
        mac.root.join("brew/bin"),
        mac.layout.home.join(".local/bin"),
    ]);
    assert!(needs_setup(&layout));
    assert_eq!(
        plan(&layout, &survey(&layout, &mac.system).unwrap()),
        Plan::Ready
    );

    let report = complete(&layout, &mac.system).unwrap();
    assert!(report.linked && !report.switched);
    assert_eq!(report.shadowed_by, vec![other.clone()]);
    assert!(report.needs_attention());
    assert_eq!(std::fs::read_to_string(&other).unwrap(), "unrelated");
}

#[test]
fn a_fresh_install_links_the_command_and_explains_a_missing_path_entry() {
    let mac = Mac::new();
    let mut layout = mac.layout.clone();
    layout.path = Some(vec!["/usr/bin".into()]);
    assert!(needs_setup(&layout));
    assert_eq!(
        plan(&layout, &survey(&layout, &mac.system).unwrap()),
        Plan::Ready
    );
    let report = complete(&layout, &mac.system).unwrap();
    assert!(report.linked);
    assert_eq!(report.add_to_path, Some(mac.layout.home.join(".local/bin")));
    assert!(!needs_setup(&layout));
    assert!(mac.system.calls().is_empty());
}

#[test]
fn a_copy_outside_applications_changes_nothing() {
    let mac = Mac::new();
    let mut layout = mac.layout.clone();
    let downloads = mac.root.join("Downloads/Aven.app");
    bundle(&downloads, AVEN_BUNDLE_ID);
    layout.running = downloads.join(BUNDLE_EXECUTABLE);
    mac.script_install();
    let before = mac.tree();
    assert!(needs_setup(&layout));
    assert_eq!(
        plan(&layout, &survey(&layout, &mac.system).unwrap()),
        Plan::MoveToApplications {
            running: layout.running.clone()
        }
    );
    assert_eq!(mac.tree(), before);

    // With nothing installed and no app in Applications, it just opens.
    std::fs::remove_dir_all(mac.root.join("Applications")).unwrap();
    std::fs::remove_dir_all(mac.layout.script_bundle()).unwrap();
    assert!(!needs_setup(&layout));
}

#[test]
fn an_unreadable_record_stops_rather_than_guessing() {
    let mac = Mac::new();
    std::fs::create_dir_all(mac.layout.state_file.parent().unwrap()).unwrap();
    std::fs::write(
        &mac.layout.state_file,
        r#"{"version":9,"restart_agent":true}"#,
    )
    .unwrap();
    assert!(needs_setup(&mac.layout));
    assert!(matches!(
        mac.plan(),
        Plan::Blocked(Blocked::UnknownState { .. })
    ));
}

#[test]
fn unwritable_paths_stop_before_any_change() {
    let mac = Mac::new();
    let old = mac.script_install();
    mac.agent(&old, true, false);
    let bin = mac.layout.home.join(".local/bin");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o555)).unwrap();
    let before = mac.tree();
    let plan = mac.plan();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        plan,
        Plan::Blocked(Blocked::Unwritable {
            paths: vec![mac.layout.command_link()]
        })
    );
    assert_eq!(mac.tree(), before);
    assert!(mac.system.calls().is_empty());
}

#[test]
fn an_interrupted_switch_restarts_the_daemon_it_stopped() {
    let mac = Mac::new();
    let old = mac.script_install();
    mac.agent(&old, true, false);
    *mac.system.fail_start.lock().unwrap() = true;
    assert!(mac.switch().is_err());
    // The old bundle stays until everything is verified.
    assert!(mac.layout.script_bundle().exists());
    assert!(mac.layout.state_file.exists());

    let plan = mac.switch_plan();
    assert!(plan.resuming && plan.agent.is_none() && plan.repoint.is_empty());
    let report = report(switch(&mac.layout, &mac.system, &plan).unwrap());
    assert!(report.switched);
    assert!(mac.system.agent().unwrap().loaded);
    assert!(!mac.layout.script_bundle().exists());
}

#[test]
fn a_bundle_at_the_script_location_that_is_not_aven_is_kept() {
    let mac = Mac::new();
    bundle(&mac.layout.script_bundle(), "com.example.other");
    let plan = mac.switch_plan();
    assert_eq!(plan.retire, None);
    let report = report(mac.switch().unwrap());
    assert_eq!(report.kept_bundle, Some(mac.layout.script_bundle()));
    assert!(mac.layout.script_bundle().exists());
}

#[test]
fn reads_path_after_shell_startup_noise() {
    let output = format!("motd\n{PATH_MARKER}/a/bin:/b bin::/c{PATH_MARKER}\n");
    assert_eq!(
        parse_shell_path(&output),
        Some(vec![
            PathBuf::from("/a/bin"),
            PathBuf::from("/b bin"),
            PathBuf::from("/c")
        ])
    );
    assert_eq!(parse_shell_path("no marker"), None);
}
