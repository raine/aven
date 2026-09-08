use std::collections::BTreeSet;

use anyhow::Result;

use crate::db::{Database, begin_immediate, get_meta, set_meta};
use crate::ids::WorkspaceId;

const IOS_QUEUE_WORKSPACE_META_KEY: &str = "ios_queue_workspace_id";
const ONBOARDING_META_KEY: &str = "tui_onboarding_version";
const FIRST_LAUNCH_ONBOARDING_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingStatus {
    Due,
    Complete,
    Established,
}

impl Database {
    pub async fn restore_ios_queue_workspace(&self) -> Result<crate::workspaces::Workspace> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let workspaces = crate::workspaces::list_workspaces(&mut tx).await?;
        let stored_id = get_meta(&mut tx, IOS_QUEUE_WORKSPACE_META_KEY)
            .await?
            .and_then(|value| value.parse::<WorkspaceId>().ok());
        let selected = stored_id
            .and_then(|id| workspaces.iter().find(|workspace| workspace.id == id))
            .or_else(|| workspaces.first())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no available workspace"))?;
        set_meta(&mut tx, IOS_QUEUE_WORKSPACE_META_KEY, selected.id.as_str()).await?;
        tx.commit().await?;
        Ok(selected)
    }

    pub async fn select_ios_queue_workspace(
        &self,
        workspace_id: &WorkspaceId,
    ) -> Result<crate::workspaces::Workspace> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let workspace = crate::workspaces::workspace_for_id(&mut tx, workspace_id).await?;
        set_meta(&mut tx, IOS_QUEUE_WORKSPACE_META_KEY, workspace.id.as_str()).await?;
        tx.commit().await?;
        Ok(workspace)
    }

    pub async fn onboarding_status(&self) -> Result<OnboardingStatus> {
        let mut conn = self.acquire_reader().await?;
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

    pub async fn complete_onboarding(&self) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
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

/// Database-local sidebar preferences shared across workspaces and projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SidebarSection {
    Views,
    Scope,
    Projects,
}

impl SidebarSection {
    fn meta_key(self) -> &'static str {
        match self {
            Self::Views => "tui_sidebar_views_collapsed",
            Self::Scope => "tui_sidebar_scope_collapsed",
            Self::Projects => "tui_sidebar_projects_collapsed",
        }
    }
}

impl Database {
    pub async fn collapsed_sidebar_sections(&self) -> Result<BTreeSet<SidebarSection>> {
        let mut conn = self.acquire_reader().await?;
        let mut sections = BTreeSet::new();
        for section in [
            SidebarSection::Views,
            SidebarSection::Scope,
            SidebarSection::Projects,
        ] {
            if get_meta(&mut conn, section.meta_key())
                .await?
                .as_deref()
                .map(str::trim)
                == Some("true")
            {
                sections.insert(section);
            }
        }
        Ok(sections)
    }

    pub async fn set_sidebar_section_collapsed(
        &self,
        section: SidebarSection,
        collapsed: bool,
    ) -> Result<()> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        set_meta(
            &mut tx,
            section.meta_key(),
            if collapsed { "true" } else { "false" },
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sidebar_preferences_are_local_and_invalid_values_default_to_expanded() {
        let database = Database::open(std::path::Path::new(":memory:"))
            .await
            .unwrap();
        let before: i64 = {
            let mut conn = database.acquire_reader().await.unwrap();
            sqlx::query_scalar("SELECT COUNT(*) FROM changes")
                .fetch_one(&mut *conn)
                .await
                .unwrap()
        };
        database
            .set_sidebar_section_collapsed(SidebarSection::Scope, true)
            .await
            .unwrap();
        {
            let mut conn = database.acquire_writer().await.unwrap();
            set_meta(&mut conn, SidebarSection::Views.meta_key(), "invalid")
                .await
                .unwrap();
        }
        assert_eq!(
            database.collapsed_sidebar_sections().await.unwrap(),
            BTreeSet::from([SidebarSection::Scope])
        );
        let mut conn = database.acquire_reader().await.unwrap();
        let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM changes")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(before, after);
    }
}
