//! Keeps the LaunchAgent in step with the `background` intent while Aven
//! Desktop is the installation that owns it.
//!
//! The agent runs only when the intent is on and the config gives it sync or
//! reminders to do. An unset intent leaves whatever is installed alone, so a
//! terminal install's agent stays the terminal user's. The agent is never
//! started while the user has Aven turned off in Login Items.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::{self, AppConfig, BackgroundIntent};
use crate::config_edit;
use crate::daemon::service::{self, AgentStatus, Ensured, LoginItem};

/// What [`reconcile`] should do with the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Leave,
    Install,
    Remove,
}

/// What [`reconcile`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reconciled {
    Unchanged,
    Started,
    Removed,
    /// The agent should run, but the user turned Aven off in Login Items.
    TurnedOff,
}

/// The step that brings the agent in line with `intent`. `app` is Desktop's
/// executable. Only an agent running `app`, or none, is Desktop's to change.
pub fn decide(
    intent: Option<BackgroundIntent>,
    has_work: bool,
    agent: &AgentStatus,
    app: &Path,
) -> Action {
    let ours = match &agent.program {
        Some(program) => same_program(program, app),
        None => !agent.installed,
    };
    if !ours {
        return Action::Leave;
    }
    match intent {
        None => Action::Leave,
        Some(BackgroundIntent::On) if has_work => {
            if agent.login_item == LoginItem::TurnedOff {
                Action::Leave
            } else {
                Action::Install
            }
        }
        Some(_) if agent.installed => Action::Remove,
        Some(_) => Action::Leave,
    }
}

/// Installs, repairs or removes the agent for the database at `db_path` as
/// `config` asks. `app` is Desktop's executable.
pub fn reconcile(config: &AppConfig, db_path: PathBuf, app: &Path) -> Result<Reconciled> {
    let agent = service::agent_status()?;
    match decide(config.background, config.daemon_has_work(), &agent, app) {
        Action::Leave if wants_agent(config) && agent.login_item == LoginItem::TurnedOff => {
            Ok(Reconciled::TurnedOff)
        }
        Action::Leave => Ok(Reconciled::Unchanged),
        Action::Install => Ok(match service::ensure_agent(db_path)? {
            Ensured::Unchanged => Reconciled::Unchanged,
            Ensured::Started => Reconciled::Started,
            Ensured::TurnedOff => Reconciled::TurnedOff,
        }),
        Action::Remove => {
            service::remove_agent(app)?;
            Ok(Reconciled::Removed)
        }
    }
}

/// Whether `config` asks for a running agent.
pub fn wants_agent(config: &AppConfig) -> bool {
    config.background == Some(BackgroundIntent::On) && config.daemon_has_work()
}

/// Records `intent` in the config file at the default location.
pub fn set_intent(intent: BackgroundIntent) -> Result<()> {
    let path = config::config_file_path()?;
    config_edit::set_top_level_scalar(&path, "background", intent.as_str())
        .with_context(|| format!("set background in {}", path.display()))
}

fn same_program(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = "/Applications/Aven.app/Contents/MacOS/aven";
    // Not a real path: Homebrew's `aven` can link to the app, which would
    // make it the same program.
    const TERMINAL: &str = "/nonexistent/homebrew/bin/aven";

    fn agent(program: Option<&str>, login_item: LoginItem) -> AgentStatus {
        AgentStatus {
            installed: program.is_some(),
            program: program.map(PathBuf::from),
            loaded: false,
            running: false,
            login_item,
        }
    }

    fn decide_for(intent: Option<BackgroundIntent>, has_work: bool, agent: &AgentStatus) -> Action {
        decide(intent, has_work, agent, Path::new(APP))
    }

    #[test]
    fn unset_intent_leaves_any_agent_alone() {
        for installed in [None, Some(APP)] {
            let agent = agent(installed, LoginItem::Allowed);
            assert_eq!(decide_for(None, true, &agent), Action::Leave);
            assert_eq!(decide_for(None, false, &agent), Action::Leave);
        }
    }

    #[test]
    fn on_with_work_installs_or_repairs_the_agent() {
        let on = Some(BackgroundIntent::On);
        for installed in [None, Some(APP)] {
            for login_item in [LoginItem::Allowed, LoginItem::Unknown] {
                assert_eq!(
                    decide_for(on, true, &agent(installed, login_item)),
                    Action::Install
                );
            }
        }
    }

    #[test]
    fn turned_off_in_login_items_is_never_started() {
        let on = Some(BackgroundIntent::On);
        for installed in [None, Some(APP)] {
            assert_eq!(
                decide_for(on, true, &agent(installed, LoginItem::TurnedOff)),
                Action::Leave
            );
        }
    }

    #[test]
    fn off_or_no_work_removes_the_apps_agent() {
        let installed = agent(Some(APP), LoginItem::Allowed);
        let turned_off = agent(Some(APP), LoginItem::TurnedOff);
        let off = Some(BackgroundIntent::Off);
        let on = Some(BackgroundIntent::On);
        assert_eq!(decide_for(off, true, &installed), Action::Remove);
        assert_eq!(decide_for(off, false, &turned_off), Action::Remove);
        assert_eq!(decide_for(on, false, &installed), Action::Remove);
        assert_eq!(
            decide_for(off, true, &agent(None, LoginItem::Allowed)),
            Action::Leave
        );
    }

    #[test]
    fn another_installations_agent_is_left_alone() {
        let terminal = agent(Some(TERMINAL), LoginItem::Allowed);
        for intent in [
            None,
            Some(BackgroundIntent::On),
            Some(BackgroundIntent::Off),
        ] {
            assert_eq!(decide_for(intent, true, &terminal), Action::Leave);
        }
        let unreadable = AgentStatus {
            installed: true,
            program: None,
            loaded: true,
            running: true,
            login_item: LoginItem::Allowed,
        };
        assert_eq!(
            decide_for(Some(BackgroundIntent::Off), true, &unreadable),
            Action::Leave
        );
    }
}
