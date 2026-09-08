mod common;

use std::net::TcpListener;
use std::path::Path;
use std::process::Output;

use aven_core::api::PairingInvitation;
use common::{TestEnv, fail, ok};

const CONFIGURED_SERVER: &str = "https://configured.example.test:8443/aven";
const EXPLICIT_SERVER: &str = "https://explicit.example.test:8443/SENSITIVE_PAIRING_SERVER_PATH";
const PARENT_SERVER: &str = "https://parent.example.test:8443/aven";
const ENV_SERVER: &str = "https://environment.example.test:8443/aven";
const TOKEN: &str = "pairing-token-fixture-0123456789";
const SENSITIVE_PATH: &str = "SENSITIVE_PAIRING_SERVER_PATH";

fn configured(env: &TestEnv, db: &Path, server: Option<&str>, token: Option<&str>) {
    let server = server
        .map(|server| format!("  server_url: \"{server}\"\n"))
        .unwrap_or_default();
    let token = token
        .map(|token| format!("  auth_token: \"{token}\"\n"))
        .unwrap_or_default();
    env.write_config(&format!(
        "local:\n  db_path: \"{}\"\n\nsync:\n  enabled: true\n{server}{token}",
        db.display()
    ));
}

fn command_log(env: &TestEnv) -> String {
    std::fs::read_to_string(env.state_dir().join("aven").join("aven.log")).unwrap_or_default()
}

fn expected_uri(server: &str, token: &str) -> String {
    PairingInvitation::new(server.to_string(), token.to_string())
        .unwrap()
        .encode()
        .unwrap()
}

fn server_identity(server: &str) -> String {
    url::Url::parse(server)
        .unwrap()
        .origin()
        .ascii_serialization()
}

fn assert_private(env: &TestEnv, output: &Output, token: &str, invitation_uri: Option<&str>) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let log = command_log(env);

    for (surface, text) in [
        ("stdout", stdout.as_ref()),
        ("stderr", stderr.as_ref()),
        ("log", log.as_str()),
    ] {
        assert!(!text.contains(token), "{surface} exposed token");
        assert!(!text.contains(SENSITIVE_PATH), "{surface} exposed URL path");
        assert!(
            !text.contains("aven://pair/"),
            "{surface} exposed an invitation URI"
        );
        assert!(
            !text.contains('\u{1b}'),
            "{surface} emitted ANSI on redirected output"
        );
        if let Some(uri) = invitation_uri {
            let payload = uri.rsplit('/').next().unwrap();
            assert!(!text.contains(uri), "{surface} exposed complete URI");
            assert!(!text.contains(payload), "{surface} exposed URI payload");
        }
    }
}

fn pairing_output(env: &TestEnv, args: &[&str], envs: &[(&str, &str)]) -> Output {
    env.aven_config_env(args, envs.iter().copied())
}

#[test]
fn sync_pair_is_standalone_private_and_uses_the_pair_local_server() {
    let env = TestEnv::new();
    let configured_db = env.db("configured-must-not-exist.sqlite");
    let explicit_db = env.db("explicit-must-not-exist.sqlite");
    configured(&env, &configured_db, Some(CONFIGURED_SERVER), Some(TOKEN));
    let config_before = std::fs::read(env.config_file()).unwrap();
    let uri = expected_uri(EXPLICIT_SERVER, TOKEN);

    let output = pairing_output(
        &env,
        &[
            "--db",
            explicit_db.to_str().unwrap(),
            "sync",
            "--server",
            PARENT_SERVER,
            "pair",
            "--server",
            EXPLICIT_SERVER,
        ],
        &[],
    );
    assert_private(&env, &output, TOKEN, Some(&uri));
    let stdout = ok(output);

    assert!(
        stdout.contains(&server_identity(EXPLICIT_SERVER)),
        "{stdout}"
    );
    assert!(
        !stdout.contains(&server_identity(PARENT_SERVER)),
        "{stdout}"
    );
    assert!(
        !stdout.contains(&server_identity(CONFIGURED_SERVER)),
        "{stdout}"
    );
    assert!(stdout.contains("Scan this code during Aven iOS onboarding."));
    assert!(!configured_db.exists());
    assert!(!explicit_db.exists());
    assert_eq!(std::fs::read(env.config_file()).unwrap(), config_before);
}

