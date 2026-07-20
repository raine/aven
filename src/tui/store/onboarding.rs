use anyhow::Result;

pub(crate) use aven_core::local_state::OnboardingStatus;

use super::TuiStore;

impl TuiStore {
    pub(crate) async fn onboarding_status(&self) -> Result<OnboardingStatus> {
        self.database.onboarding_status().await
    }

    pub(crate) async fn complete_onboarding(&self) -> Result<()> {
        self.database.complete_onboarding().await
    }
}
