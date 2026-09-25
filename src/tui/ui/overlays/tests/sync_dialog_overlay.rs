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
    assert!(rendered.contains("No changes waiting"));
    assert!(rendered.contains("end-to-end encrypted"));
    assert!(rendered.contains("automatic"));
    assert!(rendered.contains("pending"));
    assert!(rendered.contains("conflicts"));
    assert!(rendered.contains("› Sync now"));
    assert!(rendered.contains("Add device"));
    assert!(rendered.contains("d details"));
    assert!(!rendered.contains("sync cursor"));
    assert!(!rendered.contains("Up to date"));
}

#[test]
fn details_reveal_internal_diagnostics() {
    let rendered = render_overlay_view(sync_overlay(sync_status(), true));

    assert!(rendered.contains("DETAILS"));
    assert!(rendered.contains("wake address"));
    assert!(rendered.contains("sync cursor"));
    assert!(rendered.contains("local sequence"));
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

    assert!(local.contains("Local only"));
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
        text.starts_with("conflicts") && text.ends_with('2')
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
    assert!(!rendered.contains("sync cursor"));
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
        syncing: false,
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
fn local_databases_offer_setup_and_joining() {
    let rendered = render_page(SyncPage::Home, local_status(), SyncActivity::default());

    assert!(rendered.contains("Local only"));
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

#[test]
fn invitation_form_never_renders_the_secret() {
    let mut input = SecretText::default();
    input.insert("aven-sync-setup-1:SECRETSECRET");
    let rendered = render_page(
        SyncPage::Invitation {
            kind: InvitationKind::Setup,
            input,
            error: Some("This isn't a setup invitation."),
        },
        local_status(),
        SyncActivity::default(),
    );

    assert!(!rendered.contains("SECRET"));
    assert!(!rendered.contains("aven-sync-setup-1"));
    assert!(rendered.contains("30 characters pasted"));
    let mut single = SecretText::default();
    single.insert("a");
    let one = render_page(
        SyncPage::Invitation {
            kind: InvitationKind::Join,
            input: single,
            error: None,
        },
        local_status(),
        SyncActivity::default(),
    );
    assert!(one.contains("1 character pasted"), "{one}");
    assert!(rendered.contains("This isn't a setup invitation."));
    assert!(rendered.contains("isn't shown or saved"));
    assert!(rendered.contains(" Continue "));
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
    assert!(rendered.contains("142 tasks"));
    assert!(rendered.contains("2 images missing on this computer"));
    assert!(rendered.contains("backup restore or import"));
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

    assert!(rendered.contains("Setting up sync"));
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

    assert!(rendered.contains("Joined sync with https://sync.example.com"));
    assert!(rendered.contains("Some images are unavailable on the server"));
}

#[test]
fn failures_show_plain_messages_and_technical_details_on_request() {
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
    assert!(details.contains("bootstrap-network"));
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

    assert!(rendered.contains("invitation"), "{rendered}");
    assert!(rendered.contains("open, expires"), "{rendered}");
    assert!(rendered.contains("Cancel invitation"), "{rendered}");
}

fn sync_status() -> TuiSyncStatus {
    TuiSyncStatus {
        enabled: true,
        set_up: true,
        phase: LocalPhase::SetUp,
        interval_seconds: 60,
        daemon_wake: SyncStatusCheck::new(true, "127.0.0.1:3554"),
        sync_cursor: Some("42".to_string()),
        local_sequence: Some("45".to_string()),
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

    assert!(rendered.contains("Remove device a1a1a1a1ff…?"));
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
    assert!(form.contains("Use a new invitation"), "{form}");
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
    assert!(
        confirm.contains("Continue joining with a new invitation"),
        "{confirm}"
    );
    assert!(confirm.contains("keeps its identity"), "{confirm}");
    assert!(confirm.contains("earlier invitation is kept"), "{confirm}");
}
