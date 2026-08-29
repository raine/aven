use super::*;

fn dialog_height(state: AddTaskView) -> u16 {
    let buffer = overlay_buffer(add_task_overlay(state));
    let top = (0..buffer.area.height)
        .find(|row| buffer_row(&buffer, *row).contains("╭─ Add task "))
        .expect("add task top border");
    let bottom = (top..buffer.area.height)
        .rev()
        .find(|row| buffer_row(&buffer, *row).contains('╰'))
        .expect("add task bottom border");
    bottom - top + 1
}

#[test]
fn add_task_overlay_starts_compact() {
    assert_eq!(dialog_height(add_task_view()), 14);
}

#[test]
fn add_task_overlay_grows_for_wrapped_description() {
    let compact_height = dialog_height(add_task_view());
    let expanded_height = dialog_height(AddTaskView {
        description: borrow_slice(vec!["description ".repeat(40)]),
        ..add_task_view()
    });

    assert!(expanded_height > compact_height);
}

#[test]
fn add_task_overlay_keeps_space_above_shortcuts() {
    let buffer = overlay_buffer(add_task_overlay(AddTaskView {
        description: borrow_slice(vec!["last description line".to_string()]),
        focus: AddTaskStep::Description,
        ..add_task_view()
    }));
    let description_row = (0..buffer.area.height)
        .find(|row| buffer_row(&buffer, *row).contains("last description line"))
        .expect("description row");
    let footer_row = (description_row..buffer.area.height)
        .find(|row| buffer_row(&buffer, *row).contains("Ctrl-Enter"))
        .expect("footer row");

    assert!(footer_row >= description_row + 2);
}

#[test]
fn add_task_overlay_renders_metadata_fields_and_footer() {
    let rendered = render_overlay_view(add_task_overlay(AddTaskView {
        title: "ship dialogs".to_string(),
        title_cursor: 12,
        priority: "high".to_string(),
        ..add_task_view()
    }));
    assert!(rendered.contains("Add task"));
    assert!(rendered.contains("Project: ● aven"));
    assert!(rendered.contains("Status: ▣ inbox"));
    assert!(rendered.contains("Priority: ● high"));
    assert!(rendered.contains("Labels: none"));
    assert!(rendered.contains("Epic: no"));
    assert!(!rendered.contains("Create more"));
    assert!(rendered.contains("Schedule: none"));
    assert!(rendered.contains("Title"));
    assert!(rendered.contains("Description"));
    assert!(rendered.find("Schedule: none").unwrap() < rendered.find("Title").unwrap());
    assert!(rendered.contains("  ship dialogs"));
    assert!(rendered.contains("Optional details, links, or handoff context..."));
    assert!(rendered.contains("Tab next"));
    assert!(rendered.contains("^G create more"));
    assert!(rendered.contains("F1 help"));
}

#[test]
fn metadata_grid_keeps_three_balanced_columns_above_title() {
    let buffer = overlay_buffer(add_task_overlay(add_task_view()));
    let metadata_row = (0..buffer.area.height)
        .map(|row| buffer_row(&buffer, row))
        .find(|row| row.contains("Schedule: none"))
        .expect("second metadata row");

    let labels = metadata_row.find("Labels:").expect("labels column");
    let schedule = metadata_row.find("Schedule:").expect("schedule column");
    let epic = metadata_row.find("Epic:").expect("epic column");
    assert!(labels < schedule);
    assert!(schedule < epic);
    assert!(
        (0..buffer.area.height)
            .map(|row| buffer_row(&buffer, row))
            .any(|row| row.contains("Title"))
    );
}

#[test]
fn add_task_overlay_marks_automatic_status() {
    let rendered = render_overlay_view(add_task_overlay(AddTaskView {
        status: "todo".to_string(),
        status_automatic: true,
        ..add_task_view()
    }));

    assert!(rendered.contains("todo (auto)"));
}

#[test]
fn schedule_fields_align_with_the_metadata_grid() {
    let buffer = overlay_buffer(add_task_overlay(AddTaskView {
        available_at: "tomorrow".to_string(),
        ..add_task_view()
    }));
    let status_row = (0..buffer.area.height)
        .map(|row| buffer_row(&buffer, row))
        .find(|row| row.contains("Status:"))
        .unwrap();
    let schedule_row = (0..buffer.area.height)
        .map(|row| buffer_row(&buffer, row))
        .find(|row| row.contains("Schedule:"))
        .unwrap();

    let cell_position = |row: &str, label: &str| {
        let byte = row.find(label).unwrap();
        unicode_width::UnicodeWidthStr::width(&row[..byte])
    };
    assert_eq!(
        cell_position(&status_row, "Status:"),
        cell_position(&schedule_row, "Schedule:")
    );
}

#[test]
fn composer_help_expands_the_parent_dialog() {
    let compact_height = dialog_height(add_task_view());
    let help_height = dialog_height(AddTaskView {
        mode: borrow_value(AddTaskMode::Help { scroll: 0 }),
        ..add_task_view()
    });

    assert!(help_height > compact_height);
}

