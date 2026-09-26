use super::*;
use crate::sync::encrypted::{LocalPhase, SetupPreview, Stage};
use crate::tui::overlay::{InvitationKind, SecretText, SyncPage};
use crate::tui::sync_operations::{
    DrainSummary, OperationFailure, OperationKind, OperationResult, RunningOperation, SyncActivity,
};

#[test]
fn idle_sync_renders_compact_summary_and_actions() {
    let rendered = render_overlay_view(sync_overlay(sync_status(), false));

    assert!(rendered.contains(SYNC_TITLE));
    assert!(!rendered.contains("Sync status"));
    assert!(rendered.contains("● No changes waiting"));
    assert!(!rendered.contains("end-to-end encrypted"));
    assert!(rendered.contains("Server          https://sync.example.com"));
    assert!(rendered.contains("Automatic"));
    assert!(!rendered.contains("Pending"));
    assert!(!rendered.contains("Conflicts"));
    assert!(rendered.contains("› Sync now"));
    assert!(rendered.contains("Add device"));
    assert!(rendered.contains("d details"));
    assert!(!rendered.contains("Sync cursor"));
    assert!(!rendered.contains("Up to date"));
}

#[test]
fn details_reveal_internal_diagnostics() {
    let rendered = render_overlay_view(sync_overlay(sync_status(), true));

    assert!(rendered.contains("DETAILS"));
    assert!(rendered.contains("Wake address"));
    assert!(rendered.contains("Sync cursor"));
    assert!(rendered.contains("Local sequence"));
    assert!(rendered.contains("d summary"));
}

#[test]
fn wake_failures_are_visible_without_expanding_details() {
    let mut status = sync_status();
    status.daemon_wake = SyncStatusCheck::new(false, "invalid daemon wake address");
    let state = SyncDialogState::default();
    let lines = sync_dialog_lines_for_test(&sync_view(&state, status));

    assert!(
        lines
            .iter()
            .any(|line| line.to_string().contains("Sync needs attention"))
    );
    assert!(lines.iter().any(|line| {
        line.spans
            .iter()
            .any(|span| span.style.fg == Some(ORANGE) && span.content.contains("invalid"))
    }));
    assert!(!lines.iter().any(|line| line.to_string() == "DETAILS"));
}

#[test]
fn unset_up_and_runtime_disabled_states_have_distinct_copy() {
    let local = render_overlay_view(sync_overlay(TuiSyncStatus::default(), false));
    let disabled = render_overlay_view(sync_overlay(
        TuiSyncStatus {
            runtime_allowed: false,
            ..sync_status()
        },
        false,
    ));

    assert!(!local.contains("Local only"));
    assert!(!local.contains("Sync now"));
    assert!(local.contains("Set up sync"));
    assert!(disabled.contains("Sync disabled"));
}

#[test]
fn conflicts_use_attention_color() {
    let mut status = sync_status();
    status.conflicts = 2;
    let state = SyncDialogState::default();
    let lines = sync_dialog_lines_for_test(&sync_view(&state, status));

    assert_eq!(lines[0].spans[0].style.fg, Some(ORANGE));
    assert!(lines.iter().any(|line| {
        let text = line.to_string();
        text.starts_with("Conflicts") && text.ends_with('2')
    }));
}

#[test]
fn manual_sync_is_available_when_automatic_sync_is_off() {
    let mut status = sync_status();
    status.enabled = false;
    let rendered = render_overlay_view(sync_overlay(status, false));

    assert!(rendered.contains("off"));
    assert!(rendered.contains("Sync now"));
}

#[test]
fn summary_fits_narrow_terminals() {
    let rendered = render_overlay_view_at(sync_overlay(sync_status(), false), 30, 16);

    assert!(rendered.contains("Sync"));
    assert!(rendered.contains("No changes"));
    assert!(rendered.contains("Sync now"));
    assert!(!rendered.contains("Sync cursor"));
}

#[test]
fn expanded_details_scroll_on_short_terminals() {
    let state = SyncDialogState {
        details: true,
        ..SyncDialogState::default()
    };
    let view = sync_view(&state, sync_status());

    assert!(sync_dialog_scroll_cap(&view, (52, 12).into()) > 0);
    let collapsed = SyncDialogState::default();
    assert_eq!(
        sync_dialog_scroll_cap(&sync_view(&collapsed, sync_status()), (100, 30).into()),
        0
    );
    let rendered = render_overlay_view_at(OverlayView::Sync(Box::new(view)), 52, 12);
    assert!(rendered.contains("/"), "{rendered}");
}

