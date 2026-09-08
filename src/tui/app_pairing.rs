use crate::tui::app::App;
use crate::tui::overlay::OverlayState;

impl App {
    pub(in crate::tui) fn show_pairing_invitation(&mut self) {
        match self.store.pairing_presentation() {
            Ok(presentation) => {
                self.overlay = Some(OverlayState::Pairing(std::sync::Arc::new(presentation)));
            }
            Err(error) => {
                self.overlay = None;
                self.set_error(format!("pairing unavailable · {}", error.reason()));
            }
        }
    }

    pub(in crate::tui) fn pairing_missing_config_reason(&self) -> Option<&'static str> {
        self.store
            .pairing_input_error()
            .map(crate::pairing::PairingError::reason)
    }
}
