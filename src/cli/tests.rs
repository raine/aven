use super::*;
use clap::{CommandFactory, Parser};
use std::collections::BTreeSet;

#[test]
fn sync_existing_forms_parse() {
    for args in [
        vec!["aven", "sync"],
        vec!["aven", "sync", "--json"],
        vec!["aven", "sync", "status"],
        vec!["aven", "sync", "setup", "--yes"],
        vec!["aven", "sync", "invite"],
        vec!["aven", "sync", "join"],
    ] {
        let cli = Cli::try_parse_from(args).unwrap();
        assert!(matches!(cli.command, Some(Commands::Sync(_))));
    }
}

#[test]
fn public_server_bind_flag_parses() {
    let cli = Cli::try_parse_from([
        "aven",
        "server",
        "--unsafe-public-bind",
        "--data",
        "server.sqlite",
    ])
    .unwrap();
    assert!(matches!(cli.command, Some(Commands::Server(_))));
}

#[test]
fn retired_sync_forms_are_rejected() {
    for args in [
        vec!["aven", "sync", "--server", "https://sync.example.test"],
        vec!["aven", "sync", "pair"],
        vec!["aven", "server", "--encrypted", "--data", "server.sqlite"],
        vec![
            "aven",
            "server",
            "--allow-non-loopback",
            "--data",
            "server.sqlite",
        ],
    ] {
        assert!(Cli::try_parse_from(&args).is_err(), "{args:?}");
    }
}

#[test]
fn top_level_help_sections_match_visible_commands() {
    let command = Cli::command();
    let visible = command
        .get_subcommands()
        .filter(|subcommand| !subcommand.is_hide_set())
        .map(clap::Command::get_name)
        .collect::<BTreeSet<_>>();
    let listed_entries = HELP_SECTIONS
        .iter()
        .flat_map(|section| section.commands.iter().copied())
        .collect::<Vec<_>>();
    let mut seen = BTreeSet::new();
    let duplicates = listed_entries
        .iter()
        .copied()
        .filter(|name| !seen.insert(*name))
        .collect::<Vec<_>>();
    let listed = listed_entries.into_iter().collect::<BTreeSet<_>>();
    let missing = visible.difference(&listed).copied().collect::<Vec<_>>();
    let invalid = listed.difference(&visible).copied().collect::<Vec<_>>();

    assert!(
        missing.is_empty() && invalid.is_empty() && duplicates.is_empty(),
        "top-level HELP_SECTIONS drifted\nmissing visible commands: {}\nlisted names without visible commands: {}\nduplicate commands: {}",
        missing.join(", "),
        invalid.join(", "),
        duplicates.join(", ")
    );
}

#[test]
fn visible_command_tree_has_help_descriptions() {
    fn collect_missing(command: &clap::Command, path: &str, missing: &mut Vec<String>) {
        for argument in command
            .get_arguments()
            .filter(|argument| !argument.is_hide_set())
        {
            let description = argument
                .get_long_help()
                .or_else(|| argument.get_help())
                .map(|help| help.to_string())
                .unwrap_or_default();
            if description.trim().is_empty() {
                missing.push(format!("{path} <{}>", argument.get_id()));
            }
        }

        for subcommand in command
            .get_subcommands()
            .filter(|subcommand| !subcommand.is_hide_set())
        {
            let subcommand_path = format!("{path} {}", subcommand.get_name());
            let description = subcommand
                .get_long_about()
                .or_else(|| subcommand.get_about())
                .map(|about| about.to_string())
                .unwrap_or_default();
            if description.trim().is_empty() {
                missing.push(subcommand_path.clone());
            }
            collect_missing(subcommand, &subcommand_path, missing);
        }
    }

    let mut command = Cli::command();
    command.build();
    let mut missing = Vec::new();
    collect_missing(&command, "aven", &mut missing);

    assert!(
        missing.is_empty(),
        "visible commands or arguments missing help descriptions:\n{}",
        missing.join("\n")
    );
}