#[test]
fn structured_schedule_editor_preserves_composer_height() {
    let compact_height = dialog_height(add_task_view());
    let structured_height = dialog_height(AddTaskView {
        mode: borrow_value(AddTaskMode::Schedule(schedule_editor(
            ScheduleEditorMode::Once,
        ))),
        ..add_task_view()
    });

    assert_eq!(structured_height, compact_height);
}

#[test]
fn nested_controls_preserve_composer_height() {
    let compact_height = dialog_height(add_task_view());
    let picker = PickerState::new(
        PickerIntent::AddTaskPriority,
        "Add task: priority",
        Vec::new(),
        false,
    );
    let OverlayState::TagCombobox(labels) = OverlayState::tag_combobox(
        TagComboboxIntent::AddTaskLabels,
        "Add task: labels",
        Vec::new(),
        Vec::new(),
    ) else {
        panic!("expected labels control");
    };

    for mode in [
        AddTaskMode::Picker {
            field: AddTaskStep::Priority,
            state: picker,
        },
        AddTaskMode::Labels(labels),
        AddTaskMode::ConfirmDiscard,
    ] {
        let child_height = dialog_height(AddTaskView {
            mode: borrow_value(mode),
            ..add_task_view()
        });
        assert_eq!(child_height, compact_height);
    }
}

#[test]
fn nested_schedule_editor_dims_only_its_underlay() {
    let buffer = overlay_buffer(add_task_overlay(AddTaskView {
        mode: borrow_value(AddTaskMode::Schedule(schedule_editor(
            ScheduleEditorMode::Repeat,
        ))),
        ..add_task_view()
    }));
    let text_position = |needle: &str| {
        let chars = needle.chars().collect::<Vec<_>>();
        (0..buffer.area.height)
            .find_map(|row| {
                (0..buffer.area.width.saturating_sub(chars.len() as u16)).find_map(|column| {
                    chars
                        .iter()
                        .enumerate()
                        .all(|(offset, ch)| {
                            buffer[(column + offset as u16, row)].symbol() == ch.to_string()
                        })
                        .then_some((column, row))
                })
            })
            .unwrap_or_else(|| panic!("missing {needle:?}"))
    };

    assert!(
        buffer[text_position("Project:")]
            .modifier
            .contains(Modifier::DIM)
    );
    assert!(
        !buffer[text_position("Type")]
            .modifier
            .contains(Modifier::DIM)
    );
}

#[test]
fn nested_schedule_editor_leaves_the_outer_surface_unchanged() {
    let background = ratatui::style::Color::Rgb(100, 80, 60);
    let overlay = add_task_overlay(AddTaskView {
        mode: borrow_value(AddTaskMode::Schedule(schedule_editor(
            ScheduleEditorMode::Repeat,
        ))),
        ..add_task_view()
    });
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).unwrap();

    terminal
        .draw(|frame| {
            let area = frame.area();
            frame.render_widget(
                ratatui::widgets::Block::new().style(ratatui::style::Style::new().bg(background)),
                area,
            );
            render_non_help_overlay_content(frame, &overlay);
        })
        .unwrap();

    assert_eq!(terminal.backend().buffer()[(0, 0)].bg, background);
}

#[test]
fn schedule_editor_keeps_the_composer_compact() {
    let default = overlay_buffer(add_task_overlay(add_task_view()));
    let configured = overlay_buffer(add_task_overlay(AddTaskView {
        schedule_input: "every Friday at 09:00, due same day".to_string(),
        repeat_rule: "every Friday".to_string(),
        repeat_at: "09:00".to_string(),
        ..add_task_view()
    }));
    let title_row = |buffer: &ratatui::buffer::Buffer| {
        (0..buffer.area.height)
            .position(|row| buffer_row(buffer, row).contains("Add task"))
            .unwrap()
    };

    assert_eq!(title_row(&default), title_row(&configured));
}

#[test]
fn schedule_field_renders_natural_language_settings() {
    let rendered = render_overlay_view(add_task_overlay(AddTaskView {
        schedule_input: "available tomorrow, due next Friday".to_string(),
        available_at: "tomorrow".to_string(),
        due_on: "next Friday".to_string(),
        ..add_task_view()
    }));
    assert!(rendered.contains("Schedule: available"));
    assert!(!rendered.contains("Available:"));
}

#[test]
fn schedule_hit_testing_uses_the_summary_row() {
    let terminal = ratatui::layout::Rect::new(0, 0, 120, 30);
    let state = add_task_view();
    let outer = crate::tui::overlay::dialog_area(terminal, 100, 14);
    let schedule_row = outer.y + 2;
    for column in [60, 70] {
        assert_eq!(
            add_task_field_at(
                terminal,
                false,
                AddTaskLayout {
                    description: state.description,
                    mode: state.mode,
                    has_attachments: false,
                    show_schedule_error: false,
                },
                column,
                schedule_row,
            ),
            Some(AddTaskStep::Schedule)
        );
    }
}