#[test]
fn sync_pair_uses_parent_environment_then_config_server_precedence() {
    let env = TestEnv::new();
    let db = env.db("must-not-exist.sqlite");
    configured(&env, &db, Some(CONFIGURED_SERVER), Some(TOKEN));

    for (args, envs, expected) in [
        (
            vec!["sync", "--server", PARENT_SERVER, "pair"],
            vec![("AVEN_SYNC_SERVER", ENV_SERVER)],
            PARENT_SERVER,
        ),
        (
            vec!["sync", "pair"],
            vec![("AVEN_SYNC_SERVER", ENV_SERVER)],
            ENV_SERVER,
        ),
        (vec!["sync", "pair"], vec![], CONFIGURED_SERVER),
    ] {
        let output = pairing_output(&env, &args, &envs);
        assert_private(&env, &output, TOKEN, Some(&expected_uri(expected, TOKEN)));
        let stdout = ok(output);
        assert!(stdout.contains(&server_identity(expected)), "{stdout}");
    }
    assert!(!db.exists());
}

#[test]
fn sync_pair_requires_selected_server_and_trimmed_token() {
    let env = TestEnv::new();
    let db = env.db("must-not-exist.sqlite");
    configured(&env, &db, None, Some(TOKEN));

    let missing_server = pairing_output(&env, &["sync", "pair"], &[]);
    assert_private(&env, &missing_server, TOKEN, None);
    assert!(fail(missing_server).contains("pairing-server-required"));

    configured(&env, &db, Some(CONFIGURED_SERVER), Some(TOKEN));
    let blank_environment = pairing_output(&env, &["sync", "pair"], &[("AVEN_SYNC_SERVER", "")]);
    assert_private(&env, &blank_environment, TOKEN, None);
    assert!(fail(blank_environment).contains("pairing-server-required"));

    for token in [None, Some(""), Some("   ")] {
        configured(&env, &db, Some(CONFIGURED_SERVER), token);
        let missing_token = pairing_output(&env, &["sync", "pair"], &[]);
        assert_private(&env, &missing_token, TOKEN, None);
        assert!(fail(missing_token).contains("pairing-auth-token-required"));
    }
    assert!(!db.exists());
}

#[test]
fn sync_pair_reports_invalid_loopback_and_capacity_errors_privately() {
    let env = TestEnv::new();
    let db = env.db("must-not-exist.sqlite");
    configured(&env, &db, Some(CONFIGURED_SERVER), Some(TOKEN));

    for (server, code) in [
        (
            &format!("https://sync.example.test/aven?token={TOKEN}"),
            "pairing-server-invalid",
        ),
        (
            &"http://127.0.0.1:3746".to_string(),
            "pairing-server-not-reachable",
        ),
    ] {
        let output = pairing_output(&env, &["sync", "pair", "--server", server], &[]);
        assert_private(&env, &output, TOKEN, None);
        assert!(fail(output).contains(code));
    }

    let large_token = "t".repeat(3000);
    configured(&env, &db, Some(CONFIGURED_SERVER), Some(&large_token));
    let uri = expected_uri(CONFIGURED_SERVER, &large_token);
    let output = pairing_output(&env, &["sync", "pair"], &[]);
    assert_private(&env, &output, &large_token, Some(&uri));
    assert!(fail(output).contains("pairing-qr-too-large"));
    assert!(!db.exists());
}

