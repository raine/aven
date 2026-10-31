use anyhow::Result;

pub use aven_core::local_state::OnboardingStatus;

use super::TuiStore;

impl TuiStore {
    pub async fn onboarding_status(&self) -> Result<OnboardingStatus> {
        self.database.onboarding_status().await
    }

    pub async fn complete_onboarding(&self) -> Result<()> {
        self.database.complete_onboarding().await
    }
}
