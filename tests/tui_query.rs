mod common;

use common::{TestEnv, ok};

#[test]
fn tui_command_is_available() {
    let env = TestEnv::new();
    let db = env.db("tui-help.sqlite");
    let help = ok(env.atm(&db, ["tui", "--help"]));
    assert!(help.contains("Usage: atm"));
}
