use crate::cli::{
    Commands, ConflictSubcommand, DepSubcommand, EpicSubcommand, LabelSubcommand,
    ProjectPathSubcommand, ProjectSubcommand, TextSubcommand,
};
use crate::logging;

pub(crate) struct CommandMetadata {
    pub(crate) log_mode: logging::LogMode,
    pub(crate) needs_workspace: bool,
    pub(crate) wakes_daemon: bool,
}

impl CommandMetadata {
    fn cli() -> Self {
        Self {
            log_mode: logging::LogMode::Cli,
            needs_workspace: false,
            wakes_daemon: false,
        }
    }

    fn cli_workspace() -> Self {
        Self {
            log_mode: logging::LogMode::Cli,
            needs_workspace: true,
            wakes_daemon: false,
        }
    }

    fn cli_workspace_wake() -> Self {
        Self {
            log_mode: logging::LogMode::Cli,
            needs_workspace: true,
            wakes_daemon: true,
        }
    }

    fn server() -> Self {
        Self {
            log_mode: logging::LogMode::Server,
            needs_workspace: false,
            wakes_daemon: false,
        }
    }

    fn daemon() -> Self {
        Self {
            log_mode: logging::LogMode::Daemon,
            needs_workspace: false,
            wakes_daemon: false,
        }
    }

    fn tui() -> Self {
        Self {
            log_mode: logging::LogMode::Tui,
            needs_workspace: true,
            wakes_daemon: false,
        }
    }
}

impl Commands {
    pub(crate) fn metadata(&self) -> CommandMetadata {
        match self {
            Self::Add(_) => CommandMetadata::cli_workspace_wake(),
            Self::Context(_) => CommandMetadata::cli_workspace(),
            Self::Show(_) => CommandMetadata::cli_workspace(),
            Self::List(_) => CommandMetadata::cli_workspace(),
            Self::Search(_) => CommandMetadata::cli_workspace(),
            Self::BulkUpdate(args) => CommandMetadata {
                log_mode: logging::LogMode::Cli,
                needs_workspace: true,
                wakes_daemon: !args.dry_run,
            },
            Self::Prime(_) => CommandMetadata::cli_workspace(),
            Self::Update(_) => CommandMetadata::cli_workspace_wake(),
            Self::Note(_) => CommandMetadata::cli_workspace_wake(),
            Self::NoteDelete(_) => CommandMetadata::cli_workspace_wake(),
            Self::Delete(_) => CommandMetadata::cli_workspace_wake(),
            Self::Restore(_) => CommandMetadata::cli_workspace_wake(),
            Self::Text(args) => CommandMetadata {
                log_mode: logging::LogMode::Cli,
                needs_workspace: true,
                wakes_daemon: args.command.wakes_daemon(),
            },
            Self::Label(args) => CommandMetadata {
                log_mode: logging::LogMode::Cli,
                needs_workspace: true,
                wakes_daemon: args.command.wakes_daemon(),
            },
            Self::Project(args) => CommandMetadata {
                log_mode: logging::LogMode::Cli,
                needs_workspace: true,
                wakes_daemon: args.command.wakes_daemon(),
            },
            Self::Workspace(_) => CommandMetadata::cli_workspace_wake(),
            Self::Dep(args) => CommandMetadata {
                log_mode: logging::LogMode::Cli,
                needs_workspace: true,
                wakes_daemon: args.command.wakes_daemon(),
            },
            Self::Epic(args) => CommandMetadata {
                log_mode: logging::LogMode::Cli,
                needs_workspace: true,
                wakes_daemon: args.command.wakes_daemon(),
            },
            Self::Conflict(args) => CommandMetadata {
                log_mode: logging::LogMode::Cli,
                needs_workspace: true,
                wakes_daemon: args.command.wakes_daemon(),
            },
            Self::Config(_) => CommandMetadata::cli(),
            Self::Backup(_) => CommandMetadata::cli(),
            Self::Export(_) => CommandMetadata::cli(),
            Self::Import(_) => CommandMetadata::cli_workspace_wake(),
            Self::Doctor(_) => CommandMetadata::cli_workspace(),
            Self::Skill => CommandMetadata::cli(),
            Self::Sync(_) => CommandMetadata::cli_workspace(),
            Self::Server(_) => CommandMetadata::server(),
            Self::Daemon(_) => CommandMetadata::daemon(),
            Self::Tui(_) => CommandMetadata::tui(),
            Self::Tmux(_) => CommandMetadata::cli(),
            Self::Internal(_) => CommandMetadata::cli(),
        }
    }
}

impl LabelSubcommand {
    pub(crate) fn wakes_daemon(&self) -> bool {
        matches!(self, Self::Create { .. } | Self::Delete { .. })
    }
}

impl ProjectSubcommand {
    pub(crate) fn wakes_daemon(&self) -> bool {
        matches!(
            self,
            Self::Create { .. }
                | Self::Delete { .. }
                | Self::Rename { .. }
                | Self::Path {
                    command: ProjectPathSubcommand::Add { .. }
                        | ProjectPathSubcommand::Remove { .. },
                }
        )
    }
}

impl DepSubcommand {
    pub(crate) fn wakes_daemon(&self) -> bool {
        matches!(self, Self::Add { .. } | Self::Remove { .. })
    }
}

impl EpicSubcommand {
    pub(crate) fn wakes_daemon(&self) -> bool {
        matches!(self, Self::Add { .. } | Self::Remove { .. })
    }
}

impl TextSubcommand {
    pub(crate) fn wakes_daemon(&self) -> bool {
        matches!(self, Self::Set { .. })
    }
}

impl ConflictSubcommand {
    pub(crate) fn wakes_daemon(&self) -> bool {
        matches!(self, Self::Resolve { .. })
    }
}
