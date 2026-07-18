use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Size;

use crate::tui::app::App;
use crate::tui::overlay::OverlayState;
use crate::tui::store::OnboardingStatus;
use crate::tui::ui::{MIN_TUI_HEIGHT, MIN_TUI_WIDTH};

impl App {
    pub(crate) async fn maybe_open_onboarding(&mut self) {
        match self.store.onboarding_status().await {
            Ok(OnboardingStatus::Due) => {
                self.overlay = Some(OverlayState::Onboarding {
                    persist_on_exit: true,
                });
            }
            Ok(OnboardingStatus::Established) => {
                if let Err(error) = self.store.complete_onboarding().await {
                    self.set_warning(format!("could not save welcome state: {error}"));
                }
            }
            Ok(OnboardingStatus::Complete) => {}
            Err(error) => self.set_warning(format!("could not load welcome state: {error}")),
        }
    }

    pub(super) fn show_welcome(&mut self) {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::Onboarding {
            persist_on_exit: false,
        });
    }

    pub(super) async fn handle_onboarding_key(
        &mut self,
        key: KeyEvent,
        terminal_size: Size,
        persist_on_exit: bool,
    ) -> Result<()> {
        if terminal_size.width < MIN_TUI_WIDTH || terminal_size.height < MIN_TUI_HEIGHT {
            self.overlay = Some(OverlayState::Onboarding { persist_on_exit });
            return Ok(());
        }
        if !matches!(key.modifiers, KeyModifiers::NONE | KeyModifiers::SHIFT) {
            self.overlay = Some(OverlayState::Onboarding { persist_on_exit });
            return Ok(());
        }

        match key.code {
            KeyCode::Char('a') => {
                self.finish_onboarding(persist_on_exit).await;
                self.begin_add_task().await?;
            }
            KeyCode::Char('?') => {
                self.finish_onboarding(persist_on_exit).await;
                self.overlay = Some(OverlayState::Help { scroll: 0 });
            }
            KeyCode::Enter | KeyCode::Esc => {
                self.finish_onboarding(persist_on_exit).await;
                self.overlay = None;
            }
            KeyCode::Char('q') => {
                self.finish_onboarding(persist_on_exit).await;
                self.overlay = None;
                self.should_quit = true;
            }
            _ => self.overlay = Some(OverlayState::Onboarding { persist_on_exit }),
        }
        Ok(())
    }

    async fn finish_onboarding(&mut self, persist_on_exit: bool) {
        if persist_on_exit && let Err(error) = self.store.complete_onboarding().await {
            self.set_warning(format!("could not save welcome state: {error}"));
        }
    }
}
