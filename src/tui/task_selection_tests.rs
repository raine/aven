use std::collections::BTreeSet;

use crate::tui::store::task_selection::TaskSelection;
use crate::tui::test_support::task_list_item_with_id;

#[test]
fn resolves_visible_marks_before_selected_row() {
    let tasks = vec![
        task_list_item_with_id("first", "task-1"),
        task_list_item_with_id("second", "task-2"),
        task_list_item_with_id("third", "task-3"),
    ];
    let marked = BTreeSet::from([tasks[0].task.id.clone(), tasks[2].task.id.clone()]);

    let selection = TaskSelection::resolve(&tasks, &marked, Some(1)).unwrap();

    assert_eq!(selection.len(), 2);
    assert_eq!(
        selection.ids().cloned().collect::<Vec<_>>(),
        vec![tasks[0].task.id.clone(), tasks[2].task.id.clone()]
    );
    assert_eq!(selection.anchor_id(), &tasks[1].task.id);
    assert_eq!(selection.anchor_index(), 1);
}

#[test]
fn resolves_single_target_without_marks() {
    let tasks = vec![
        task_list_item_with_id("first", "task-1"),
        task_list_item_with_id("second", "task-2"),
    ];

    let selection = TaskSelection::resolve_single(&tasks, Some(1)).unwrap();

    assert_eq!(selection.single_id(), Some(&tasks[1].task.id));
    assert_eq!(selection.anchor_id(), &tasks[1].task.id);
    assert_eq!(selection.anchor_index(), 1);
    assert_eq!(TaskSelection::resolve_single(&tasks, None), None);
}

#[test]
fn detail_target_uses_open_task_as_refresh_anchor() {
    let anchor = task_list_item_with_id("open", "task-open");
    let target = task_list_item_with_id("related", "task-related");

    let selection = TaskSelection::for_detail_target(target.clone(), &anchor, 3);

    assert_eq!(selection.single_id(), Some(&target.task.id));
    assert_eq!(selection.anchor_id(), &anchor.task.id);
    assert_eq!(selection.anchor_index(), 3);
}

#[test]
fn ignores_hidden_marks_and_requires_a_visible_target() {
    let tasks = vec![task_list_item_with_id("visible", "task-visible")];
    let hidden = task_list_item_with_id("hidden", "task-hidden").task.id;
    let marked = BTreeSet::from([hidden]);

    let selection = TaskSelection::resolve(&tasks, &marked, Some(0)).unwrap();
    assert_eq!(selection.single_id(), Some(&tasks[0].task.id));
    assert!(TaskSelection::resolve(&tasks, &marked, None).is_none());
    assert!(TaskSelection::resolve(&[], &marked, Some(0)).is_none());
}