#[test]
fn clicks_resolve_to_the_rendered_action_rows() {
    let state = SyncDialogState::default();
    let view = sync_view(&state, sync_status());
    let backend = TestBackend::new(80, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            render_non_help_overlay_content(
                frame,
                &OverlayView::Sync(Box::new(sync_view(&state, sync_status()))),
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let (row, line) = (0..30)
        .map(|row| (row, buffer_row(buffer, row)))
        .find(|(_, line)| line.contains("Add device"))
        .expect("add device row");
    let column = line[..line.find("Add device").unwrap()].chars().count() as u16;

    assert_eq!(
        sync_dialog_hit(&view, (80, 30).into(), column, row),
        SyncDialogHit::Action(1)
    );
    assert_eq!(
        sync_dialog_hit(&view, (80, 30).into(), 0, 0),
        SyncDialogHit::Outside
    );
}

fn sync_overlay(status: TuiSyncStatus, details: bool) -> OverlayView<'static> {
    let state = borrow_value(SyncDialogState {
        details,
        ..SyncDialogState::default()
    });
    OverlayView::Sync(Box::new(sync_view(state, status)))
}

fn sync_view(state: &SyncDialogState, status: TuiSyncStatus) -> SyncDialogView<'_> {
    activity_view(state, status, SyncActivity::default())
}

fn activity_view(
    state: &SyncDialogState,
    status: TuiSyncStatus,
    activity: SyncActivity,
) -> SyncDialogView<'_> {
    SyncDialogView {
        state,
        status: borrow_value(status),
        activity: borrow_value(activity),
        syncing: None,
    }
}

/// Dialog rows joined by spaces, so wrapped phrases stay searchable.
fn dialog_text(view: SyncDialogView<'_>) -> String {
    let backend = TestBackend::new(100, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| render_non_help_overlay_content(frame, &OverlayView::Sync(Box::new(view))))
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..40)
        .filter_map(|row| {
            let line = buffer_row(buffer, row);
            let start = line.find('│')? + '│'.len_utf8();
            let end = line.rfind('│')?;
            (start < end).then(|| line[start..end].trim().to_string())
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The border title row of the rendered dialog.
fn page_title(page: SyncPage, status: TuiSyncStatus) -> String {
    let state = borrow_value(SyncDialogState::page(page));
    let backend = TestBackend::new(100, 40);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            render_non_help_overlay_content(
                frame,
                &OverlayView::Sync(Box::new(sync_view(state, status))),
            )
        })
        .unwrap();
    (0..40)
        .map(|row| buffer_row(terminal.backend().buffer(), row))
        .find(|line| line.contains(SYNC_TITLE))
        .expect("title row")
}

fn render_page(page: SyncPage, status: TuiSyncStatus, activity: SyncActivity) -> String {
    let state = borrow_value(SyncDialogState::page(page));
    dialog_text(activity_view(state, status, activity))
}

fn local_status() -> TuiSyncStatus {
    TuiSyncStatus::default()
}

fn running(kind: OperationKind, stage: Option<Stage>) -> SyncActivity {
    SyncActivity {
        running: Some(RunningOperation {
            kind,
            stage,
            started_at: std::time::Instant::now(),
        }),
        last: None,
        devices: None,
        join_timed_out: false,
    }
}

#[test]
fn sub_pages_name_themselves_in_the_border_title() {
    let invitation = |kind| SyncPage::Invitation {
        kind,
        input: SecretText::default(),
        error: None,
    };
    let status = |phase| TuiSyncStatus {
        set_up: phase != LocalPhase::NotSetUp,
        phase,
        ..TuiSyncStatus::default()
    };
    let confirm_join = |replace| SyncPage::ConfirmJoin {
        server: "https://sync.example.com".to_string(),
        invitation: SecretText::default(),
        replace,
    };
    let confirm_setup = SyncPage::ConfirmSetup {
        server: "https://sync.example.com".to_string(),
        preview: SetupPreview {
            workspaces: 1,
            tasks: 2,
            missing_images: 0,
            leaves_unencrypted_server: false,
        },
        invitation: SecretText::default(),
    };
    for (page, phase, expected) in [
        (
            invitation(InvitationKind::Setup),
            LocalPhase::NotSetUp,
            "Sync › Set up sync",
        ),
        (
            invitation(InvitationKind::Setup),
            LocalPhase::SetupIncomplete,
            "Sync › Resume setup",
        ),
        (confirm_setup, LocalPhase::NotSetUp, "Sync › Set up sync"),
        (
            invitation(InvitationKind::Join),
            LocalPhase::NotSetUp,
            "Sync › Join existing sync",
        ),
        (
            invitation(InvitationKind::Join),
            LocalPhase::JoinIncomplete,
            "Sync › Use a new invitation",
        ),
        (
            confirm_join(false),
            LocalPhase::NotSetUp,
            "Sync › Join existing sync",
        ),
        (
            confirm_join(true),
            LocalPhase::JoinIncomplete,
            "Sync › Use a new invitation",
        ),
        (
            SyncPage::Devices,
            LocalPhase::SetUp,
            "Sync › Manage devices",
        ),
        (
            SyncPage::ConfirmRemove { device: [2; 32] },
            LocalPhase::SetUp,
            "Sync › Remove device",
        ),
    ] {
        let title = page_title(page.clone(), status(phase));
        assert!(title.contains(expected), "{title}");
        let body = render_page(page, status(phase), SyncActivity::default());
        let heading = expected.trim_start_matches("Sync › ");
        assert!(
            !body.lines().any(|line| line.trim() == heading),
            "in-box title for {heading}: {body}"
        );
    }
    let home = page_title(SyncPage::Home, local_status());
    assert!(!home.contains('›'), "{home}");
}

