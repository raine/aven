use super::*;

#[test]
fn detail_body_shows_epic_parent_relationship() {
    let mut item = detail_test_item();
    item.epic_parent = Some(crate::query::TaskDependencyLink {
        project_key: "app".to_string(),
        task_id: crate::test_support::task_id("epic-task-id"),
        display_ref: "APP-EPIC".to_string(),
        title: "Ship authentication reliability".to_string(),
        status: "active".to_string(),
        priority: "high".to_string(),
        unresolved: true,
    });

    let lines = detail_body_lines(&item, 60, None);

    assert_eq!(lines[0].to_string(), "EPIC PARENT");
    assert!(
        lines[1]
            .to_string()
            .starts_with(&format!("└─ {EPIC_MARKER} APP-EPIC"))
    );
    let marker = lines[1]
        .spans
        .iter()
        .find(|span| span.content == EPIC_MARKER)
        .expect("epic parent marker");
    assert_eq!(marker.style.fg, Some(theme::yellow()));
    assert!(
        lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join(" ")
            .contains("Ship authentication")
    );
}

#[test]
fn detail_epic_parent_relationship_wraps_to_width() {
    let mut item = detail_test_item();
    item.epic_parent = Some(crate::query::TaskDependencyLink {
        project_key: "app".to_string(),
        task_id: crate::test_support::task_id("epic-task-id"),
        display_ref: "APP-EPIC".to_string(),
        title: "A long epic title that must fit the sticky header".to_string(),
        status: "active".to_string(),
        priority: "high".to_string(),
        unresolved: true,
    });

    let lines = detail_body_lines(&item, 32, None);

    assert!(lines.iter().all(|line| line.width() <= 32));
    assert!(lines.len() > 2);
}

#[test]
fn detail_projection_orders_every_relationship_section() {
    let mut item = detail_test_item();
    item.task.is_epic = true;
    item.epic_parent = Some(crate::query::TaskDependencyLink {
        project_key: "app".to_string(),
        task_id: crate::test_support::task_id("epic-parent-id"),
        display_ref: "APP-EPIC".to_string(),
        title: "Parent epic".to_string(),
        status: "active".to_string(),
        priority: "high".to_string(),
        unresolved: true,
    });
    item.epic_children = vec![crate::query::TaskDependencyLink {
        project_key: "app".to_string(),
        task_id: crate::test_support::task_id("epic-child-id"),
        display_ref: "APP-CHILD".to_string(),
        title: "Child task".to_string(),
        status: "todo".to_string(),
        priority: "medium".to_string(),
        unresolved: true,
    }];
    item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];

    let sections = detail_interactive_rows(
        &item,
        100,
        40,
        Some(&DetailInlineImageContext::default()),
        &BTreeSet::new(),
    )
    .into_iter()
    .map(|row| row.target.section())
    .fold(Vec::new(), |mut sections, section| {
        if sections.last() != Some(&section) {
            sections.push(section);
        }
        sections
    });

    assert_eq!(
        sections,
        vec![
            DetailSection::EpicParent,
            DetailSection::EpicChildren,
            DetailSection::Attachments,
            DetailSection::Notes,
            DetailSection::DependsOn,
            DetailSection::Blocks,
        ]
    );
}

#[test]
fn detail_projection_expands_long_dependency_sections() {
    let mut item = detail_test_item();
    item.depends_on = (0..5)
        .map(|index| crate::query::TaskDependencyLink {
            project_key: "app".to_string(),
            task_id: crate::test_support::task_id(&format!("blocker-id-{index}")),
            display_ref: format!("APP-B{index}"),
            title: format!("blocker {index}"),
            status: "todo".to_string(),
            priority: "medium".to_string(),
            unresolved: true,
        })
        .collect();
    item.blocks.clear();

    let collapsed = detail_interactive_rows(&item, 80, 24, None, &BTreeSet::new());
    assert_eq!(
        collapsed
            .iter()
            .filter(|row| row.target.section() == DetailSection::DependsOn)
            .count(),
        4
    );
    assert!(matches!(
        collapsed.last().map(|row| &row.target),
        Some(DetailTargetId::Expand {
            section: DetailSection::DependsOn
        })
    ));

    let expanded = detail_interactive_rows(
        &item,
        80,
        24,
        None,
        &[DetailSection::DependsOn].into_iter().collect(),
    );
    assert_eq!(
        expanded
            .iter()
            .filter(|row| row.target.section() == DetailSection::DependsOn)
            .count(),
        6
    );
}

#[test]
fn detail_content_renders_dependency_tree() {
    let item = detail_test_item();
    let rendered = detail_content_lines(&item, 80, None)
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("WHY BLOCKED open=1 total=1"));
    assert!(rendered.contains("└─ ← APP-7KQ1"));
    assert!(rendered.contains("Ship auth service"));
    assert!(rendered.contains("WHAT THIS UNLOCKS open=1 total=1"));
    assert!(rendered.contains("└─ → APP-7KQ2"));
    assert!(rendered.contains("Write rollout notes"));
}

