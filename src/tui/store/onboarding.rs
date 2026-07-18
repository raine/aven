use anyhow::Result;

use crate::db::{begin_immediate, get_meta, set_meta};

use super::TuiStore;

const ONBOARDING_META_KEY: &str = "tui_onboarding_version";
const FIRST_LAUNCH_ONBOARDING_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OnboardingStatus {
    Due,
    Complete,
    Established,
}

impl TuiStore {
    pub(crate) async fn onboarding_status(&self) -> Result<OnboardingStatus> {
        let mut conn = self.pool.acquire().await?;
        let marker = get_meta(&mut conn, ONBOARDING_META_KEY).await?;
        if marker_version(marker.as_deref())
            .is_some_and(|version| version >= FIRST_LAUNCH_ONBOARDING_VERSION)
        {
            return Ok(OnboardingStatus::Complete);
        }

        let has_tasks: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tasks)")
            .fetch_one(&mut *conn)
            .await?;
        Ok(if has_tasks {
            OnboardingStatus::Established
        } else {
            OnboardingStatus::Due
        })
    }

    pub(crate) async fn complete_onboarding(&self) -> Result<()> {
        let mut conn = self.pool.acquire().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let marker = get_meta(&mut tx, ONBOARDING_META_KEY).await?;
        if marker_version(marker.as_deref())
            .is_none_or(|version| version < FIRST_LAUNCH_ONBOARDING_VERSION)
        {
            set_meta(
                &mut tx,
                ONBOARDING_META_KEY,
                &FIRST_LAUNCH_ONBOARDING_VERSION.to_string(),
            )
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

fn marker_version(value: Option<&str>) -> Option<u32> {
    value?.trim().parse().ok()
}
