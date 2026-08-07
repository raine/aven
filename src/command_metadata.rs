use crate::cli::{
    Commands, ConflictSubcommand, DepSubcommand, EpicSubcommand, LabelSubcommand,
    ProjectPathSubcommand, ProjectSubcommand, RecurSubcommand, TextSubcommand,
};
use crate::logging;

pub(crate) struct CommandMetadata {
    pub(crate) log_mode: logging::LogMode,
    pub(crate) wakes_daemon: bool,
}

impl CommandMetadata {
    fn cli() -> Self {
        Self {
            log_mode: logging::LogMode::Cli,
            wakes_daemon: false,
        }
    }

    fn cli_wake() -> Self {
        Self {
            log_mode: logging::LogMode::Cli,
            wakes_daemon: true,
        }
    }

    fn server() -> Self {
        Self {
            log_mode: logging::LogMode::Server,
            wakes_daemon: false,
        }
    }

    fn daemon() -> Self {
        Self {
            log_mode: logging::LogMode::Daemon,
            wakes_daemon: false,
        }
    }

    fn tui() -> Self {
        Self {
            log_mode: logging::LogMode::Tui,
            wakes_daemon: false,
        }
    }
}

impl Commands {
    pub(crate) fn metadata(&self) -> CommandMetadata {
        match self {
            Self::Add(_) => CommandMetadata::cli_wake(),
            Self::Context(_) => CommandMetadata::cli(),
            Self::Show(_) => CommandMetadata::cli(),
            Self::List(_) => CommandMetadata::cli(),
            Self::Search(_) => CommandMetadata::cli(),
            Self::BulkUpdate(args) => CommandMetadata {
                log_mode: logging::LogMode::Cli,
                wakes_daemon: !args.dry_run,
            },
            Self::Prime(_) => CommandMetadata::cli(),
            Self::Edit(_) => CommandMetadata::cli_wake(),
            Self::Update(_) => CommandMetadata::cli(),
            Self::Note(_) => CommandMetadata::cli_wake(),
            Self::NoteDelete(_) => CommandMetadata::cli_wake(),
            Self::Delete(_) => CommandMetadata::cli_wake(),
            Self::Restore(_) => CommandMetadata::cli_wake(),
            Self::Attachment(args) => CommandMetadata {
                log_mode: logging::LogMode::Cli,
                wakes_daemon: args.command.wakes_daemon(),
            },
            Self::Text(args) => CommandMetadata {
                log_mode: logging::LogMode::Cli,
                wakes_daemon: args.command.wakes_daemon(),
            },
            Self::Label(args) => CommandMetadata {
                log_mode: logging::LogMode::Cli,
                wakes_daemon: args.command.wakes_daemon(),
            },
            Self::Project(args) => CommandMetadata {
                log_mode: logging::LogMode::Cli,
                wakes_daemon: args.command.wakes_daemon(),
            },
            Self::Recur(args) => CommandMetadata {
                log_mode: logging::LogMode::Cli,
                wakes_daemon: args.command.wakes_daemon(),
            },
            Self::Workspace(_) => CommandMetadata::cli_wake(),
            Self::Dep(args) => CommandMetadata {
                log_mode: logging::LogMode::Cli,
                wakes_daemon: args.command.wakes_daemon(),
            },
            Self::Epic(args) => CommandMetadata {
                log_mode: logging::LogMode::Cli,
                wakes_daemon: args.command.wakes_daemon(),
            },
            Self::Conflict(args) => CommandMetadata {
                log_mode: logging::LogMode::Cli,
                wakes_daemon: args.command.wakes_daemon(),
            },
            Self::Config(_) => CommandMetadata::cli(),
            Self::Backup(_) => CommandMetadata::cli(),
            Self::Export(_) => CommandMetadata::cli(),
            Self::Import(_) => CommandMetadata::cli_wake(),
            Self::Doctor(_) => CommandMetadata::cli(),
            Self::Skill(_) => CommandMetadata::cli(),
            Self::Sync(_) => CommandMetadata::cli(),
            Self::Server(_) => CommandMetadata::server(),
            Self::Daemon(_) => CommandMetadata::daemon(),
            Self::Tui(_) => CommandMetadata::tui(),
            Self::Demo => CommandMetadata::tui(),
            Self::Internal(_) => CommandMetadata::cli(),
        }
    }
}

impl RecurSubcommand {
    pub(crate) fn wakes_daemon(&self) -> bool {
        !matches!(self, Self::List(_) | Self::Show(_) | Self::History(_))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_uses_tui_metadata() {
        let metadata = Commands::Demo.metadata();
        assert_eq!(metadata.log_mode, logging::LogMode::Tui);
        assert!(!metadata.wakes_daemon);
    }
}