#[test]
fn local_databases_offer_setup_and_joining() {
    let rendered = render_page(SyncPage::Home, local_status(), SyncActivity::default());

    assert!(!rendered.contains("Local only"));
    assert!(rendered.contains("Keep your tasks in sync across devices"));
    assert!(rendered.contains("› Set up sync"));
    assert!(rendered.contains("Join existing sync"));
    assert!(!rendered.contains("aven sync setup"));
}

#[test]
fn interrupted_work_offers_resume_instead_of_a_fresh_attempt() {
    for (phase, action) in [
        (LocalPhase::SetupIncomplete, "Resume setup"),
        (LocalPhase::JoinIncomplete, "Resume joining"),
    ] {
        let status = TuiSyncStatus {
            set_up: true,
            phase,
            ..TuiSyncStatus::default()
        };
        let rendered = render_page(SyncPage::Home, status, SyncActivity::default());
        assert!(rendered.contains(action), "{rendered}");
        assert!(!rendered.contains("Set up sync"), "{rendered}");
        assert!(!rendered.contains("No changes waiting"), "{rendered}");
    }
}

fn render_invitation(kind: InvitationKind, text: &str) -> String {
    let mut input = SecretText::default();
    input.insert(text);
    render_page(
        SyncPage::Invitation {
            kind,
            input,
            error: None,
        },
        local_status(),
        SyncActivity::default(),
    )
}

#[test]
fn invitation_form_never_renders_the_secret() {
    let mut input = SecretText::default();
    input.insert("aven-sync-setup-1:SECRETSECRET");
    let rendered = render_page(
        SyncPage::Invitation {
            kind: InvitationKind::Setup,
            input,
            error: Some("Paste the invitation first."),
        },
        local_status(),
        SyncActivity::default(),
    );

    assert!(!rendered.contains("SECRET"));
    assert!(!rendered.contains("aven-sync-setup-1"));
    assert!(rendered.contains("Incomplete invitation; part may be missing"));
    assert!(rendered.contains("Paste the invitation first."));
    assert!(!rendered.contains("isn't shown or saved"));
    assert!(rendered.contains(" Continue "));
    assert!(rendered.contains("←→ select  Enter choose  Ctrl-U clear  Esc back"));
    assert!(!rendered.contains("Enter continue"));
}

#[test]
fn resuming_setup_names_its_button_after_the_action() {
    let status = TuiSyncStatus {
        set_up: true,
        phase: LocalPhase::SetupIncomplete,
        ..TuiSyncStatus::default()
    };
    let rendered = render_page(
        SyncPage::Invitation {
            kind: InvitationKind::Setup,
            input: SecretText::default(),
            error: None,
        },
        status,
        SyncActivity::default(),
    );

    assert!(rendered.contains(" Resume setup "), "{rendered}");
    assert!(!rendered.contains(" Continue "), "{rendered}");
}

#[test]
fn empty_invitation_field_draws_an_input_cursor() {
    let state = SyncDialogState::page(SyncPage::Invitation {
        kind: InvitationKind::Join,
        input: SecretText::default(),
        error: None,
    });
    let lines = sync_dialog_lines_for_test(&sync_view(&state, local_status()));
    let field = lines
        .iter()
        .find(|line| line.to_string().contains("Paste the invitation here"))
        .expect("invitation field");

    assert!(
        field
            .spans
            .iter()
            .any(|span| span.content == "P" && span.style.bg == Some(crate::tui::theme::FG))
    );
}