#[test]
fn schedule_focus_footer_explains_direct_input_and_details() {
    let rendered = render_overlay_view(add_task_overlay(AddTaskView {
        focus: AddTaskStep::Schedule,
        ..add_task_view()
    }));

    assert!(rendered.contains("type schedule"));
    assert!(rendered.contains("Enter details"));
    assert!(rendered.contains("^A available"));
    assert!(rendered.contains("^U due"));
}

#[test]
fn focused_schedule_field_teaches_natural_input() {
    let rendered = render_overlay_view(add_task_overlay(AddTaskView {
        focus: AddTaskStep::Schedule,
        ..add_task_view()
    }));

    assert!(rendered.contains("type schedule"));
    assert!(rendered.contains("Enter details"));
}

#[test]
fn focused_schedule_field_waits_to_show_validation() {
    let draft = AddTaskView {
        focus: AddTaskStep::Schedule,
        schedule_input: "d".to_string(),
        schedule_error: Some("invalid schedule".to_string()),
        ..add_task_view()
    };
    let editing = render_overlay_view(add_task_overlay(draft.clone()));
    let blurred = render_overlay_view(add_task_overlay(AddTaskView {
        schedule_validation_requested: true,
        ..draft
    }));

    assert!(!editing.contains("Schedule: Try tomorrow"));
    assert!(blurred.contains("Schedule: Try tomorrow"));
}

#[test]
fn malformed_recurrence_shows_recurrence_guidance() {
    let rendered = render_overlay_view(add_task_overlay(AddTaskView {
        focus: AddTaskStep::Project,
        schedule_input: "every 0 days".to_string(),
        schedule_error: Some("invalid recurrence".to_string()),
        schedule_validation_requested: true,
        ..add_task_view()
    }));

    assert!(rendered.contains("Schedule: Try daily"));
    assert!(!rendered.contains("Schedule: Try tomorrow"));
}

#[test]
fn add_task_metadata_values_use_shared_styles() {
    let project = metadata_field(AddTaskStep::Project, "Project", "aven", AddTaskStep::Title);
    assert_eq!(project.to_string(), "  ^P Project: ● aven");
    assert_eq!(
        project.spans[3].style.fg,
        Some(theme::project_color("aven"))
    );

    let status = metadata_field(AddTaskStep::Status, "Status", "active", AddTaskStep::Title);
    assert_eq!(status.to_string(), "  ^T Status: ● active");
    assert_eq!(status.spans[3].style.fg, theme::status_style("active").fg);

    let priority = metadata_field(
        AddTaskStep::Priority,
        "Priority",
        "medium",
        AddTaskStep::Title,
    );
    assert_eq!(priority.to_string(), "  ^R Priority: ◐ med");
    assert_eq!(
        priority.spans[3].style.fg,
        theme::priority_style("medium").fg
    );
    assert!(!priority.to_string().contains('['));

    let availability = metadata_field(
        AddTaskStep::AvailableAt,
        "Available",
        "Now",
        AddTaskStep::Title,
    );
    assert_eq!(availability.to_string(), "  ^A Available: Now");
}

#[test]
fn add_task_wide_medium_and_narrow_layouts_keep_every_field() {
    for (width, height) in [(120, 30), (80, 24), (42, 24), (50, 12)] {
        let rendered = render_overlay_view_at(
            add_task_overlay(AddTaskView {
                focus: AddTaskStep::Labels,
                ..add_task_view()
            }),
            width,
            height,
        );
        for label in [
            "Project",
            "Status",
            "Priority",
            "Labels",
            "Schedule",
            "Title",
            "Description",
        ] {
            assert!(
                rendered.contains(label),
                "missing {label} at {width}x{height}"
            );
        }
        assert!(
            rendered.contains("▶ ^L Labels"),
            "missing focus cue at {width}x{height}"
        );
        for shortcut in ["^P", "^T", "^R", "^L"] {
            assert!(
                rendered.contains(shortcut),
                "missing {shortcut} at {width}x{height}"
            );
        }
    }
}

#[test]
fn structured_schedule_dialog_stays_coherent_across_responsive_layouts() {
    for (width, height) in [(120, 30), (80, 24), (42, 30)] {
        let rendered = render_overlay_view_at(
            add_task_overlay(AddTaskView {
                mode: borrow_value(AddTaskMode::Schedule(schedule_editor(
                    ScheduleEditorMode::Repeat,
                ))),
                ..add_task_view()
            }),
            width,
            height,
        );
        for label in [
            "Schedule",
            "Type",
            "Available",
            "Due",
            "Repeat",
            "Starts",
            "Next",
        ] {
            assert!(
                rendered.contains(label),
                "missing {label} at {width}x{height}"
            );
        }
    }
}

