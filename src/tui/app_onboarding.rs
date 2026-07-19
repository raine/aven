use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Size;

use crate::tui::app::App;
use crate::tui::overlay::OverlayState;
use crate::tui::store::OnboardingStatus;
use crate::tui::ui::{MIN_TUI_HEIGHT, MIN_TUI_WIDTH};

pub(super) const ONBOARDING_FRAME_INTERVAL: Duration = Duration::from_millis(33);
const ONBOARDING_HANDOFF_DURATION: Duration = Duration::from_millis(1_400);
const ONBOARDING_AFTERGLOW_DURATION: Duration = Duration::from_millis(700);
const ONBOARDING_INTRO_DURATION: Duration = Duration::from_millis(2_100);

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct OnboardingIntroVisual {
    pub(crate) purple_reveal: f32,
    pub(crate) check_reveal: f32,
    pub(crate) dim: f32,
    pub(crate) dialog_reveal: f32,
    pub(crate) afterglow: f32,
}

pub(super) struct OnboardingIntro {
    started: Instant,
}

impl OnboardingIntro {
    fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }

    pub(super) fn visual(&self) -> OnboardingIntroVisual {
        onboarding_visual(self.started.elapsed())
    }

    pub(super) fn remaining(&self) -> Duration {
        ONBOARDING_INTRO_DURATION.saturating_sub(self.started.elapsed())
    }

    pub(super) fn is_complete(&self) -> bool {
        self.started.elapsed() >= ONBOARDING_INTRO_DURATION
    }

    fn is_interactive(&self) -> bool {
        self.started.elapsed() >= ONBOARDING_HANDOFF_DURATION
    }
}

fn onboarding_visual(elapsed: Duration) -> OnboardingIntroVisual {
    let millis = elapsed.as_secs_f32() * 1_000.0;
    let afterglow_start = ONBOARDING_HANDOFF_DURATION.as_secs_f32() * 1_000.0;
    let afterglow_end =
        (ONBOARDING_HANDOFF_DURATION + ONBOARDING_AFTERGLOW_DURATION).as_secs_f32() * 1_000.0;
    OnboardingIntroVisual {
        purple_reveal: smoothstep(window(millis, 0.0, 400.0)),
        check_reveal: smoothstep(window(millis, 320.0, 700.0)),
        dim: smoothstep(window(millis, 950.0, 1_250.0)),
        dialog_reveal: ease_out_cubic(window(millis, 1_050.0, 1_400.0)),
        afterglow: window(millis, afterglow_start, afterglow_end),
    }
}

fn window(value: f32, start: f32, end: f32) -> f32 {
    ((value - start) / (end - start)).clamp(0.0, 1.0)
}

fn smoothstep(progress: f32) -> f32 {
    progress * progress * (3.0 - 2.0 * progress)
}

fn ease_out_cubic(progress: f32) -> f32 {
    1.0 - (1.0 - progress).powi(3)
}

impl App {
    pub(crate) async fn maybe_open_onboarding(&mut self) {
        match self.store.onboarding_status().await {
            Ok(OnboardingStatus::Due) => {
                self.overlay = Some(OverlayState::Onboarding {
                    persist_on_exit: true,
                });
                self.onboarding_intro = Some(OnboardingIntro::new());
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

    pub(super) fn onboarding_intro_visual(&self) -> Option<OnboardingIntroVisual> {
        self.onboarding_intro.as_ref().map(OnboardingIntro::visual)
    }

    pub(super) fn finish_onboarding_intro_if_elapsed(&mut self) -> bool {
        if self
            .onboarding_intro
            .as_ref()
            .is_some_and(OnboardingIntro::is_complete)
        {
            self.onboarding_intro = None;
            return true;
        }
        false
    }

    pub(super) fn skip_onboarding_intro(&mut self) -> bool {
        let consume = self
            .onboarding_intro
            .as_ref()
            .is_some_and(|intro| !intro.is_interactive());
        self.onboarding_intro = None;
        consume
    }

    pub(super) fn onboarding_intro_timeout(&self) -> Option<Duration> {
        self.onboarding_intro
            .as_ref()
            .map(|intro| intro.remaining().min(ONBOARDING_FRAME_INTERVAL))
    }

    pub(super) fn show_welcome(&mut self) {
        self.pending_shortcut.clear();
        self.onboarding_intro = None;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_draw_overlaps_end_of_purple_reveal() {
        let before_check = onboarding_visual(Duration::from_millis(320));
        assert!(before_check.purple_reveal > 0.8);
        assert_eq!(before_check.check_reveal, 0.0);
        assert_eq!(before_check.afterglow, 0.0);

        let overlap = onboarding_visual(Duration::from_millis(360));
        assert!(overlap.purple_reveal < 1.0);
        assert!(overlap.check_reveal > 0.0);
        assert_eq!(overlap.afterglow, 0.0);
    }

    #[test]
    fn completed_check_holds_before_the_handoff() {
        let hold = onboarding_visual(Duration::from_millis(900));
        assert_eq!(hold.purple_reveal, 1.0);
        assert_eq!(hold.check_reveal, 1.0);
        assert_eq!(hold.dim, 0.0);
        assert_eq!(hold.dialog_reveal, 0.0);
        assert_eq!(hold.afterglow, 0.0);
    }

    #[test]
    fn handoff_finishes_before_the_afterglow() {
        let handoff = onboarding_visual(Duration::from_millis(1_100));
        assert!(handoff.dim > 0.0);
        assert!(handoff.dialog_reveal > 0.0);
        assert_eq!(handoff.afterglow, 0.0);

        let afterglow =
            onboarding_visual(ONBOARDING_HANDOFF_DURATION + ONBOARDING_AFTERGLOW_DURATION / 2);
        assert_eq!(afterglow.dim, 1.0);
        assert_eq!(afterglow.dialog_reveal, 1.0);
        assert_eq!(afterglow.afterglow, 0.5);

        let complete = onboarding_visual(ONBOARDING_INTRO_DURATION);
        assert_eq!(complete.purple_reveal, 1.0);
        assert_eq!(complete.check_reveal, 1.0);
        assert_eq!(complete.dim, 1.0);
        assert_eq!(complete.dialog_reveal, 1.0);
        assert_eq!(complete.afterglow, 1.0);
    }
}