#[test]
fn sync_pair_performs_no_network_request() {
    let env = TestEnv::new();
    let db = env.db("must-not-exist.sqlite");
    let listener = TcpListener::bind("0.0.0.0:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = format!("http://0.0.0.0:{}", listener.local_addr().unwrap().port());
    configured(&env, &db, Some(&server), Some(TOKEN));

    let output = pairing_output(&env, &["sync", "pair"], &[]);
    assert_private(&env, &output, TOKEN, Some(&expected_uri(&server, TOKEN)));
    ok(output);

    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    assert!(!db.exists());
}

#[test]
fn sync_pair_is_available_while_synchronization_is_disabled() {
    let env = TestEnv::new();
    let db = env.db("must-not-exist.sqlite");
    configured(&env, &db, Some(CONFIGURED_SERVER), Some(TOKEN));
    let text = std::fs::read_to_string(env.config_file())
        .unwrap()
        .replace("enabled: true", "enabled: false");
    env.write_config(&text);

    let output = pairing_output(&env, &["sync", "pair"], &[("AVEN_SYNC_DISABLED", "1")]);
    assert_private(
        &env,
        &output,
        TOKEN,
        Some(&expected_uri(CONFIGURED_SERVER, TOKEN)),
    );
    let stdout = ok(output);

    assert!(
        stdout.contains(&server_identity(CONFIGURED_SERVER)),
        "{stdout}"
    );
    assert!(!db.exists());
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn clipboard_mock(env: &TestEnv, fail_copy: bool) -> (String, String) {
    use std::os::unix::fs::PermissionsExt;

    let directory = env.state_dir().join("clipboard-bin");
    std::fs::create_dir_all(&directory).unwrap();
    let destination = directory.join("copied");
    let script = format!(
        "#!/usr/bin/env bash\nset -euo pipefail\numask 077\n/bin/cat > \"$PAIRING_TEST_CLIPBOARD\"\n/bin/cat \"$PAIRING_TEST_CLIPBOARD\"\n/bin/cat \"$PAIRING_TEST_CLIPBOARD\" >&2\nexit {}\n",
        if fail_copy { 1 } else { 0 }
    );
    for program in ["pbcopy", "wl-copy", "xclip"] {
        let path = directory.join(program);
        std::fs::write(&path, &script).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    (
        format!("{}:/usr/bin:/bin", directory.display()),
        destination.display().to_string(),
    )
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn sync_pair_copy_delivers_only_to_clipboard_without_qr_capacity_limit() {
    for token in [TOKEN.to_string(), "t".repeat(3000)] {
        let env = TestEnv::new();
        let db = env.db("must-not-exist.sqlite");
        configured(&env, &db, Some(CONFIGURED_SERVER), Some(&token));
        let config_before = std::fs::read(env.config_file()).unwrap();
        let (path, destination) = clipboard_mock(&env, false);
        let uri = expected_uri(EXPLICIT_SERVER, &token);
        let output = pairing_output(
            &env,
            &["sync", "--server", EXPLICIT_SERVER, "pair", "--copy"],
            &[
                ("PATH", &path),
                ("PAIRING_TEST_CLIPBOARD", &destination),
                ("SSH_CONNECTION", ""),
                ("SSH_CLIENT", ""),
                ("SSH_TTY", ""),
            ],
        );
        assert_private(&env, &output, &token, Some(&uri));
        let stdout = ok(output);
        assert!(
            stdout.contains("Pairing invitation copied for https://explicit.example.test:8443.")
        );
        assert!(!stdout.contains("Scan this code"));
        assert!(!stdout.contains('█'));
        let copied = std::fs::read_to_string(destination).unwrap();
        assert!(
            copied == uri,
            "clipboard contents differ from the encoded invitation"
        );
        assert!(PairingInvitation::decode(&copied).is_ok());
        assert!(!db.exists());
        assert!(std::fs::read(env.config_file()).unwrap() == config_before);
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn sync_pair_copy_failure_suppresses_helper_output_and_reports_no_success() {
    let env = TestEnv::new();
    let db = env.db("must-not-exist.sqlite");
    configured(&env, &db, Some(EXPLICIT_SERVER), Some(TOKEN));
    let (path, destination) = clipboard_mock(&env, true);
    let output = pairing_output(
        &env,
        &["sync", "pair", "--copy"],
        &[
            ("PATH", &path),
            ("PAIRING_TEST_CLIPBOARD", &destination),
            ("SSH_CONNECTION", ""),
            ("SSH_CLIENT", ""),
            ("SSH_TTY", ""),
        ],
    );
    assert_private(
        &env,
        &output,
        TOKEN,
        Some(&expected_uri(EXPLICIT_SERVER, TOKEN)),
    );
    assert!(output.stdout.is_empty());
    assert!(fail(output).contains("pairing-copy-unavailable"));
    assert!(!db.exists());
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn sync_pair_copy_rejects_ssh_and_invalid_invitation_before_clipboard_access() {
    let env = TestEnv::new();
    let db = env.db("must-not-exist.sqlite");
    configured(&env, &db, Some(CONFIGURED_SERVER), Some(TOKEN));
    let (path, destination) = clipboard_mock(&env, false);
    for (server, ssh, code) in [
        (CONFIGURED_SERVER, "remote", "pairing-copy-remote-session"),
        ("http://127.0.0.1:3746", "", "pairing-server-not-reachable"),
    ] {
        let output = pairing_output(
            &env,
            &["sync", "pair", "--copy", "--server", server],
            &[
                ("PATH", &path),
                ("PAIRING_TEST_CLIPBOARD", &destination),
                ("SSH_CONNECTION", ssh),
                ("SSH_CLIENT", ""),
                ("SSH_TTY", ""),
            ],
        );
        assert_private(&env, &output, TOKEN, None);
        assert!(fail(output).contains(code));
        assert!(!Path::new(&destination).exists());
    }
    assert!(!db.exists());
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[test]
fn sync_pair_copy_missing_clipboard_returns_safe_error() {
    let env = TestEnv::new();
    let db = env.db("must-not-exist.sqlite");
    configured(&env, &db, Some(EXPLICIT_SERVER), Some(TOKEN));
    let missing = env.state_dir().join("no-clipboard-programs");
    let output = pairing_output(
        &env,
        &["sync", "pair", "--copy"],
        &[
            ("PATH", missing.to_str().unwrap()),
            ("SSH_CONNECTION", ""),
            ("SSH_CLIENT", ""),
            ("SSH_TTY", ""),
        ],
    );
    assert_private(&env, &output, TOKEN, None);
    assert!(output.stdout.is_empty());
    assert!(fail(output).contains("pairing-copy-unavailable"));
    assert!(!db.exists());
}