#[test]
fn add_task_wide_layout_bounds_long_metadata_values() {
    let rendered = render_overlay_view_at(
        add_task_overlay(AddTaskView {
            project: "telegram-tori-bot-with-a-long-name".to_string(),
            status: "canceled".to_string(),
            labels: borrow_slice(vec!["feature".to_string(), "urgent".to_string()]),
            ..add_task_view()
        }),
        160,
        30,
    );

    assert!(
        rendered.contains("Project: ● tele"),
        "missing bounded project value:\n{rendered}"
    );
    assert!(rendered.contains('…'));
    assert!(rendered.contains("^T Status:"));
    assert!(rendered.contains("^R Priority:"));
    assert!(rendered.contains("^L Labels: featu"));
}

#[test]
fn add_task_validation_and_help_are_visible() {
    let validation = render_overlay_view(add_task_overlay(AddTaskView {
        title_error: true,
        ..add_task_view()
    }));
    assert!(validation.contains("Title is required"));

    let help = render_overlay_view(add_task_overlay(AddTaskView {
        mode: borrow_value(crate::tui::overlay::AddTaskMode::Help { scroll: 0 }),
        ..add_task_view()
    }));
    assert!(help.contains("Composer help"));
    assert!(help.contains("Shift+Tab"));
    assert!(help.contains("Ctrl-g"));
    assert!(help.contains("one-off Due"));
    assert!(help.contains("Schedule editor"));
    assert!(help.contains("↑/↓ move"));
    assert!(help.contains("aven.raine.dev/tui/#capture-tasks"));
}

#[test]
fn inactive_composer_headers_stay_distinct_from_placeholders() {
    let header = add_task_field_label("Title", false);
    let placeholder = add_task_title_input_line("", None, 40);

    assert_eq!(header.spans[1].style.fg, Some(crate::tui::theme::FG_MUTED));
    assert!(
        header.spans[1]
            .style
            .add_modifier
            .contains(ratatui::style::Modifier::BOLD)
    );
    assert_eq!(placeholder.spans[0].style.fg, Some(FG_DIM));
    assert!(
        !placeholder.spans[0]
            .style
            .add_modifier
            .contains(ratatui::style::Modifier::BOLD)
    );
}

#[test]
fn composer_help_scrolls_with_a_stable_dialog_and_scrollbar() {
    let top = render_overlay_view_at(
        add_task_overlay(AddTaskView {
            mode: borrow_value(AddTaskMode::Help { scroll: 0 }),
            ..add_task_view()
        }),
        100,
        20,
    );
    let bottom = render_overlay_view_at(
        add_task_overlay(AddTaskView {
            mode: borrow_value(AddTaskMode::Help {
                scroll: composer_help_scroll_cap(20, false, false),
            }),
            ..add_task_view()
        }),
        100,
        20,
    );

    let top_border = top.lines().position(|line| line.contains("Composer help"));
    let bottom_border = bottom
        .lines()
        .position(|line| line.contains("Composer help"));
    let top_end = top.lines().position(|line| line.contains('╰'));
    let bottom_end = bottom.lines().position(|line| line.contains('╰'));
    assert_eq!(top_border, bottom_border);
    assert_eq!(top_end, bottom_end);
    assert!(top.contains('▲'));
    assert!(top.contains('▼'));
    assert!(top.contains("Tab / Shift+Tab"));
    assert!(!top.contains("confirm discard"));
    assert!(!bottom.contains("Tab / Shift+Tab"));
    assert!(bottom.contains("confirm discard"));
    assert!(bottom.contains("j/k scroll"));
}

#[test]
fn composer_help_scroll_cap_matches_add_task_layout() {
    assert_eq!(composer_help_scroll_cap(20, false, false), 10);
    assert_eq!(composer_help_scroll_cap(20, false, true), 8);
    assert_eq!(composer_help_scroll_cap(20, true, false), 6);
    assert_eq!(composer_help_scroll_cap(30, false, false), 5);
}

#[test]
fn composer_help_uses_muted_keys_and_dim_descriptions() {
    let line = composer_help_line("Ctrl-a", "jump to availability");
    let long = composer_help_line("Ctrl-Enter / Ctrl-s", "create from any field");
    let docs = composer_help_line("Docs", "https://aven.raine.dev/tui/#capture-tasks");

    assert_eq!(line.spans[0].style.fg, Some(crate::tui::theme::FG_MUTED));
    assert_eq!(line.spans[1].style.fg, Some(FG_DIM));
    assert!(long.to_string().contains("Ctrl-s   create"));
    assert_eq!(docs.spans[0].style.fg, Some(ACCENT));
    assert_eq!(docs.spans[1].style.fg, Some(ACCENT));
    assert!(
        docs.spans[1]
            .style
            .add_modifier
            .contains(ratatui::style::Modifier::UNDERLINED)
    );
}