#[test]
fn complex_commands_keep_examples_and_safety_guidance() {
    fn long_help(path: &[&str]) -> String {
        let mut command = Cli::command();
        for name in path {
            command = command
                .find_subcommand(name)
                .unwrap_or_else(|| panic!("missing command path component {name}"))
                .clone();
        }
        let mut output = Vec::new();
        command.write_long_help(&mut output).unwrap();
        String::from_utf8(output).unwrap()
    }

    let expectations = [
        (&["add"][..], "weekly on mon,wed,fri"),
        (&["add"][..], "--repeat excludes terminal values"),
        (&["list"][..], "nondeleted, open tasks"),
        (&["edit"][..], "--due accepts natural date expressions"),
        (&["note"][..], "exactly one text source"),
        (&["bulk-update"][..], "at least one\nupdate option"),
        (&["recur"][..], "linked occurrence task ref"),
        (&["recur", "edit"][..], "future occurrences"),
        (&["text", "get"][..], "when --output is omitted"),
        (&["text", "set"][..], "hash guard"),
        (&["conflict", "resolve"][..], "--use takes precedence"),
        (&["config", "set"][..], "positive integer"),
        (
            &["backup", "restore"][..],
            "attachment objects available on",
        ),
        (&["export"][..], "no attachment bytes"),
        (&["import"][..], "requires --yes"),
        (&["prime"][..], "live project work"),
        (&["skill"][..], "without live task context"),
        (&["skill", "install"][..], "repeat for multiple"),
        (&["sync"][..], "end-to-end encrypted"),
        (&["server"][..], "binds only"),
        (&["server", "setup"][..], "aven server --data PATH"),
    ];

    for (path, expected) in expectations {
        let help = long_help(path);
        assert!(
            help.contains(expected),
            "{} help omitted {expected:?}",
            path.join(" ")
        );
    }
}

#[test]
fn omitted_command_is_accepted_for_default_tui_launch() {
    let parsed = Cli::try_parse_from(["aven"]).unwrap();
    assert!(parsed.command.is_none());

    let parsed = Cli::try_parse_from(["aven", "--db", "local.db"]).unwrap();
    assert!(parsed.command.is_none());
}

#[test]
fn application_update_and_task_edit_are_distinct_commands() {
    let update = Cli::try_parse_from(["aven", "update"]).unwrap();
    assert!(matches!(
        update.command,
        Some(Commands::Update(SelfUpdateArgs { yes: false }))
    ));

    let edit = Cli::try_parse_from(["aven", "edit", "APP-1234", "--status", "active"]).unwrap();
    assert!(matches!(edit.command, Some(Commands::Edit(_))));
    assert!(Cli::try_parse_from(["aven", "edit"]).is_err());
    assert!(Cli::try_parse_from(["aven", "update", "APP-1234"]).is_err());
    assert!(
        Cli::try_parse_from(["aven", "update", "--yes", "--allow-sync-incompatibility"]).is_err()
    );
}

#[test]
fn conflict_resolve_parses_use_variant() {
    let parsed = Cli::try_parse_from([
        "aven", "conflict", "resolve", "APP-1234", "title", "--use", "remote",
    ])
    .unwrap();
    let Some(Commands::Conflict(ConflictCommand {
        command:
            ConflictSubcommand::Resolve {
                task_ref,
                field,
                use_variant,
                value,
                value_file,
                value_stdin,
            },
    })) = parsed.command
    else {
        panic!("expected conflict resolve command");
    };
    assert_eq!(task_ref, "APP-1234");
    assert_eq!(field, "title");
    assert_eq!(use_variant.as_deref(), Some("remote"));
    assert_eq!(value, None);
    assert_eq!(value_file, None);
    assert!(!value_stdin);

    assert!(
        Cli::try_parse_from([
            "aven",
            "conflict",
            "resolve",
            "APP-1234",
            "title",
            "--use-variant",
            "remote",
        ])
        .is_err()
    );
    assert!(
        Cli::try_parse_from(["aven", "conflict", "resolve", "APP-1234", "title", "--use",])
            .is_err()
    );
}

