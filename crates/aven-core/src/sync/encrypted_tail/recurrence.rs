use crate::sync::wire::ChangeWire;

pub(super) fn is_deterministic(change: &ChangeWire) -> bool {
    change.op_type == "project_recurrence_occurrence"
        || (change.op_type == "create_task" && change.payload["series_id"].is_string())
}