#[test]
fn invitation_field_describes_what_was_pasted() {
    let (setup, device) = crate::sync::encrypted::sample_invitations("http://127.0.0.1:37463");
    for (kind, text, expected) in [
        (
            InvitationKind::Setup,
            "",
            "Invitation      Paste the invitation here",
        ),
        (
            InvitationKind::Setup,
            setup.as_str(),
            "Invitation      ✓ http://127.0.0.1:37463",
        ),
        (
            InvitationKind::Join,
            device.as_str(),
            "Invitation      ✓ http://127.0.0.1:37463",
        ),
        (
            InvitationKind::Setup,
            device.as_str(),
            "Device invitation; use Join existing sync instead",
        ),
        (
            InvitationKind::Join,
            setup.as_str(),
            "Setup invitation; use Set up sync instead",
        ),
        (
            InvitationKind::Join,
            &device[..40],
            "Incomplete invitation; part may be missing",
        ),
        (InvitationKind::Join, "hello", "Not an Aven invitation"),
    ] {
        let rendered = render_invitation(kind, text);
        assert!(rendered.contains(expected), "{rendered}");
        if text.len() > 20 {
            assert!(!rendered.contains(&text[20..]), "{rendered}");
        }
    }
}

#[test]
fn setup_data_summary_items_share_one_indent() {
    let state = SyncDialogState::page(SyncPage::ConfirmSetup {
        server: "https://sync.example.com".to_string(),
        preview: SetupPreview {
            workspaces: 1,
            tasks: 2,
            missing_images: 3,
            leaves_unencrypted_server: false,
        },
        invitation: SecretText::default(),
    });
    let lines = sync_dialog_lines_for_test(&sync_view(&state, local_status()))
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let start = lines
        .iter()
        .position(|line| line == "Use this computer's data:")
        .expect("data summary")
        + 1;
    let items = lines[start..]
        .iter()
        .take_while(|line| !line.is_empty())
        .collect::<Vec<_>>();

    assert!(items.len() > 3, "the missing-images item wraps: {items:?}");
    assert_eq!(items[0], "  1 workspace");
    assert_eq!(items[1], "  2 tasks (including scheduled and recurring)");
    for item in items {
        assert!(
            item.starts_with("  ") && !item.starts_with("   "),
            "{item:?}"
        );
    }
}

#[test]
fn setup_confirmation_discloses_data_server_and_restrictions() {
    let rendered = render_page(
        SyncPage::ConfirmSetup {
            server: "https://sync.example.com".to_string(),
            preview: SetupPreview {
                workspaces: 3,
                tasks: 142,
                missing_images: 2,
                leaves_unencrypted_server: true,
            },
            invitation: SecretText::default(),
        },
        local_status(),
        SyncActivity::default(),
    );

    assert!(rendered.contains("https://sync.example.com"));
    assert!(rendered.contains("3 workspaces"));
    assert!(rendered.contains("142 tasks (including scheduled and recurring)"));
    assert!(rendered.contains("2 images missing on this computer"));
    assert!(!rendered.contains("restore"));
    assert!(rendered.contains("previous unencrypted sync server"));
    assert!(rendered.contains(" Back "));
    assert!(rendered.contains(" Set up sync "));
}

#[test]
fn setup_progress_marks_reached_stages_without_counts() {
    let status = TuiSyncStatus {
        set_up: true,
        phase: LocalPhase::SetupIncomplete,
        ..TuiSyncStatus::default()
    };
    let rendered = render_page(
        SyncPage::Home,
        status,
        running(OperationKind::Setup, Some(Stage::UploadingData)),
    );

    assert!(rendered.contains("✓ Prepared your data"));
    assert!(rendered.contains("Uploading encrypted data"));
    assert!(rendered.contains("· Finishing setup"));
    assert!(rendered.contains("close this dialog and keep working"));
    assert!(!rendered.contains('%'));
    assert!(!rendered.contains("Resume setup"));
}

#[test]
fn join_progress_distinguishes_tasks_from_images() {
    let waiting = render_page(
        SyncPage::Home,
        local_status(),
        running(OperationKind::Join, Some(Stage::WaitingForInviter)),
    );
    assert!(waiting.contains("Waiting for the other device"));
    assert!(!waiting.contains("Local only"), "{waiting}");
    assert!(waiting.contains("Editing waits until tasks are downloaded"));

    let images = render_page(
        SyncPage::Home,
        TuiSyncStatus {
            set_up: true,
            phase: LocalPhase::SetUp,
            ..TuiSyncStatus::default()
        },
        running(OperationKind::Join, Some(Stage::DownloadingImages)),
    );
    assert!(images.contains("✓ Downloaded tasks"));
    assert!(images.contains("Downloading images"));
    assert!(images.contains("Tasks are available"));
}

#[test]
fn results_distinguish_images_from_tasks() {
    let rendered = render_page(
        SyncPage::Home,
        sync_status(),
        SyncActivity {
            running: None,
            last: Some(OperationResult::Joined {
                server: "https://sync.example.com".to_string(),
                drain: DrainSummary {
                    tasks_current: true,
                    images: "unavailable",
                },
            }),
            devices: None,
            join_timed_out: false,
        },
    );

    assert!(rendered.contains("✓ Joined sync"));
    assert!(rendered.contains("Some images are unavailable on the server"));
}