#[test]
fn label_and_project_lists_parse_command_specific_arguments() {
    let label = Cli::try_parse_from([
        "aven", "label", "list", "--search", "bug", "--limit", "3", "--json",
    ])
    .unwrap();
    let Some(Commands::Label(LabelCommand {
        command: LabelSubcommand::List(label_args),
    })) = label.command
    else {
        panic!("expected label list command");
    };
    assert_eq!(label_args.search.as_deref(), Some("bug"));
    assert_eq!(label_args.limit, Some(3));
    assert!(label_args.json);

    let project = Cli::try_parse_from([
        "aven", "project", "list", "--search", "agent", "--limit", "5", "--json",
    ])
    .unwrap();
    let Some(Commands::Project(ProjectCommand {
        command: ProjectSubcommand::List(project_args),
    })) = project.command
    else {
        panic!("expected project list command");
    };
    assert_eq!(project_args.search.as_deref(), Some("agent"));
    assert_eq!(project_args.limit, Some(5));
    assert!(project_args.json);
}

#[test]
fn result_limits_preserve_defaults_and_validate_explicit_values() {
    let search = Cli::try_parse_from(["aven", "search", "task"]).unwrap();
    let Some(Commands::Search(search_args)) = search.command else {
        panic!("expected search command");
    };
    assert_eq!(search_args.limit, 50);

    let history = Cli::try_parse_from(["aven", "recur", "history", "RCR-1234"]).unwrap();
    let Some(Commands::Recur(RecurCommand {
        command: RecurSubcommand::History(history_args),
    })) = history.command
    else {
        panic!("expected recurrence history command");
    };
    assert_eq!(history_args.limit, 100);

    let list = Cli::try_parse_from(["aven", "list"]).unwrap();
    let Some(Commands::List(list_args)) = list.command else {
        panic!("expected list command");
    };
    assert_eq!(list_args.limit, None);

    for arguments in [
        vec!["aven", "list", "--limit", "0"],
        vec!["aven", "search", "task", "--limit", "0"],
        vec!["aven", "recur", "history", "RCR-1234", "--limit", "0"],
        vec!["aven", "prime", "--limit", "0"],
        vec!["aven", "label", "list", "--limit", "0"],
        vec!["aven", "project", "list", "--limit", "0"],
        vec!["aven", "conflict", "list", "--limit", "0"],
    ] {
        let error = Cli::try_parse_from(arguments).err().unwrap();
        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
    }

    assert!(Cli::try_parse_from(["aven", "search", "task", "--limit", "1"]).is_ok());
    assert!(
        Cli::try_parse_from(["aven", "recur", "history", "RCR-1234", "--limit", "500",]).is_ok()
    );
    for limit in ["501", "900"] {
        let error = Cli::try_parse_from(["aven", "recur", "history", "RCR-1234", "--limit", limit])
            .err()
            .unwrap();
        assert_eq!(error.kind(), clap::error::ErrorKind::ValueValidation);
    }
}

#[test]
fn internal_workspace_ids_are_validated_by_clap() {
    let parsed = Cli::try_parse_from([
        "aven",
        "internal",
        "natural-add",
        "--workspace-id",
        "0123456789ABCDEF",
        "--input",
        "task",
    ])
    .unwrap();
    let Some(Commands::Internal(InternalCommand {
        command: InternalSubcommand::NaturalAdd(args),
    })) = parsed.command
    else {
        panic!("expected internal natural-add command");
    };
    assert_eq!(args.workspace_id.as_str(), "0123456789ABCDEF");

    assert!(
        Cli::try_parse_from([
            "aven",
            "internal",
            "natural-add",
            "--workspace-id",
            "invalid",
            "--input",
            "task",
        ])
        .is_err()
    );
}