#[test]
fn add_task_child_modes_use_shared_dialog_and_control_styles() {
    let mut picker = PickerState::new(
        PickerIntent::AddTaskPriority,
        "Add task: priority",
        vec![
            PickerItem {
                label: "none".to_string(),
                value: "none".to_string(),
                selected: false,
            },
            PickerItem {
                label: "high".to_string(),
                value: "high".to_string(),
                selected: true,
            },
        ],
        false,
    );
    picker.filter.text = "hi".to_string();
    picker.filter.cursor = 2;
    let rendered = render_overlay_view(add_task_overlay(AddTaskView {
        mode: borrow_value(AddTaskMode::Picker {
            field: AddTaskStep::Priority,
            state: picker,
        }),
        ..add_task_view()
    }));
    assert!(rendered.contains("╭─ Add task: priority"));
    assert!(rendered.contains("▸"));
    assert!(rendered.contains("Enter submit"));
    assert!(!rendered.contains("/hi"));

    let OverlayState::TagCombobox(labels) = OverlayState::tag_combobox(
        TagComboboxIntent::AddTaskLabels,
        "Add task: labels",
        vec!["feature".to_string()],
        vec!["feature".to_string()],
    ) else {
        panic!("expected labels control");
    };
    let rendered = render_overlay_view(add_task_overlay(AddTaskView {
        mode: borrow_value(AddTaskMode::Labels(labels)),
        ..add_task_view()
    }));
    assert!(rendered.contains("╭─ Add task: labels"));
    assert!(rendered.contains("feature"));
    assert!(rendered.contains("Enter add/save"));

    let rendered = render_overlay_view(add_task_overlay(AddTaskView {
        mode: borrow_value(AddTaskMode::ConfirmDiscard),
        ..add_task_view()
    }));
    assert!(rendered.contains("╭─ Discard draft?"));
    assert!(rendered.contains("Discard this task draft?"));
    assert!(rendered.contains("y yes"));
    assert!(rendered.contains("n no"));
    assert!(rendered.contains("Esc cancel"));
    assert!(!rendered.contains("This draft has content"));
}

#[test]
fn add_task_overlay_shows_pending_images() {
    let empty = render_overlay_view(add_task_overlay(add_task_view()));
    assert!(!empty.contains("Images ("));

    let rendered = render_overlay_view(add_task_overlay(AddTaskView {
        attachments: Box::new(AddTaskAttachmentsView {
            items: borrow_slice(vec![
                pending_attachment("diagram.png", 2048, Some((800, 600))),
                pending_attachment("screenshot.png", 1536, Some((1440, 900))),
            ]),
            selected: 0,
        }),
        ..add_task_view()
    }));
    assert!(rendered.contains("Images (2)  1/2 diagram.png · 800×600 · 2.0 KiB"));
    assert!(!rendered.contains("D remove"));

    let focused = render_overlay_view(add_task_overlay(AddTaskView {
        attachments: Box::new(AddTaskAttachmentsView {
            items: borrow_slice(vec![
                pending_attachment("diagram.png", 2048, Some((800, 600))),
                pending_attachment("screenshot.png", 1536, Some((1440, 900))),
            ]),
            selected: 1,
        }),
        focus: AddTaskStep::Images,
        ..add_task_view()
    }));
    assert!(focused.contains("▶ Images (2)  ◀ 2/2 screenshot.png · 1440×900 · 1.5 KiB ▶"));
    assert!(focused.contains("D remove"));

    let buffer = overlay_buffer(add_task_overlay(AddTaskView {
        attachments: Box::new(AddTaskAttachmentsView {
            items: borrow_slice(vec![pending_attachment(
                "diagram.png",
                2048,
                Some((800, 600)),
            )]),
            selected: 0,
        }),
        ..add_task_view()
    }));
    let image_row = (0..buffer.area.height)
        .find(|row| buffer_row(&buffer, *row).contains("Images (1)"))
        .unwrap();
    let image_line = buffer_row(&buffer, image_row);
    assert!(image_line.contains("diagram.png · 800×600 · 2.0 KiB"));
    assert!(!image_line.contains("1/1"));
    let title_row = (0..buffer.area.height)
        .find(|row| buffer_row(&buffer, *row).contains("Title"))
        .unwrap();
    assert_eq!(title_row, image_row + 2);
}

#[test]
fn add_task_overlay_pins_footer_to_bottom() {
    let buffer = overlay_buffer(add_task_overlay(AddTaskView {
        focus: AddTaskStep::Description,
        ..add_task_view()
    }));
    let hint_row = (0..buffer.area.height)
        .find(|row| buffer_row(&buffer, *row).contains("Ctrl-Enter / ^S create"))
        .unwrap();
    let bottom_border_row = (0..buffer.area.height)
        .rev()
        .find(|row| buffer_row(&buffer, *row).contains("╰"))
        .unwrap();
    assert_eq!(hint_row + 1, bottom_border_row);
}

#[test]
fn add_task_overlay_does_not_truncate_title_hints() {
    let rendered = render_overlay_view(add_task_overlay(AddTaskView { ..add_task_view() }));
    assert!(rendered.contains("Esc cancel"));
}

#[test]
fn add_task_overlay_does_not_truncate_description_hints() {
    let rendered = render_overlay_view(add_task_overlay(AddTaskView {
        focus: AddTaskStep::Description,
        ..add_task_view()
    }));
    assert!(rendered.contains("Esc cancel"));
}