#[test]
fn complete_results_leave_only_the_status_and_server() {
    let rendered = render_page(
        SyncPage::Home,
        sync_status(),
        SyncActivity {
            last: Some(OperationResult::SetUp {
                server: "https://sync.example.com".to_string(),
                drain: DrainSummary {
                    tasks_current: true,
                    images: "complete",
                },
            }),
            ..SyncActivity::default()
        },
    );

    assert!(rendered.contains("No changes waiting"));
    assert!(!rendered.contains("Sync is set up"));
    assert!(!rendered.contains("in sync"));
    assert_eq!(rendered.matches("https://sync.example.com").count(), 1);
}

#[test]
fn manual_sync_spins_in_the_status_line_without_moving_rows() {
    let state = SyncDialogState::default();
    let idle = sync_dialog_lines_for_test(&sync_view(&state, sync_status()));
    let mut view = sync_view(&state, sync_status());
    view.syncing = Some(std::time::Instant::now());
    let syncing = sync_dialog_lines_for_test(&view);

    assert_eq!(idle.len(), syncing.len());
    assert!(idle[0].to_string().contains("No changes waiting"));
    assert!(syncing[0].to_string().ends_with("Syncing…"));
    assert_eq!(syncing[0].spans[1].style.fg, Some(ACCENT));
    for (before, after) in idle.iter().zip(&syncing).skip(1) {
        assert_eq!(before.to_string(), after.to_string());
    }
}

#[test]
fn nonzero_counts_show_as_attention_rows() {
    let status = TuiSyncStatus {
        pending_changes: 3,
        ..sync_status()
    };
    let lines = sync_dialog_lines_for_test(&sync_view(&SyncDialogState::default(), status));
    let pending = lines
        .iter()
        .find(|line| line.to_string().starts_with("Pending"))
        .expect("pending row");
    assert!(pending.to_string().ends_with('3'));
    assert_eq!(pending.spans[1].style.fg, Some(ORANGE));
}

#[test]
fn failures_show_plain_messages_without_raw_error_chains() {
    let failure = OperationFailure {
        kind: OperationKind::Setup,
        message: "Couldn't reach the sync server.".to_string(),
        details: "error bootstrap-network outcome-unknown".to_string(),
    };
    let activity = SyncActivity {
        running: None,
        last: Some(OperationResult::Failed(failure)),
        devices: None,
        join_timed_out: false,
    };
    let status = TuiSyncStatus {
        set_up: true,
        phase: LocalPhase::SetupIncomplete,
        ..TuiSyncStatus::default()
    };
    let summary = render_page(SyncPage::Home, status.clone(), activity.clone());
    assert!(summary.contains("Setup didn't finish"));
    assert!(summary.contains("Couldn't reach the sync server."));
    assert!(!summary.contains("bootstrap-network"));
    assert!(summary.contains("Resume setup"));

    let state = borrow_value(SyncDialogState {
        details: true,
        ..SyncDialogState::default()
    });
    let details = dialog_text(activity_view(state, status, activity));
    assert!(details.contains("Couldn't reach the sync server."));
    assert!(!details.contains("bootstrap-network"));
}

#[test]
fn access_refusal_shows_error_guidance_and_only_retry() {
    let status = TuiSyncStatus {
        access_refused_at: Some("2026-09-24T12:00:00Z".to_string()),
        ..sync_status()
    };

    let rendered = render_page(SyncPage::Home, status, SyncActivity::default());

    assert!(rendered.contains("Sync access unconfirmed"));
    assert!(rendered.contains("may have been removed from sync"));
    assert!(rendered.contains("Local tasks and images stay here"));
    assert!(rendered.contains("Sync now"));
    assert!(!rendered.contains("Add device"));
    assert!(!rendered.contains("Manage devices"));
}

