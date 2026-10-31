use anyhow::Result;

use crate::config::AppConfig;

fn load_config(text: &str) -> Result<AppConfig> {
    super::install_command_rules();
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config.yaml");
    std::fs::write(&path, text)?;
    AppConfig::load_from_path(&path)
}

#[test]
fn custom_tui_commands_reject_invalid_names_and_collisions() {
    for yaml in [
        "tui:\n  commands:\n    - name: ':dispatch'\n      description: Dispatch\n      program: dispatch\n",
        "tui:\n  commands:\n    - name: quit\n      description: Dispatch\n      program: dispatch\n",
        "tui:\n  commands:\n    - name: dispatch\n      aliases: [dispatch]\n      description: Dispatch\n      program: dispatch\n",
        "tui:\n  commands:\n    - name: dispatch\n      aliases: [same]\n      description: Dispatch\n      program: dispatch\n    - name: other\n      aliases: [same]\n      description: Other\n      program: other\n",
    ] {
        assert!(load_config(yaml).is_err(), "accepted {yaml}");
    }
}

#[test]
fn custom_tui_commands_validate_contextual_keybindings() {
    let inherited = load_config(
            "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      keys: [z d]\n",
        )
        .unwrap();
    assert_eq!(inherited.tui.commands[0].detail_keys, None);

    let detail_disabled = load_config(
            "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      keys: [z d]\n      detail_keys: []\n",
        )
        .unwrap();
    assert_eq!(detail_disabled.tui.commands[0].detail_keys, Some(vec![]));

    for yaml in [
        "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      keys: [q]\n",
        "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      keys: ['']\n",
        "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      keys: [Esc]\n",
        "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      keys: [z unknown]\n",
        "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      keys: [z d a b c]\n",
        "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      keys: [z d]\n      detail_keys: [s]\n",
        "tui:\n  commands:\n    - name: dispatch\n      description: Dispatch\n      program: dispatch\n      keys: [z d]\n    - name: other\n      description: Other\n      program: other\n      keys: [z d]\n",
    ] {
        assert!(load_config(yaml).is_err(), "accepted {yaml}");
    }
}
