#[derive(Debug, Clone)]
pub(crate) struct Task {
    pub(crate) id: String,
    pub(crate) workspace_id: String,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) project_key: String,
    pub(crate) project_prefix: String,
    pub(crate) status: String,
    pub(crate) priority: String,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
    pub(crate) queue_activity_at: String,
    pub(crate) deleted: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct Project {
    pub(crate) workspace_id: String,
    pub(crate) key: String,
    pub(crate) name: String,
    pub(crate) prefix: String,
}