#[test]
fn detail_dependency_tree_caps_long_blockers() {
    let mut item = detail_test_item();
    item.depends_on = (0..5)
        .map(|index| crate::query::TaskDependencyLink {
            project_key: "app".to_string(),
            task_id: crate::test_support::task_id(&format!("blocker-id-{index}")),
            display_ref: format!("APP-B{index}"),
            title: format!("blocker {index}"),
            status: "todo".to_string(),
            priority: "medium".to_string(),
            unresolved: true,
        })
        .collect();
    item.blocks.clear();

    let rendered = detail_content_lines(&item, 80, None)
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("├─ ← APP-B0"));
    assert!(rendered.contains("├─ ← APP-B2"));
    assert!(rendered.contains("└─ Show 2 more"));
    assert!(!rendered.contains("APP-B3"));
}

#[test]
fn detail_dependency_tree_caps_long_dependents() {
    let mut item = detail_test_item();
    item.blocks = (0..5)
        .map(|index| crate::query::TaskDependencyLink {
            project_key: "app".to_string(),
            task_id: crate::test_support::task_id(&format!("dependent-id-{index}")),
            display_ref: format!("APP-D{index}"),
            title: format!("dependent {index}"),
            status: "inbox".to_string(),
            priority: "low".to_string(),
            unresolved: true,
        })
        .collect();
    item.depends_on.clear();

    let rendered = detail_content_lines(&item, 80, None)
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("├─ → APP-D0"));
    assert!(rendered.contains("├─ → APP-D2"));
    assert!(rendered.contains("└─ Show 2 more"));
    assert!(!rendered.contains("APP-D3"));
}

#[test]
fn detail_dependency_tree_truncates_titles_in_narrow_width() {
    let mut item = detail_test_item();
    item.depends_on[0].title = "A very long title that should fit the dependency row".to_string();

    let rendered = detail_content_lines(&item, 40, None);

    let blocker_line = rendered
        .iter()
        .find(|line| line.to_string().contains("APP-7KQ1"))
        .expect("blocker line rendered");

    assert!(
        blocker_line.width() <= 40,
        "line width {} exceeded 40",
        blocker_line.width()
    );
    assert!(blocker_line.to_string().contains('…'));
}

#[test]
fn detail_dependency_tree_stacks_when_narrow() {
    let mut item = detail_test_item();
    item.depends_on[0].title =
        "A very long title that should stack in the dependency tree".to_string();

    let rendered = detail_dependency_lines(&item, 20);
    let rendered_text = rendered
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered_text.contains("└─ ← APP-7KQ1"));
    assert!(rendered_text.contains("A very long tit…"));
    assert!(rendered_text.contains("□ todo  ● high"));
    for line in rendered.into_iter().filter(|line| {
        let text = line.to_string();
        text.contains("APP-") || text.contains("todo")
    }) {
        assert!(
            line.width() <= 20,
            "line width {} exceeded 20: {line:?}",
            line.width()
        );
    }
}

#[test]
fn detail_dependency_tree_is_omitted_without_links() {
    let mut item = detail_test_item();
    item.depends_on.clear();
    item.blocks.clear();

    let rendered = detail_content_lines(&item, 60, None)
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!rendered.contains("WHY BLOCKED"));
    assert!(!rendered.contains("WHAT THIS UNLOCKS"));
}

#[test]
fn detail_content_lists_epic_children() {
    let item = detail_test_epic_item();

    let lines = detail_content_lines(&item, 80, None)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let rendered = lines.join("\n");
    let child_heading_index = lines
        .iter()
        .position(|line| line.contains("CHILD TASKS"))
        .unwrap();

    assert_eq!(lines[child_heading_index.saturating_sub(1)], "");
    assert!(lines[child_heading_index.saturating_sub(2)].contains("active"));

    assert!(rendered.contains("CHILD TASKS open=1 total=2"));
    assert!(rendered.contains("├─ APP-CHLD"));
    assert!(rendered.contains("Build the first child task"));
    assert!(rendered.contains("└─ APP-DONE"));
    assert!(rendered.contains("Finished child task"));
    assert!(!rendered.contains("blocked by"));
    assert!(
        rendered.find("CHILD TASKS").unwrap()
            < rendered.find("Two token refresh requests").unwrap()
    );
}