#[test]
fn confirmation_buttons_are_clickable() {
    let state = SyncDialogState::page(SyncPage::ConfirmJoin {
        server: "https://sync.example.com".to_string(),
        invitation: SecretText::default(),
        replace: false,
    });
    let view = sync_view(&state, local_status());
    let backend = TestBackend::new(80, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            render_non_help_overlay_content(
                frame,
                &OverlayView::Sync(Box::new(sync_view(&state, local_status()))),
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let (row, line) = (0..30)
        .map(|row| (row, buffer_row(buffer, row)))
        .rfind(|(_, line)| line.contains(" Back "))
        .expect("button row");
    let join = line[..line.find(" Join ").unwrap()].chars().count() as u16 + 1;
    let back = line[..line.find(" Back ").unwrap()].chars().count() as u16 + 1;

    assert_eq!(
        sync_dialog_hit(&view, (80, 30).into(), join, row),
        SyncDialogHit::Action(1)
    );
    assert_eq!(
        sync_dialog_hit(&view, (80, 30).into(), back, row),
        SyncDialogHit::Action(0)
    );
}

#[test]
fn open_invitation_is_visible_and_can_be_cancelled() {
    let status = TuiSyncStatus {
        invitation: Some(crate::sync::encrypted::InvitationStatus {
            expires_at: crate::sync::encrypted::unix_now().unwrap() + 600,
            keys_may_have_been_sent: false,
        }),
        ..sync_status()
    };
    let rendered = render_page(SyncPage::Home, status, SyncActivity::default());

    assert!(rendered.contains("Invitation"), "{rendered}");
    assert!(rendered.contains("open, expires"), "{rendered}");
    assert!(rendered.contains("Cancel invitation"), "{rendered}");
}

fn sync_status() -> TuiSyncStatus {
    TuiSyncStatus {
        enabled: true,
        set_up: true,
        phase: LocalPhase::SetUp,
        interval_seconds: 60,
        daemon_wake: SyncStatusCheck::new(true, "127.0.0.1:3746"),
        sync_cursor: Some("42".to_string()),
        local_sequence: Some("45".to_string()),
        server: Some("https://sync.example.com".to_string()),
        ..TuiSyncStatus::default()
    }
}

fn device_activity(rotation_pending: bool) -> SyncActivity {
    use crate::sync::encrypted::{Device, DeviceListing};
    let mut other = [0xa1; 32];
    other[4] = 0xff;
    SyncActivity {
        devices: Some(crate::tui::sync_operations::DeviceSnapshot {
            listing: DeviceListing {
                server: "https://sync.example.com".to_string(),
                key_rotation_pending: rotation_pending,
                devices: vec![
                    Device {
                        id: [0xa1; 32],
                        label: Some("Office Mac".to_string()),
                        current: true,
                        admission_sequence: 0,
                    },
                    Device {
                        id: other,
                        label: None,
                        current: false,
                        admission_sequence: 7,
                    },
                ],
            },
            checked_at: std::time::Instant::now(),
            after_removal: false,
        }),
        ..SyncActivity::default()
    }
}

#[test]
fn device_list_marks_this_device_and_disambiguates_short_ids() {
    let rendered = render_page(SyncPage::Devices, sync_status(), device_activity(false));

    assert!(rendered.contains("Checked with the server just now."));
    assert!(rendered.contains("› a1a1a1a1a1…  Office Mac"));
    assert!(rendered.contains("This device"));
    assert!(rendered.contains("a1a1a1a1ff…"));
    assert!(rendered.contains(&hex::encode([0xa1; 32])[..40]));
    assert!(rendered.contains("removed only from another device"));
    assert!(!rendered.contains("admission"));
    assert!(!rendered.contains("Remove device"));
    assert!(rendered.contains("This device a1a1a1a1ff… Name Office Mac Device ID"));
}

#[test]
fn selecting_another_device_shows_its_full_id_and_removal_hint() {
    let state = borrow_value(SyncDialogState {
        selected: 1,
        ..SyncDialogState::page(SyncPage::Devices)
    });
    let rendered = dialog_text(activity_view(state, sync_status(), device_activity(false)));
    let mut other = [0xa1; 32];
    other[4] = 0xff;

    assert!(rendered.contains(&hex::encode(other)[..40]));
    assert!(rendered.contains("Enter removes this device from sync."));
    assert!(rendered.contains("y copy ID"));
    assert!(rendered.contains("a1a1a1a1ff… Device ID"));
    assert!(!rendered.contains("Name"));
}

#[test]
fn pending_rotation_and_stale_lists_are_labelled_honestly() {
    let pending = render_page(SyncPage::Devices, sync_status(), device_activity(true));
    assert!(pending.contains("Securing future changes after a removal is unfinished"));
    assert!(pending.contains("› Finish removal"));

    let mut stale = device_activity(false);
    stale.last = Some(OperationResult::Failed(OperationFailure {
        kind: OperationKind::ListDevices,
        message: "Couldn't reach the sync server.".to_string(),
        details: "error enrollment-network outcome-unknown".to_string(),
    }));
    let rendered = render_page(SyncPage::Devices, sync_status(), stale);
    assert!(rendered.contains("latest check failed"));
    assert!(!rendered.contains("Checked with the server"));
    assert!(rendered.contains("Couldn't reach the sync server."));
}

#[test]
fn removal_confirmation_explains_consequences_and_names_the_full_target() {
    let mut other = [0xa1; 32];
    other[4] = 0xff;
    let rendered = render_page(
        SyncPage::ConfirmRemove { device: other },
        sync_status(),
        device_activity(false),
    );

    assert!(rendered.contains("Remove a1a1a1a1ff… from sync?"));
    assert!(rendered.contains("lose access to future synced changes"));
    assert!(rendered.contains("already downloaded will remain on it"));
    assert!(rendered.contains(&hex::encode(other)[..40]));
    assert!(rendered.contains(" Cancel "));
    assert!(rendered.contains(" Remove device "));
}

#[test]
fn removal_results_do_not_claim_completion_before_rotation() {
    let removal = |key_rotation_pending| SyncActivity {
        last: Some(OperationResult::Removed(crate::sync::encrypted::Removal {
            device: [0xb2; 32],
            access_revoked: true,
            key_rotation_pending,
        })),
        ..device_activity(key_rotation_pending)
    };
    let pending = render_page(SyncPage::Devices, sync_status(), removal(true));
    assert!(pending.contains("Access removed for b2b2b2b2…"));
    assert!(pending.contains("Securing future changes is unfinished"));
    assert!(!pending.contains("Removed b2b2b2b2… from sync"));

    let complete = render_page(SyncPage::Devices, sync_status(), removal(false));
    assert!(complete.contains("Removed b2b2b2b2… from sync"));
    assert!(complete.contains("Future changes use new keys"));

    let running = SyncActivity {
        running: Some(RunningOperation {
            kind: OperationKind::RemoveDevice([0xb2; 32]),
            stage: None,
            started_at: std::time::Instant::now(),
        }),
        ..device_activity(false)
    };
    let rendered = render_page(SyncPage::Devices, sync_status(), running);
    assert!(rendered.contains("Removing access and securing future changes"));
}

#[test]
fn join_timeout_guidance_stays_visible_while_resuming_in_the_session() {
    let incomplete = TuiSyncStatus {
        set_up: true,
        phase: LocalPhase::JoinIncomplete,
        ..TuiSyncStatus::default()
    };
    let timed_out = SyncActivity {
        last: Some(OperationResult::Failed(OperationFailure {
            kind: OperationKind::Join,
            message: crate::tui::sync_errors::JOIN_TIMEOUT.to_string(),
            details: "error sync-join-timeout".to_string(),
        })),
        join_timed_out: true,
        ..SyncActivity::default()
    };
    let after = render_page(SyncPage::Home, incomplete.clone(), timed_out.clone());
    assert!(after.contains("didn't add this device in time"), "{after}");
    assert!(after.contains("Use a new invitation"), "{after}");
    assert!(after.contains("choose Use a new invitation"), "{after}");
    assert!(after.contains("Resume joining"), "{after}");

    let resuming = SyncActivity {
        running: Some(RunningOperation {
            kind: OperationKind::Join,
            stage: Some(Stage::WaitingForInviter),
            started_at: std::time::Instant::now(),
        }),
        last: None,
        ..timed_out
    };
    let during = render_page(SyncPage::Home, incomplete.clone(), resuming);
    assert!(during.contains("Waiting for the other device"), "{during}");
    assert!(during.contains("choose Use a new invitation"), "{during}");

    let fresh = render_page(SyncPage::Home, incomplete, SyncActivity::default());
    assert!(!fresh.contains("choose Use a new invitation"), "{fresh}");
}

#[test]
fn a_new_invitation_for_an_unfinished_join_explains_what_is_kept() {
    let incomplete = TuiSyncStatus {
        set_up: true,
        phase: LocalPhase::JoinIncomplete,
        ..TuiSyncStatus::default()
    };
    let mut input = SecretText::default();
    input.insert("aven://pair/v2/SECRETSECRET");
    let form = render_page(
        SyncPage::Invitation {
            kind: InvitationKind::Join,
            input,
            error: None,
        },
        incomplete.clone(),
        SyncActivity::default(),
    );
    assert!(
        form.contains("device that created the first invitation"),
        "{form}"
    );
    assert!(!form.contains("SECRET"), "{form}");

    let confirm = render_page(
        SyncPage::ConfirmJoin {
            server: "https://sync.example.com".to_string(),
            invitation: SecretText::default(),
            replace: true,
        },
        incomplete,
        SyncActivity::default(),
    );
    assert!(confirm.contains("keeps its identity"), "{confirm}");
    assert!(confirm.contains("earlier invitation is kept"), "{confirm}");
}

#[test]
fn commands_render_in_code_style_without_backticks() {
    let rendered = render_invitation(InvitationKind::Setup, "");
    assert!(rendered.contains("printed by aven server setup on your server"));
    assert!(!rendered.contains('`'));

    let failure = OperationFailure {
        kind: OperationKind::Join,
        message: "Start aven with `aven --db /new/path sync join` to use a new database."
            .to_string(),
        details: String::new(),
    };
    let activity = SyncActivity {
        running: None,
        last: Some(OperationResult::Failed(failure)),
        devices: None,
        join_timed_out: false,
    };
    let state = SyncDialogState::default();
    let lines = sync_dialog_lines_for_test(&activity_view(&state, local_status(), activity));
    assert!(!lines.iter().any(|line| line.to_string().contains('`')));
    let code = lines
        .iter()
        .flat_map(|line| &line.spans)
        .filter(|span| span.style.bg == crate::tui::theme::CODE.bg)
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>()
        .join(" ");
    assert_eq!(code, "aven --db /new/path sync join");
}

/// A body line styled as a heading: flush left and every visible span bold,
/// with no status mark or selection prefix.
fn is_in_box_heading(line: &Line<'_>) -> bool {
    let text = line.to_string();
    !text.trim().is_empty()
        && !text.starts_with([' ', '›'])
        && line
            .spans
            .iter()
            .filter(|span| !span.content.trim().is_empty())
            .all(|span| span.style.add_modifier.contains(Modifier::BOLD))
}

#[test]
fn no_sync_page_renders_its_title_inside_the_box() {
    let failed = |kind| SyncActivity {
        last: Some(OperationResult::Failed(OperationFailure {
            kind,
            message: "Couldn't reach the sync server.".to_string(),
            details: String::new(),
        })),
        ..SyncActivity::default()
    };
    let phase = |phase| TuiSyncStatus {
        set_up: phase != LocalPhase::NotSetUp,
        phase,
        ..TuiSyncStatus::default()
    };
    let mut removed = device_activity(true);
    removed.last = Some(OperationResult::Removed(crate::sync::encrypted::Removal {
        device: [3; 32],
        access_revoked: true,
        key_rotation_pending: true,
    }));
    let mut removing = device_activity(false);
    removing.running = running(OperationKind::RemoveDevice([3; 32]), None).running;
    let conflicts = TuiSyncStatus {
        conflicts: 2,
        ..sync_status()
    };
    let cases = [
        (
            SyncPage::Home,
            phase(LocalPhase::NotSetUp),
            running(OperationKind::Setup, Some(Stage::UploadingData)),
            Some("Sync › Setting up sync"),
        ),
        (
            SyncPage::Home,
            phase(LocalPhase::SetupIncomplete),
            running(OperationKind::Setup, Some(Stage::FinishingSetup)),
            Some("Sync › Setting up sync"),
        ),
        (
            SyncPage::Home,
            phase(LocalPhase::JoinIncomplete),
            running(OperationKind::Join, Some(Stage::WaitingForInviter)),
            Some("Sync › Joining sync"),
        ),
        (SyncPage::Home, sync_status(), SyncActivity::default(), None),
        (SyncPage::Home, conflicts, SyncActivity::default(), None),
        (
            SyncPage::Home,
            sync_status(),
            running(OperationKind::Sync, None),
            None,
        ),
        (
            SyncPage::Home,
            phase(LocalPhase::SetupIncomplete),
            failed(OperationKind::Setup),
            None,
        ),
        (
            SyncPage::Home,
            phase(LocalPhase::SetupRecoveryRequired),
            SyncActivity::default(),
            None,
        ),
        (
            SyncPage::Devices,
            sync_status(),
            removing,
            Some("Sync › Manage devices"),
        ),
        (
            SyncPage::Devices,
            sync_status(),
            removed,
            Some("Sync › Manage devices"),
        ),
        (
            SyncPage::Devices,
            sync_status(),
            failed(OperationKind::ListDevices),
            Some("Sync › Manage devices"),
        ),
        (
            SyncPage::ConfirmRemove { device: [3; 32] },
            sync_status(),
            device_activity(false),
            Some("Sync › Remove device"),
        ),
    ];
    for (page, status, activity, title) in cases {
        let state = SyncDialogState::page(page);
        let view = activity_view(&state, status, activity);
        let heading = sync_dialog_lines_for_test(&view)
            .into_iter()
            .find(is_in_box_heading);
        assert!(heading.is_none(), "in-box heading: {heading:?}");

        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_non_help_overlay_content(frame, &OverlayView::Sync(Box::new(view)))
            })
            .unwrap();
        let border = (0..40)
            .map(|row| buffer_row(terminal.backend().buffer(), row))
            .find(|line| line.contains(SYNC_TITLE))
            .expect("title row");
        match title {
            Some(title) => assert!(border.contains(title), "{border}"),
            None => assert!(!border.contains('›'), "{border}"),
        }
    }
}