#[test]
fn add_task_overlay_replaces_footer_when_status_prefix_is_active() {
    let rendered = render_overlay_view(add_task_overlay(AddTaskView {
        status_prefix_active: true,
        ..add_task_view()
    }));
    assert!(rendered.contains("i inbox"));
    assert!(rendered.contains("a active"));
    assert!(rendered.contains("Esc cancel"));
    assert!(!rendered.contains("Enter create"));
    assert!(!rendered.contains("^P project"));
}

#[test]
fn add_task_overlay_replaces_footer_when_priority_prefix_is_active() {
    let rendered = render_overlay_view(add_task_overlay(AddTaskView {
        priority_prefix_active: true,
        ..add_task_view()
    }));
    assert!(rendered.contains("n none"));
    assert!(rendered.contains("h high"));
    assert!(rendered.contains("Esc cancel"));
    assert!(!rendered.contains("Enter create"));
    assert!(!rendered.contains("^P project"));
}

#[test]
fn add_task_overlay_omits_title_placeholder_cursor_when_description_focused() {
    let buffer = overlay_buffer(add_task_overlay(AddTaskView {
        description: borrow_slice(vec!["details".to_string()]),
        description_column: 7,
        focus: AddTaskStep::Description,
        ..add_task_view()
    }));
    let title_row = (0..buffer.area.height)
        .find(|row| buffer_row(&buffer, *row).contains(ADD_TASK_TITLE_PLACEHOLDER))
        .unwrap();
    let row = buffer_row(&buffer, title_row);
    assert!(row.contains(ADD_TASK_TITLE_PLACEHOLDER));
    for column in 0..buffer.area.width {
        assert_ne!(buffer[(column, title_row)].style().bg, Some(FG));
    }
}

#[test]
fn add_task_description_wraps_and_marks_hidden_rows() {
    let lines = add_task_description_lines(
        &AddTaskView {
            description: borrow_slice(vec!["abcdefghijklmnopqrstuvwxyz".to_string()]),
            description_column: 25,
            focus: AddTaskStep::Description,
            ..add_task_view()
        },
        2,
        12,
    );

    assert_eq!(lines.len(), 2);
    assert!(lines[0].to_string().starts_with("↑ "));
    assert!(lines[0].to_string().contains("klmnopqrst"));
    assert!(lines[1].to_string().contains("uvwxyz"));
    assert!(!lines[0].to_string().contains("abcdefghij"));
}

#[test]
fn add_task_description_unfocused_preview_starts_at_top() {
    let lines = add_task_description_lines(
        &AddTaskView {
            description: borrow_slice(vec!["abcdefghijklmnopqrstuvwxyz".to_string()]),
            description_column: 25,
            ..add_task_view()
        },
        2,
        12,
    );

    assert!(lines[0].to_string().contains("abcdefghij"));
    assert!(lines[1].to_string().starts_with("↓ "));
}

#[test]
fn hint_lines_style_keys() {
    let add_task_keys =
        styled_key_contents(add_task_hint_line(AddTaskStep::Title, false, false, false));
    assert_eq!(
        add_task_keys,
        vec!["Enter", "↑/↓", "Tab", "^N", "F1", "Esc"]
    );

    let multiline_keys = styled_key_contents(multiline_hint_line());
    assert_eq!(multiline_keys, vec!["Ctrl-Enter / ^S", "Esc"]);

    let add_task_description_keys = styled_key_contents(add_task_hint_line(
        AddTaskStep::Description,
        false,
        false,
        false,
    ));
    assert_eq!(
        add_task_description_keys,
        vec!["Ctrl-Enter / ^S", "Tab", "^N", "F1", "Esc"]
    );

    let add_task_description_editor_keys = styled_key_contents(add_task_description_hint_line());
    assert_eq!(
        add_task_description_editor_keys,
        vec!["Ctrl-Enter / ^S", "Enter", "^P", "^R", "Esc"]
    );

    let add_task_natural_keys = styled_key_contents(add_task_natural_hint_line());
    assert_eq!(
        add_task_natural_keys,
        vec!["Ctrl-Enter / ^S", "Enter", "Esc"]
    );

    let status_keys = styled_key_contents(add_task_status_hint_line());
    assert_eq!(status_keys, vec!["i", "b", "t", "a", "d", "x", "Esc"]);

    let priority_keys = styled_key_contents(add_task_priority_hint_line());
    assert_eq!(priority_keys, vec!["n", "l", "m", "h", "u", "Esc"]);

    let confirm_keys = styled_key_contents(confirm_hint_line());
    assert_eq!(confirm_keys, vec!["y", "n", "Esc"]);
}

#[test]
fn add_task_empty_title_input_shows_placeholder() {
    let line = add_task_title_input_line("", Some(0), 20);
    assert_eq!(line.spans[0].content.as_ref(), "E");
    assert_eq!(line.spans[0].style.fg, Some(BG_ALT));
    assert_eq!(line.spans[0].style.bg, Some(FG));
    assert_eq!(line.spans[1].content.as_ref(), "nter title here...");
    assert_eq!(line.spans[1].style.fg, Some(FG_DIM));
    assert_eq!(line.to_string(), ADD_TASK_TITLE_PLACEHOLDER);
}