#[test]
fn epic_children_show_shared_blocker_without_changing_membership_tree() {
    let mut item = detail_test_epic_item();
    item.epic_children[1].status = "todo".to_string();
    item.epic_children[1].unresolved = true;
    let blocker = crate::query::TaskDependencyLink {
        project_key: "app".to_string(),
        task_id: crate::test_support::task_id("shared-blocker-id"),
        display_ref: "APP-BLKR".to_string(),
        title: "Prepare the shared environment".to_string(),
        status: "active".to_string(),
        priority: "urgent".to_string(),
        unresolved: true,
    };
    for child in &item.epic_children {
        item.epic_child_dependencies
            .insert(child.task_id.clone(), vec![blocker.clone()]);
    }

    let lines = detail_content_lines(&item, 80, None)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let first_child = lines
        .iter()
        .position(|line| line.contains("APP-CHLD"))
        .unwrap();
    let second_child = lines
        .iter()
        .position(|line| line.contains("APP-DONE"))
        .unwrap();

    assert_eq!(lines[first_child + 1], "│  ← blocked by APP-BLKR");
    assert_eq!(lines[second_child + 1], "   ← blocked by APP-BLKR");
    assert!(first_child < second_child);
}

#[test]
fn epic_child_blockers_use_compact_clipped_rows_at_narrow_widths() {
    let mut item = detail_test_epic_item();
    let child_id = item.epic_children[0].task_id.clone();
    item.epic_child_dependencies.insert(
        child_id,
        vec![crate::query::TaskDependencyLink {
            project_key: "app".to_string(),
            task_id: crate::test_support::task_id("narrow-blocker-id"),
            display_ref: "APP-BLOCKER-LONG".to_string(),
            title: "Prepare the environment".to_string(),
            status: "active".to_string(),
            priority: "urgent".to_string(),
            unresolved: true,
        }],
    );

    let dependency_lines = detail_content_lines(&item, 18, None)
        .into_iter()
        .map(|line| line.to_string())
        .filter(|line| line.starts_with("│  ←"))
        .collect::<Vec<_>>();

    assert_eq!(dependency_lines.len(), 1);
    assert_eq!(dependency_lines[0], "│  ← 1 blocker");
    assert!(!dependency_lines[0].contains("blocked by"));
    assert!(
        dependency_lines.iter().all(|line| line.width() <= 18),
        "dependency row exceeded content width: {dependency_lines:?}"
    );
}

#[test]
fn epic_child_blockers_stay_on_one_row_and_count_hidden_refs() {
    let mut blockers = (0..10)
        .map(|index| crate::query::TaskDependencyLink {
            project_key: "app".to_string(),
            task_id: crate::test_support::task_id(&format!("blocker-{index}")),
            display_ref: format!("APP-B{index}"),
            title: format!("Blocker {index}"),
            status: "todo".to_string(),
            priority: "high".to_string(),
            unresolved: true,
        })
        .collect::<Vec<_>>();
    blockers[9].unresolved = false;
    let lines = epic_child_dependency_lines(&blockers, false, 80, false);
    assert_eq!(lines.len(), 1);
    assert_eq!(
        lines[0].to_string(),
        "│  ← blocked by APP-B0, APP-B1 +7 more"
    );
    for width in [0, 5, 18, 32, 40, 80] {
        let lines = epic_child_dependency_lines(&blockers, true, width, true);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].width() <= width);
        assert!(
            lines[0]
                .spans
                .iter()
                .all(|span| span.style.bg == Some(theme::bg_panel()))
        );
    }
    for blocker in &mut blockers {
        blocker.unresolved = false;
    }
    assert!(epic_child_dependency_lines(&blockers, false, 80, false).is_empty());
}

#[test]
fn epic_child_dependency_rows_share_the_child_hit_target() {
    let mut item = detail_test_epic_item();
    let child_id = item.epic_children[0].task_id.clone();
    item.epic_child_dependencies.insert(
        child_id.clone(),
        vec![crate::query::TaskDependencyLink {
            project_key: "app".to_string(),
            task_id: crate::test_support::task_id("hit-blocker-id"),
            display_ref: "APP-BLKR".to_string(),
            title: "Prepare the environment".to_string(),
            status: "active".to_string(),
            priority: "urgent".to_string(),
            unresolved: true,
        }],
    );
    let children = detail_epic_children(&item, None);
    let body = build_detail_body_document(&item, &children, 80, &BTreeSet::new(), None, &[]);
    let child_row = body
        .interactive_rows
        .iter()
        .find(|row| {
            row.target
                == DetailTargetId::Task {
                    section: DetailSection::EpicChildren,
                    task_id: child_id.clone(),
                }
        })
        .expect("child interaction row");

    assert_eq!(child_row.height, 2);
}

