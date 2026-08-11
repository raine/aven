use anyhow::{Context, Result};

use crate::config::{CustomTuiCommandExecution, CustomTuiCommandSuccess};
use crate::tui::app::{App, Notification};
use crate::tui::custom_command::{plan_invocation, resolve_command_targets};
use crate::tui::event::CommandHandler;

impl App {
    pub(super) async fn execute_command_handler(&mut self, handler: CommandHandler) -> Result<()> {
        match handler {
            CommandHandler::BuiltIn(action) => self.execute(action).await,
            CommandHandler::Custom(command_id) => {
                let name = self
                    .command_catalog
                    .custom(command_id)
                    .context("custom command disappeared from the catalog")?
                    .name
                    .clone();
                self.execute_custom_command(command_id, &name).await
            }
        }
    }

    pub(super) async fn execute_custom_command(
        &mut self,
        command_id: usize,
        invoked_as: &str,
    ) -> Result<()> {
        let command = self
            .command_catalog
            .custom(command_id)
            .cloned()
            .context("custom command disappeared from the catalog")?;
        let primary = self.store.selected_task(self.list.selected_task());
        let marked_ids = self.marked_task_ids_in_view();
        let marked = marked_ids
            .iter()
            .filter_map(|id| self.store.tasks.iter().find(|item| item.task.id == *id))
            .collect::<Vec<_>>();
        let targets = match resolve_command_targets(command.target, primary, &marked) {
            Ok(targets) => targets,
            Err(reason) => {
                self.set_warning(format!(":{} is disabled: {reason}", command.name));
                return Ok(());
            }
        };
        let invocation = plan_invocation(
            &command,
            invoked_as,
            &self.store.active_workspace,
            primary,
            &marked,
            &targets,
        )?;
        if let Err(error) = self.custom_commands.launch(invocation) {
            self.set_error(format!("{error:#}"));
            return Ok(());
        }
        self.overlay = None;
        match command.execution {
            CustomTuiCommandExecution::Background => {
                self.set_info(format!("launched :{}", command.name));
            }
            CustomTuiCommandExecution::Wait => {
                self.notification =
                    Some(Notification::loading(format!("running :{}", command.name)));
            }
        }
        Ok(())
    }

    pub(super) async fn poll_custom_commands(&mut self) -> bool {
        let completions = self.custom_commands.poll().await;
        if completions.is_empty() {
            return false;
        }
        for completion in completions {
            match completion.result {
                Ok(()) => {
                    if completion.on_success == CustomTuiCommandSuccess::Quit {
                        self.should_quit = true;
                    } else {
                        self.set_info(format!("custom command :{} completed", completion.name));
                    }
                }
                Err(error) => self.set_error(error),
            }
        }
        true
    }
}
