use crate::choices::{PRIORITIES, STATUSES};
use crate::query::ProjectListItem;
use crate::tui::overlay::PickerItem;
use crate::workspaces::Workspace;

use super::TuiStore;

pub(crate) const CREATE_PROJECT_PICKER_VALUE_PREFIX: &str = "\0create-project:";

pub(crate) fn create_project_picker_name(value: &str) -> Option<&str> {
    value
        .strip_prefix(CREATE_PROJECT_PICKER_VALUE_PREFIX)
        .filter(|name| !name.is_empty())
}

impl TuiStore {
    pub(crate) fn status_picker_items(&self, selected: Option<&str>) -> Vec<PickerItem> {
        let selected = selected.unwrap_or_default();
        STATUSES
            .iter()
            .map(|status| PickerItem {
                label: (*status).to_string(),
                value: (*status).to_string(),
                selected: *status == selected,
            })
            .collect()
    }

    pub(crate) fn label_picker_items(&self) -> Vec<PickerItem> {
        self.labels
            .iter()
            .map(|label| PickerItem {
                label: label.clone(),
                value: label.clone(),
                selected: false,
            })
            .collect()
    }

    pub(crate) fn existing_project_picker_items(&self, selected: &str) -> Vec<PickerItem> {
        self.projects
            .iter()
            .map(|project| project_picker_item(project, selected))
            .collect()
    }

    pub(crate) fn edit_project_picker_items(&self, selected: &str) -> Vec<PickerItem> {
        let mut items = self.existing_project_picker_items(selected);
        items.push(create_project_picker_item());
        items
    }

    pub(crate) fn project_picker_items(&self, selected: Option<&str>) -> Vec<PickerItem> {
        let selected = selected.unwrap_or_default();
        let inferred_label = self
            .projects
            .iter()
            .find(|project| project.key == selected)
            .map(|project| format!("Infer project ({})", project.key))
            .unwrap_or_else(|| "Infer project".to_string());
        let mut items = vec![PickerItem {
            label: inferred_label,
            value: String::new(),
            selected: selected.is_empty(),
        }];
        items.extend(
            self.projects
                .iter()
                .map(|project| project_picker_item(project, selected)),
        );
        items.push(create_project_picker_item());
        items
    }

    pub(crate) fn priority_picker_items(&self, selected: &str) -> Vec<PickerItem> {
        PRIORITIES
            .iter()
            .map(|priority| PickerItem {
                label: (*priority).to_string(),
                value: (*priority).to_string(),
                selected: *priority == selected,
            })
            .collect()
    }

    pub(crate) fn workspace_picker_items(&self) -> Vec<PickerItem> {
        let selected_key = self
            .workspaces
            .iter()
            .find(|workspace| workspace.key != self.active_workspace.key)
            .map(|workspace| workspace.key.as_str());
        self.workspaces
            .iter()
            .filter(|workspace| workspace.key == self.active_workspace.key)
            .chain(
                self.workspaces
                    .iter()
                    .filter(|workspace| workspace.key != self.active_workspace.key),
            )
            .map(|workspace| workspace_picker_item(workspace, selected_key))
            .collect()
    }

    pub(crate) fn workspace_rename_picker_items(&self) -> Vec<PickerItem> {
        let selected_key = Some(self.active_workspace.key.as_str());
        self.workspaces
            .iter()
            .filter(|workspace| workspace.key == self.active_workspace.key)
            .chain(
                self.workspaces
                    .iter()
                    .filter(|workspace| workspace.key != self.active_workspace.key),
            )
            .map(|workspace| workspace_picker_item(workspace, selected_key))
            .collect()
    }

    pub(crate) fn selected_dependency_picker_items(&self, index: Option<usize>) -> Vec<PickerItem> {
        let Some(item) = self.selected_task(index) else {
            return Vec::new();
        };
        item.depends_on
            .iter()
            .map(|link| PickerItem {
                label: format!("{} {}", link.display_ref, link.title),
                value: link.task_id.to_string(),
                selected: false,
            })
            .collect()
    }
}

fn create_project_picker_item() -> PickerItem {
    PickerItem {
        label: "+ Create project".to_string(),
        value: CREATE_PROJECT_PICKER_VALUE_PREFIX.to_string(),
        selected: false,
    }
}

fn project_picker_item(project: &ProjectListItem, selected: &str) -> PickerItem {
    PickerItem {
        label: format!("{} {}", project.prefix, project.name),
        value: project.key.clone(),
        selected: project.key == selected,
    }
}

pub(crate) fn deleted_picker_items(selected: &str) -> Vec<PickerItem> {
    ["0", "1"]
        .into_iter()
        .map(|value| PickerItem {
            label: if value == "1" {
                "deleted".to_string()
            } else {
                "not deleted".to_string()
            },
            value: value.to_string(),
            selected: value == selected,
        })
        .collect()
}

fn workspace_picker_item(workspace: &Workspace, selected_key: Option<&str>) -> PickerItem {
    let label = if workspace.name == workspace.key {
        workspace.name.clone()
    } else {
        format!("{} ({})", workspace.name, workspace.key)
    };
    PickerItem {
        label,
        value: workspace.key.clone(),
        selected: selected_key.is_some_and(|key| workspace.key == key),
    }
}