#[test]
fn add_task_empty_title_input_without_focus_omits_cursor() {
    let line = add_task_title_input_line("", None, 20);
    assert_eq!(line.to_string(), ADD_TASK_TITLE_PLACEHOLDER);
    assert_eq!(line.spans.len(), 1);
    assert_eq!(line.spans[0].style.fg, Some(FG_DIM));
    assert_eq!(line.spans[0].style.bg, None);
}

#[test]
fn add_task_title_input_draws_cursor_as_cell() {
    let line = add_task_title_input_line("abc", Some(1), 20);
    assert_eq!(line.spans[0].content.as_ref(), "a");
    assert_eq!(line.spans[1].content.as_ref(), "b");
    assert_eq!(line.spans[1].style.fg, Some(BG_ALT));
    assert_eq!(line.spans[1].style.bg, Some(FG));
    assert_eq!(line.spans[2].content.as_ref(), "c");
}

#[test]
fn add_task_title_input_draws_end_cursor_as_blank_cell() {
    let line = add_task_title_input_line("abc", Some(3), 20);
    assert_eq!(line.spans[0].content.as_ref(), "abc");
    assert_eq!(line.spans[1].content.as_ref(), " ");
    assert_eq!(line.spans[1].style.bg, Some(FG));
}

#[test]
fn add_task_title_input_scrolls_to_cursor_cell() {
    let line = add_task_title_input_line("abcdef", Some(5), 4);
    assert_eq!(line.spans[0].content.as_ref(), "cde");
    assert_eq!(line.spans[1].content.as_ref(), "f");
}

#[test]
fn add_task_metadata_title_labels_values() {
    let line = add_task_metadata_title(
        "aven",
        "todo",
        "none",
        &["feature".to_string(), "ui".to_string()],
        120,
    );
    let rendered = line.to_string();
    assert!(rendered.contains("project: aven"));
    assert!(rendered.contains("status: todo"));
    assert!(rendered.contains("prio: none"));
    assert!(rendered.contains("labels: feature,ui"));
    assert!(rendered.contains(" · "));
    assert!(!rendered.contains("Tab"));
    assert!(!rendered.contains("^P"));
    let project = line
        .spans
        .iter()
        .find(|span| span.content == "aven")
        .unwrap();
    assert_eq!(project.style.fg, Some(theme::project_color("aven")));
    let status = line
        .spans
        .iter()
        .find(|span| span.content == "todo")
        .unwrap();
    assert_eq!(status.style.fg, theme::status_style("todo").fg);
    let priority = line
        .spans
        .iter()
        .find(|span| span.content == "none")
        .unwrap();
    assert_eq!(priority.style.fg, theme::priority_style("none").fg);
}

#[test]
fn add_task_description_empty_input_shows_placeholder() {
    let line = add_task_description_input_line("", Some(0), true);
    assert_eq!(line.spans[0].content.as_ref(), "O");
    assert_eq!(line.spans[0].style.fg, Some(BG_ALT));
    assert_eq!(line.spans[0].style.bg, Some(FG));
    assert_eq!(
        line.spans[1].content.as_ref(),
        "ptional details, links, or handoff context..."
    );
    assert_eq!(line.spans[1].style.fg, Some(FG_DIM));
}

#[test]
fn add_task_description_empty_unfocused_shows_placeholder() {
    let line = add_task_description_input_line("", None, true);
    assert_eq!(
        line.to_string(),
        "Optional details, links, or handoff context..."
    );
    assert_eq!(line.spans[0].style.fg, Some(FG_DIM));
}

#[test]
fn recurring_schedule_dialog_renders_fields_and_preview() {
    let rendered = render_overlay_view_at(
        add_task_overlay(AddTaskView {
            mode: borrow_value(AddTaskMode::Schedule(schedule_editor(
                ScheduleEditorMode::Repeat,
            ))),
            ..add_task_view()
        }),
        120,
        30,
    );
    for expected in [
        " Repeating ",
        "Repeat    every Friday",
        "Available 09:00",
        "Same day - due on the occurrence day",
        "Starts    2026-08-03",
        "Next      Fri Aug 7, Fri Aug 14",
    ] {
        assert!(rendered.contains(expected), "missing {expected}");
    }
}

#[test]
fn recurring_schedule_dialog_keeps_validation_guidance_visible() {
    let mut editor = schedule_editor(ScheduleEditorMode::Repeat);
    editor.repeat_rule = LineEdit::new("sometimes".to_string());
    editor.error = Some(crate::recurrence_input::rule_guidance().to_string());
    let rendered = render_overlay_view_at(
        add_task_overlay(AddTaskView {
            mode: borrow_value(AddTaskMode::Schedule(editor)),
            ..add_task_view()
        }),
        100,
        28,
    );
    assert!(rendered.contains("Repeat"));
    assert!(rendered.contains("Repeat: use daily"));
    assert!(rendered.contains("←/→ choose"));
    assert!(rendered.contains("↑/↓ move"));
}