#[test]
fn tui_launch_arguments_compose_browse_state() {
    let parsed = Cli::try_parse_from([
        "aven",
        "tui",
        "--project",
        "app",
        "--view",
        "all",
        "--layout",
        "columns",
        "--label",
        "bug",
        "--priority",
        "urgent",
        "--add-task",
        "--natural",
    ])
    .unwrap();
    let Some(Commands::Tui(args)) = parsed.command else {
        panic!("expected tui command");
    };
    assert_eq!(args.project.as_deref(), Some("app"));
    assert_eq!(args.view, Some(TuiViewArg::All));
    assert_eq!(args.layout, Some(TuiLayoutArg::Columns));
    assert_eq!(args.label.as_deref(), Some("bug"));
    assert_eq!(args.priority, Some(TuiPriorityArg::Urgent));
    assert!(args.add_task);
    assert!(args.natural);
}

#[test]
fn tui_launch_parses_all_typed_values() {
    for view in [
        "queue",
        "all",
        "open",
        "inbox",
        "active",
        "backlog",
        "todo",
        "done",
        "ready",
        "blocked",
        "overdue",
        "upcoming",
        "conflicts",
        "epics",
        "recurring",
        "recent-actions",
    ] {
        assert!(Cli::try_parse_from(["aven", "tui", "--view", view]).is_ok());
    }
    for layout in ["list", "columns"] {
        assert!(Cli::try_parse_from(["aven", "tui", "--layout", layout]).is_ok());
    }
    for priority in ["none", "low", "medium", "high", "urgent"] {
        assert!(Cli::try_parse_from(["aven", "tui", "--priority", priority]).is_ok());
    }
    assert!(Cli::try_parse_from(["aven", "tui", "--view", "search"]).is_err());
    assert!(Cli::try_parse_from(["aven", "tui", "--priority", "critical"]).is_err());
}

#[test]
fn tui_launch_parses_task_and_optional_project_value() {
    let parsed = Cli::try_parse_from(["aven", "tui", "APP-1234"]).unwrap();
    let Some(Commands::Tui(args)) = parsed.command else {
        panic!("expected tui command");
    };
    assert_eq!(args.task_ref.as_deref(), Some("APP-1234"));
    assert_eq!(args.project, None);

    let parsed = Cli::try_parse_from(["aven", "tui", "-p", "app"]).unwrap();
    let Some(Commands::Tui(args)) = parsed.command else {
        panic!("expected tui command");
    };
    assert_eq!(args.project.as_deref(), Some("app"));
    assert_eq!(args.task_ref, None);

    let parsed = Cli::try_parse_from(["aven", "tui", "-p", "--view", "inbox"]).unwrap();
    let Some(Commands::Tui(args)) = parsed.command else {
        panic!("expected tui command");
    };
    assert_eq!(args.project.as_deref(), Some(""));
    assert_eq!(args.view, Some(TuiViewArg::Inbox));
}

#[test]
fn tui_launch_rejects_conflicting_targets_and_modes() {
    for arguments in [
        vec!["aven", "tui", "APP-1234", "--view", "open"],
        vec!["aven", "tui", "APP-1234", "--project", "app"],
        vec!["aven", "tui", "APP-1234", "--label", "bug"],
        vec!["aven", "tui", "APP-1234", "--priority", "high"],
        vec!["aven", "tui", "APP-1234", "--add-task"],
        vec!["aven", "tui", "--add-task", "--add-task-only"],
        vec!["aven", "tui", "--add-task-only", "--view", "inbox"],
        vec!["aven", "tui", "--add-task-only", "--label", "bug"],
        vec!["aven", "tui", "--natural"],
    ] {
        assert!(Cli::try_parse_from(arguments).is_err());
    }

    assert!(
        Cli::try_parse_from([
            "aven",
            "tui",
            "--add-task-only",
            "--project",
            "app",
            "--natural",
        ])
        .is_ok()
    );
}