#[test]
fn removed_epic_child_is_labeled_and_excluded_from_counts() {
    let mut item = detail_test_epic_item();
    let child = item.epic_children.remove(0);
    item.epic_children.clear();
    let removed = crate::tui::app::RemovedEpicChild {
        epic_id: item.task.id.clone(),
        child,
        original_position: 0,
    };
    let children = detail_epic_children(&item, Some(&removed));
    let body = build_detail_body_document(&item, &children, 80, &BTreeSet::new(), None, &[]);
    let rendered = body
        .lines
        .iter()
        .map(Line::to_string)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("CHILD TASKS open=0 total=0"));
    assert!(rendered.contains("Build the first child task  [removed]"));
    assert_eq!(children[0].state, EpicChildState::Removed);
}

#[test]
fn linked_references_use_their_own_project_color_and_dim_suffix() {
    let mut item = detail_test_epic_item();
    item.epic_children[0].project_key = "different-project".to_string();
    item.depends_on = vec![item.epic_children[0].clone()];
    item.blocks = item.depends_on.clone();
    let children = detail_epic_children(&item, None);
    let body = build_detail_body_document(&item, &children, 120, &BTreeSet::new(), None, &[]);
    let rows: Vec<_> = body
        .lines
        .iter()
        .filter(|line| line.to_string().contains("APP-CHLD"))
        .collect();
    assert_eq!(rows.len(), 3);
    for row in rows {
        let prefix = row.spans.iter().find(|span| span.content == "APP").unwrap();
        let suffix = row
            .spans
            .iter()
            .find(|span| span.content == "CHLD")
            .unwrap();
        assert_eq!(
            prefix.style.fg,
            Some(theme::project_color("different-project"))
        );
        assert_eq!(suffix.style.fg, Some(theme::fg_dim()));
    }
}

#[test]
fn legitimate_removed_suffix_remains_a_live_epic_child_title() {
    let mut item = detail_test_epic_item();
    item.epic_children.truncate(1);
    item.epic_children[0].title = "Investigate literal [removed]".to_string();

    let children = detail_epic_children(&item, None);
    let body = build_detail_body_document(&item, &children, 80, &BTreeSet::new(), None, &[]);
    let child_line = body
        .lines
        .iter()
        .find(|line| line.to_string().contains("APP-CHLD"))
        .expect("child row");
    let child_ref = child_line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "APP")
        .expect("child ref");
    let rendered = body
        .lines
        .iter()
        .map(Line::to_string)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("CHILD TASKS open=1 total=1"));
    assert!(rendered.contains("Investigate literal [removed]"));
    assert_eq!(children[0].state, EpicChildState::Live);
    assert_eq!(child_ref.style.fg, Some(theme::project_color("app")));
    assert!(!child_ref.style.add_modifier.contains(Modifier::DIM));
}

#[test]
fn detail_child_hit_maps_child_rows() {
    let item = detail_test_epic_item();

    let hit = detail_child_task_at_position(&item, 120, 30, 4, 8, 0).unwrap();

    assert_eq!(
        hit.task_id.as_str(),
        crate::test_support::task_id("child-task-id").as_str()
    );
}

#[test]
fn hovered_detail_child_uses_link_style() {
    let item = detail_test_epic_item();
    let hovered = crate::test_support::task_id("child-task-id");
    let lines = detail_body_lines(&item, 80, Some(hovered.as_str()));
    let line = lines
        .iter()
        .find(|line| line.to_string().contains("APP-CHLD"))
        .unwrap();
    let ref_span = line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "APP")
        .unwrap();

    assert_eq!(ref_span.style.bg, Some(theme::bg_panel()));
}

#[test]
fn related_detail_rows_hide_deleted_targets_for_live_tasks() {
    let mut item = detail_test_item();
    let live_id = crate::test_support::task_id("live-related-task");
    let deleted_id = crate::test_support::task_id("deleted-related-task");
    item.related = vec![
        crate::query::TaskRelatedLink {
            project_key: "app".to_string(),
            task_id: live_id.clone(),
            display_ref: "APP-LIVE".to_string(),
            title: "Live related".to_string(),
            status: TaskStatus::Todo,
            priority: TaskPriority::None,
            deleted: false,
            linked_at: "2026-08-22T00:00:00Z".to_string(),
        },
        crate::query::TaskRelatedLink {
            project_key: "app".to_string(),
            task_id: deleted_id.clone(),
            display_ref: "APP-DEAD".to_string(),
            title: "Deleted related".to_string(),
            status: TaskStatus::Done,
            priority: TaskPriority::Low,
            deleted: true,
            linked_at: "2026-08-22T00:00:01Z".to_string(),
        },
    ];

    let related_targets = |item: &TaskListItem| {
        detail_interactive_rows(item, 100, 40, None, &BTreeSet::new())
            .into_iter()
            .filter_map(|row| match row.target {
                DetailTargetId::Task {
                    section: DetailSection::Related,
                    task_id,
                } => Some(task_id),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(related_targets(&item), vec![live_id.clone()]);

    item.task.deleted = true;
    assert_eq!(related_targets(&item), vec![live_id, deleted_id]);
}
