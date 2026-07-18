use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TransferObject {
    pub(super) sha256: String,
    pub(super) byte_size: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TransferBudget {
    pub(super) objects: usize,
    pub(super) bytes: u64,
    pub(super) completed_objects: usize,
}

impl TransferBudget {
    pub(super) fn consume(&mut self, object: &TransferObject) {
        self.objects = self.objects.saturating_sub(1);
        self.bytes = self.bytes.saturating_sub(object.byte_size);
        self.completed_objects += 1;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PendingChange {
    pub(super) missing_blob: Option<TransferObject>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ChangePrefixPlan {
    pub(super) change_count: usize,
    pub(super) transfers: Vec<TransferObject>,
}

pub(super) fn plan_change_prefix(
    changes: &[PendingChange],
    budget: TransferBudget,
) -> ChangePrefixPlan {
    let mut plan = ChangePrefixPlan::default();
    let mut seen = HashSet::new();
    let mut remaining = budget;

    for change in changes {
        if let Some(blob) = &change.missing_blob
            && seen.insert(blob.sha256.as_str())
        {
            if !object_fits(blob, remaining, plan.transfers.is_empty()) {
                break;
            }
            remaining.consume(blob);
            plan.transfers.push(blob.clone());
        }
        plan.change_count += 1;
    }

    plan
}

pub(super) fn plan_transfers(
    objects: &[TransferObject],
    budget: TransferBudget,
) -> Vec<TransferObject> {
    let mut plan = Vec::new();
    let mut seen = HashSet::new();
    let mut remaining = budget;

    for object in objects {
        if !seen.insert(object.sha256.as_str()) {
            continue;
        }
        if !object_fits(object, remaining, plan.is_empty()) {
            break;
        }
        remaining.consume(object);
        plan.push(object.clone());
    }

    plan
}

fn object_fits(object: &TransferObject, budget: TransferBudget, plan_empty: bool) -> bool {
    if budget.objects == 0 {
        return false;
    }
    if object.byte_size <= budget.bytes {
        return true;
    }
    budget.completed_objects == 0 && plan_empty
}

#[cfg(test)]
mod tests {
    use super::*;

    fn object(hash: &str, byte_size: u64) -> TransferObject {
        TransferObject {
            sha256: hash.to_string(),
            byte_size,
        }
    }

    fn change(blob: Option<TransferObject>) -> PendingChange {
        PendingChange { missing_blob: blob }
    }

    #[test]
    fn transfer_plan_observes_exact_count_and_byte_boundaries() {
        let objects = vec![object("a", 4), object("b", 6), object("c", 1)];
        let plan = plan_transfers(
            &objects,
            TransferBudget {
                objects: 2,
                bytes: 10,
                completed_objects: 0,
            },
        );
        assert_eq!(plan, objects[..2]);
    }

    #[test]
    fn transfer_plan_stops_before_first_object_over_byte_budget() {
        let objects = vec![object("a", 6), object("b", 5), object("c", 1)];
        let plan = plan_transfers(
            &objects,
            TransferBudget {
                objects: 3,
                bytes: 10,
                completed_objects: 0,
            },
        );
        assert_eq!(plan, objects[..1]);
    }

    #[test]
    fn transfer_plan_allows_one_progress_object_before_any_completed_transfer() {
        let objects = vec![object("a", 11), object("b", 1)];
        let plan = plan_transfers(
            &objects,
            TransferBudget {
                objects: 2,
                bytes: 10,
                completed_objects: 0,
            },
        );
        assert_eq!(plan, objects[..1]);

        let blocked = plan_transfers(
            &objects,
            TransferBudget {
                objects: 2,
                bytes: 10,
                completed_objects: 1,
            },
        );
        assert!(blocked.is_empty());
    }

    #[test]
    fn change_plan_deduplicates_hashes_and_preserves_the_largest_prefix() {
        let duplicate = object("a", 6);
        let changes = vec![
            change(None),
            change(Some(duplicate.clone())),
            change(Some(duplicate)),
            change(None),
            change(Some(object("b", 5))),
            change(None),
        ];
        let plan = plan_change_prefix(
            &changes,
            TransferBudget {
                objects: 2,
                bytes: 10,
                completed_objects: 0,
            },
        );
        assert_eq!(plan.change_count, 4);
        assert_eq!(plan.transfers, vec![object("a", 6)]);
    }
}
