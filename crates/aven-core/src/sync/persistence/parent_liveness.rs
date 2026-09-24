/// Accepted-order conservative retention, independent of decrypted task storage.
#[derive(Default)]
pub(crate) struct ParentState {
    pub version: Option<String>,
    pub deleted: bool,
    pub protected: bool,
}
impl ParentState {
    /// A reference hint can add protection but cannot establish deletion evidence.
    pub fn protect_hint(&mut self, deleted: bool, version: Option<&str>) {
        self.protected |=
            self.version.is_none() || self.version.as_deref() != version || self.deleted != deleted;
    }

    pub fn apply(&mut self, action: u8, id: &str, deleted: bool, version: Option<&str>) {
        if action == 0 {
            if self.version.is_none() {
                self.version = version.map(str::to_owned);
            }
        } else if self.version.is_none() || (action == 1 && version != self.version.as_deref()) {
            self.protected = true;
        } else {
            self.deleted = deleted;
            self.version = Some(id.to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Applies accepted history as (id, action, deleted, version) where action
    /// 0 creates, 1 sets and 2 force-resolves the deleted field; returns
    /// whether the parent is unreferenced.
    fn project(history: &[(&str, u8, bool, Option<&str>)]) -> bool {
        let mut state = ParentState::default();
        for (id, action, deleted, version) in history {
            state.apply(*action, id, *deleted, *version);
        }
        state.deleted && state.version.is_some() && !state.protected
    }

    #[test]
    fn ordered_deletion_restore_and_custom_seed() {
        let mut history = vec![
            ("create", 0, false, Some("seed")),
            ("delete", 1, true, Some("seed")),
        ];
        assert!(project(&history));
        history.push(("restore", 1, false, Some("delete")));
        assert!(!project(&history));
        history.push(("delete2", 1, true, Some("restore")));
        assert!(project(&history));
    }

    #[test]
    fn stale_delete_and_force_resolution_keep_protection() {
        let mut history = vec![
            ("create", 0, false, Some("seed")),
            ("delete", 1, true, Some("seed")),
            ("restore", 1, false, Some("delete")),
            ("stale", 1, true, Some("seed")),
        ];
        assert!(!project(&history));
        history.push(("resolve", 2, true, Some("restore")));
        assert!(!project(&history));
    }

    #[test]
    fn equal_value_conflicts_and_missing_creation_are_conservative() {
        assert!(!project(&[
            ("create", 0, false, Some("seed")),
            ("delete", 1, true, Some("seed")),
            ("other", 1, true, Some("seed")),
        ]));
        assert!(!project(&[("delete", 1, true, None)]));
        assert!(!project(&[]));
    }
}