#[test]
fn once_schedule_dialog_explains_available_and_due_inputs() {
    let mut editor = schedule_editor(ScheduleEditorMode::Once);
    editor.focus = ScheduleEditorField::Available;
    let rendered = render_overlay_view_at(
        add_task_overlay(AddTaskView {
            mode: borrow_value(AddTaskMode::Schedule(editor)),
            ..add_task_view()
        }),
        100,
        28,
    );
    assert!(rendered.contains(" One-off "));
    assert!(!rendered.contains(" None "));
    assert!(rendered.contains("Available tomorrow or next monday at 9am"));
    assert!(rendered.contains("Due       next friday or none"));
}

#[test]
fn once_schedule_dialog_aligns_type_and_field_values() {
    let buffer = overlay_buffer(add_task_overlay(AddTaskView {
        mode: borrow_value(AddTaskMode::Schedule(schedule_editor(
            ScheduleEditorMode::Once,
        ))),
        ..add_task_view()
    }));
    let rows = (0..buffer.area.height)
        .map(|row| buffer_row(&buffer, row))
        .collect::<Vec<_>>();
    let type_row_index = rows
        .iter()
        .position(|row| row.contains(" One-off "))
        .unwrap();
    let type_row = &rows[type_row_index];
    let available_row = rows
        .iter()
        .find(|row| row.contains("tomorrow or next monday at 9am"))
        .unwrap();

    let type_start = type_row.find(" One-off ").unwrap();
    let available_start = available_row
        .find("tomorrow or next monday at 9am")
        .unwrap();
    let type_column = type_row[..type_start].chars().count();
    assert_eq!(
        type_column,
        available_row[..available_start].chars().count()
    );
    assert_eq!(
        buffer[(type_column as u16, type_row_index as u16)]
            .style()
            .bg,
        Some(ACCENT)
    );
}

#[test]
fn repeating_schedule_dialog_aligns_field_values() {
    let buffer = overlay_buffer(add_task_overlay(AddTaskView {
        mode: borrow_value(AddTaskMode::Schedule(schedule_editor(
            ScheduleEditorMode::Repeat,
        ))),
        ..add_task_view()
    }));
    let rows = (0..buffer.area.height)
        .map(|row| buffer_row(&buffer, row))
        .collect::<Vec<_>>();
    let value_columns = [
        " One-off ",
        "every Friday",
        "09:00",
        "Same day",
        "2026-08-03",
        "Fri Aug 7",
    ]
    .map(|value| {
        let row = rows.iter().find(|row| row.contains(value)).unwrap();
        let start = row.find(value).unwrap();
        row[..start].chars().count()
    });

    assert!(
        value_columns
            .iter()
            .all(|column| *column == value_columns[0])
    );
}

#[test]
fn repeating_schedule_dialog_waits_to_validate_an_empty_rule() {
    let mut editor = schedule_editor(ScheduleEditorMode::Repeat);
    editor.repeat_rule = LineEdit::blank();
    editor.refresh();
    let rendered = render_overlay_view(add_task_overlay(AddTaskView {
        mode: borrow_value(AddTaskMode::Schedule(editor)),
        ..add_task_view()
    }));

    assert!(!rendered.contains("Repeat: use daily"));
}

#[test]
fn composer_shows_configured_natural_schedule_without_detail_fields() {
    let rendered = render_overlay_view_at(
        add_task_overlay(AddTaskView {
            schedule_input: "daily at 09:00, no due, starting 2026-08-03".to_string(),
            repeat_rule: "daily".to_string(),
            repeat_at: "09:00".to_string(),
            repeat_due: "none".to_string(),
            repeat_start_on: "2026-08-03".to_string(),
            create_more_available: false,
            ..add_task_view()
        }),
        100,
        30,
    );
    assert!(rendered.contains("Schedule: daily"));
    assert!(!rendered.contains("Available:"));
    assert!(!rendered.contains("Starts:"));
    assert!(!rendered.contains("^G create more"));
}

#[test]
fn add_task_template_shows_natural_schedule_summary() {
    let rendered = render_overlay_view_at(
        add_task_overlay(AddTaskView {
            editing_template: true,
            schedule_input:
                "Every 4 weeks on Monday and Thursday, due same day, starting 2026-07-20"
                    .to_string(),
            repeat_rule: "Every 4 weeks on Monday and Thursday".to_string(),
            repeat_start_on: "2026-07-20".to_string(),
            ..add_task_view()
        }),
        100,
        30,
    );
    assert!(rendered.contains("Edit recurring template"));
    assert!(rendered.contains("Schedule: Every 4 weeks"));
    assert!(!rendered.contains("Europe/Stockholm"));
}

#[test]
fn add_task_description_blank_later_line_omits_placeholder() {
    let line = add_task_description_input_line("", Some(0), false);
    assert_eq!(line.to_string(), " ");
    assert!(!line.to_string().contains("Optional details"));
}
