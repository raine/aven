use aven_core::choices::{TaskPriority, TaskStatus};

pub(super) struct DemoTask {
    pub key: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub project: &'static str,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub labels: &'static [&'static str],
    pub due_in_days: Option<i64>,
    pub available_in_hours: Option<i64>,
    pub is_epic: bool,
}

pub(super) struct DemoNote {
    pub task: &'static str,
    pub body: &'static str,
}

pub(super) struct DemoDependency {
    pub task: &'static str,
    pub depends_on: &'static str,
}

pub(super) struct DemoEpicLink {
    pub epic: &'static str,
    pub child: &'static str,
}

pub(super) struct DemoRelatedLink {
    pub task: &'static str,
    pub related: &'static str,
}

pub(super) const PROJECTS: &[&str] = &[
    "api",
    "cli",
    "docs",
    "mobile-app",
    "sync-service",
    "website",
];

pub(super) const LABELS: &[&str] = &[
    "backend",
    "bug",
    "capture",
    "cli",
    "config",
    "docs",
    "frontend",
    "mobile",
    "refs",
    "release",
    "scheduling",
    "security",
    "sync",
    "testing",
    "ui",
    "ux",
];

pub(super) const TASKS: &[DemoTask] = &[
    // api
    DemoTask {
        key: "add_json_output_for_automation",
        title: "Add JSON output for automation",
        description: "## Goal\n\nReturn structured JSON for commands used by scripts and integrations.\n\n### Example\n\n```sh\naven list --json --project cli\n```\n\n### Acceptance criteria\n\n- [x] JSON includes refs, status, priority, labels, project, and relationships.\n- [x] Human-readable output remains the default.",
        project: "api",
        status: TaskStatus::Done,
        priority: TaskPriority::Medium,
        labels: &["backend"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "receive_webhook_task_captures",
        title: "Receive webhook task captures",
        description: "## Goal\n\nReceive signed webhook events that create task captures.\n\n### Event shape\n\n```json\n{\n  \"title\": \"Follow up on deploy notes\",\n  \"project\": \"api\"\n}\n```\n\n### Safety\n\nRequire signatures and narrow token scopes before accepting writes.",
        project: "api",
        status: TaskStatus::Backlog,
        priority: TaskPriority::None,
        labels: &["backend"],
        due_in_days: None,
        available_in_hours: Some(-4),
        is_epic: false,
    },
    DemoTask {
        key: "add_scoped_api_tokens",
        title: "Add scoped API tokens",
        description: "## Goal\n\nAdd scoped API tokens for trusted local tools and integrations.\n\n### Token scopes\n\n- `tasks:read` for browsing and dashboards.\n- `tasks:write` for capture tools.\n- `sync:diagnostics` for health checks.\n\n### Security notes\n\nTokens should be easy to rotate and should never appear in logs, task descriptions, or shell history.",
        project: "api",
        status: TaskStatus::Todo,
        priority: TaskPriority::High,
        labels: &["backend"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "import_tasks_from_markdown_checklists",
        title: "Import tasks from Markdown checklists",
        description: "## Goal\n\nTurn Markdown planning notes into structured tasks without losing context.\n\n### Example input\n\n```md\n- [ ] Add keyboard shortcut docs\n  - Depends on final keymap\n```\n\n### Output\n\n- One task per checklist item.\n- Nested bullets copied into the task description.\n- Source file path stored in an import note.",
        project: "api",
        status: TaskStatus::Inbox,
        priority: TaskPriority::None,
        labels: &["backend"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    // cli
    DemoTask {
        key: "add_a_one_line_quick_capture_command",
        title: "Add a one-line quick capture command",
        description: "## Goal\n\nProvide the fastest possible task capture path.\n\n### Command shape\n\n```sh\naven add \"follow up on release checklist\"\n```\n\n### Acceptance criteria\n\n- [x] Infers the current project.\n- [x] Saves without opening an editor.\n- [x] Prints a stable task ref for follow-up.",
        project: "cli",
        status: TaskStatus::Done,
        priority: TaskPriority::High,
        labels: &["cli"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "add_stable_project_prefixed_task_refs",
        title: "Add stable project-prefixed task refs",
        description: "## Goal\n\nMake task refs stable, compact, and easy to mention.\n\n### Properties\n\n- Prefix shows the project context.\n- Suffix stays stable if the task moves projects.\n- Refs work offline.\n\n### Example\n\n`CLI-FCXR` is readable in chat, notes, and terminal output.",
        project: "cli",
        status: TaskStatus::Done,
        priority: TaskPriority::Medium,
        labels: &["cli", "refs"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "parse_natural_language_dates_when_adding_tasks",
        title: "Parse natural-language dates when adding tasks",
        description: "## Goal\n\nParse simple date phrases during capture.\n\n### Examples\n\n- `tomorrow`\n- `next Friday`\n- `in two weeks`\n- `Monday morning`\n\n### Rule\n\nWhen parsing is ambiguous, keep the original text and ask for confirmation in the composer.",
        project: "cli",
        status: TaskStatus::Backlog,
        priority: TaskPriority::Medium,
        labels: &["capture"],
        due_in_days: None,
        available_in_hours: Some(72),
        is_epic: false,
    },
    DemoTask {
        key: "attach_screenshots_to_task_descriptions",
        title: "Improve screenshot attachments in task descriptions",
        description: "## Goal\n\nPolish pasted screenshot attachments so visual bugs keep their evidence.\n\n### Flow\n\n1. Paste an image into the composer.\n2. Aven stores it with the task.\n3. The detail view renders the attachment inline.\n\n### Acceptance criteria\n\n- [ ] Export preserves image files and Markdown references.\n- [ ] Failed image preparation has a clear retry path.",
        project: "cli",
        status: TaskStatus::Backlog,
        priority: TaskPriority::None,
        labels: &["capture", "ui"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "add_labels_in_the_task_composer",
        title: "Improve labels in the task composer",
        description: "## Goal\n\nMake label selection while creating a task faster and easier to scan.\n\n### Composer behavior\n\n- Rank matching labels as the user types.\n- Keep inline label creation available.\n- Preserve fast keyboard navigation.\n\n### Acceptance criteria\n\n- [ ] Large label sets remain easy to navigate.\n- [ ] The saved task appears in label search immediately.",
        project: "cli",
        status: TaskStatus::Todo,
        priority: TaskPriority::Medium,
        labels: &["ui"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "make_capture_instant_from_anywhere",
        title: "Make capture instant from anywhere",
        description: "## Epic outcome\n\nTask capture becomes available wherever work appears.\n\n### Capture paths\n\n- TUI composer.\n- One-line CLI command.\n- Clipboard import.\n- tmux popup.\n- Screenshot attachment flow.\n\n### Success signal\n\nUsers stop keeping temporary notes in random buffers because capture is faster than switching tools.",
        project: "cli",
        status: TaskStatus::Inbox,
        priority: TaskPriority::High,
        labels: &["ux"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: true,
    },
    DemoTask {
        key: "bring_dates_and_scheduling_into_the_task_flow",
        title: "Bring dates and scheduling into the task flow",
        description: "## Epic outcome\n\nDates and scheduling become part of the normal task workflow.\n\n### Child work\n\n- Due dates and start dates.\n- Today and Upcoming views.\n- Recurring rules.\n- Natural-language date capture.\n\n### Success signal\n\nUsers can plan a week of work without leaving the terminal queue.",
        project: "cli",
        status: TaskStatus::Inbox,
        priority: TaskPriority::Urgent,
        labels: &["ux"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: true,
    },
    DemoTask {
        key: "capture_tasks_from_a_tmux_popup",
        title: "Capture tasks from a tmux popup",
        description: "## Goal\n\nOpen a focused capture flow from a tmux popup and save work into the right project.\n\n### Workflow\n\n1. Press the global task-capture shortcut.\n2. Type a title and optional Markdown notes.\n3. Save and return to the previous pane.\n\n### Why it matters\n\nTask capture stays close to the work, even when the user is deep inside a terminal session.",
        project: "cli",
        status: TaskStatus::Todo,
        priority: TaskPriority::High,
        labels: &["config", "ux"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "design_recurring_task_rules",
        title: "Design recurring task rules",
        description: "## Goal\n\nDefine the recurrence rules that Aven supports first.\n\n### Rules to support\n\n- Daily and weekly repeat intervals.\n- Weekday-only schedules.\n- Repeat after completion.\n\n### Non-goals\n\n- Full calendar replacement.\n- Complex RRULE editing in the first pass.",
        project: "cli",
        status: TaskStatus::Todo,
        priority: TaskPriority::High,
        labels: &["scheduling"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "show_today_and_upcoming_views",
        title: "Improve Today and Upcoming views",
        description: "## Goal\n\nRefine Today and Upcoming so dated work is easy to plan.\n\n### Today\n\nShows tasks due today, tasks that become available today, and overdue items.\n\n### Upcoming\n\nGroups dated work by week while keeping unavailable tasks out of the main queue.",
        project: "cli",
        status: TaskStatus::Todo,
        priority: TaskPriority::High,
        labels: &["ui"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "add_due_dates_and_scheduling",
        title: "Add due dates and scheduling",
        description: "## Goal\n\nRefine dates in the task flow without turning Aven into a calendar.\n\n### Scope\n\n- Improve due dates and availability editing in the TUI and CLI.\n- Keep the queue focused on work that is actionable today.\n- Explain date-driven queue placement in task detail.\n\n### Acceptance criteria\n\n- [ ] Date editing remains fast from the keyboard.\n- [ ] Queue groups use dates consistently when ranking work.\n- [ ] Task detail explains why an item appears in **Needs Action**.",
        project: "cli",
        status: TaskStatus::Active,
        priority: TaskPriority::Urgent,
        labels: &["scheduling", "ux"],
        due_in_days: Some(-2),
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "highlight_overdue_work_in_the_queue",
        title: "Highlight overdue work in the queue",
        description: "## Goal\n\nMake overdue work visible without making the interface feel alarming.\n\n### Design constraints\n\n- Use calm color, not constant warning red.\n- Prefer sorting and section placement over visual noise.\n- Explain overdue state in task detail.\n\n### Acceptance criteria\n\n- [ ] Overdue tasks rank above normal todo work.\n- [ ] The TUI shows why the task is urgent.",
        project: "cli",
        status: TaskStatus::Inbox,
        priority: TaskPriority::Medium,
        labels: &["ui"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "create_tasks_from_clipboard_text",
        title: "Create tasks from clipboard text",
        description: "## Goal\n\nTurn copied text into a well-formed task quickly.\n\n### Inputs\n\n- Error messages.\n- Review comments.\n- Meeting notes.\n\n### Behavior\n\nAven extracts a short title, preserves the raw text as Markdown, and routes the task to the active project when possible.",
        project: "cli",
        status: TaskStatus::Inbox,
        priority: TaskPriority::Medium,
        labels: &["capture"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    // docs
    DemoTask {
        key: "publish_getting_started_guide",
        title: "Publish getting started guide",
        description: "## Goal\n\nPublish a getting started guide that takes users from install to first useful queue.\n\n### Guide outline\n\n1. Install Aven.\n2. Open the TUI.\n3. Add a task.\n4. Add notes and labels.\n5. Review the queue.\n\n### Done when\n\nA new user can follow the guide without already knowing the task model.",
        project: "docs",
        status: TaskStatus::Done,
        priority: TaskPriority::Medium,
        labels: &["docs"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "add_example_workflows_for_busy_projects",
        title: "Add example workflows for busy projects",
        description: "## Goal\n\nAdd realistic example workflows to the docs.\n\n### Examples\n\n- Triage a busy inbox.\n- Capture work from code review.\n- Resume work from notes.\n- Sync across two laptops.\n\n### Format\n\nEach workflow should include commands, screenshots, and a short explanation of the resulting task state.",
        project: "docs",
        status: TaskStatus::Backlog,
        priority: TaskPriority::None,
        labels: &["docs"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "document_automation_workflows",
        title: "Document automation workflows",
        description: "## Goal\n\nDocument automation as an extension of the task workflow.\n\n### Include examples\n\n- Add a task from a script.\n- Update status from CI output.\n- Append a note from a local tool.\n\n### Tone\n\nKeep the main story about task management, with automation as a power feature.",
        project: "docs",
        status: TaskStatus::Todo,
        priority: TaskPriority::Medium,
        labels: &["docs"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "polish_the_public_launch_story",
        title: "Polish the public launch story",
        description: "## Epic outcome\n\nMake the public story clear, polished, and easy to share.\n\n### Workstreams\n\n- Homepage copy.\n- Onboarding examples.\n- Automation documentation.\n- Curated screenshots.\n\n### Success signal\n\nA new visitor can explain Aven after looking at the README for 30 seconds.",
        project: "docs",
        status: TaskStatus::Inbox,
        priority: TaskPriority::Medium,
        labels: &["docs", "release"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: true,
    },
    DemoTask {
        key: "refresh_homepage_hero_copy",
        title: "Refresh homepage hero copy",
        description: "## Goal\n\nRewrite the homepage hero so visitors understand Aven in one scan.\n\n### Message hierarchy\n\n1. Local-first task manager.\n2. One queue across projects.\n3. Terminal UI with durable task context.\n\n### Deliverables\n\n- New headline.\n- Short supporting paragraph.\n- Screenshot caption that explains what the user sees.",
        project: "docs",
        status: TaskStatus::Todo,
        priority: TaskPriority::High,
        labels: &["docs"],
        due_in_days: Some(4),
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "update_onboarding_screenshots",
        title: "Update onboarding screenshots",
        description: "## Goal\n\nRefresh the onboarding screenshots so the first impression shows a clean, realistic workflow.\n\n### Must show\n\n- Cross-project queue with **Needs Action**, **Blocked**, **Focus**, and **Soon** bands.\n- Labels, priorities, dependencies, and epic membership.\n- A selected task with useful Markdown context.\n\n### Notes\n\nUse curated demo data rather than a personal database dump.",
        project: "docs",
        status: TaskStatus::Active,
        priority: TaskPriority::Urgent,
        labels: &["docs", "release"],
        due_in_days: Some(0),
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "document_popup_capture_setup",
        title: "Document popup capture setup",
        description: "## Goal\n\nDocument how to launch Aven capture from a terminal popup.\n\n### Include\n\n```sh\naven tui\naven add \"follow up on review feedback\"\n```\n\n### Reader outcome\n\nA user can bind a hotkey, open a popup, capture a task, and return to work without changing projects.",
        project: "docs",
        status: TaskStatus::Inbox,
        priority: TaskPriority::Medium,
        labels: &["docs"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    // mobile-app
    DemoTask {
        key: "create_mobile_inbox_prototype",
        title: "Create mobile inbox prototype",
        description: "## Goal\n\nPrototype the mobile inbox before building the full companion app.\n\n### Prototype features\n\n- Add a title.\n- Save to inbox.\n- Review recent captures.\n\n### Learning goal\n\nFind the smallest phone workflow that complements the terminal UI.",
        project: "mobile-app",
        status: TaskStatus::Done,
        priority: TaskPriority::Low,
        labels: &["frontend"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "build_a_compact_review_screen",
        title: "Build a compact review screen",
        description: "## Goal\n\nCreate a compact mobile review surface for small moments between deeper work.\n\n### Views\n\n- Inbox.\n- Today.\n- Blocked.\n\n### Interaction model\n\nSwipe or tap to triage, then leave detailed editing to the terminal UI.",
        project: "mobile-app",
        status: TaskStatus::Backlog,
        priority: TaskPriority::Medium,
        labels: &["mobile"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "send_reminder_notifications",
        title: "Send reminder notifications",
        description: "## Goal\n\nSend reminder notifications for dated tasks.\n\n### Requirements\n\n- Reminders are optional.\n- Notification delivery does not control sync correctness.\n- Quiet hours are respected.\n\n### Success signal\n\nUsers can trust reminders without feeling that the app is noisy.",
        project: "mobile-app",
        status: TaskStatus::Backlog,
        priority: TaskPriority::Low,
        labels: &["mobile"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "ship_a_companion_mobile_inbox",
        title: "Ship a companion mobile inbox",
        description: "## Epic outcome\n\nShip a small mobile companion for capture and lightweight review.\n\n### Scope\n\n- Quick inbox capture.\n- Share-sheet support.\n- Compact triage view.\n- Optional reminders.\n\n### Non-goal\n\nThe mobile app does not replace the full terminal workflow.",
        project: "mobile-app",
        status: TaskStatus::Inbox,
        priority: TaskPriority::Medium,
        labels: &["ux"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: true,
    },
    DemoTask {
        key: "add_mobile_quick_inbox_capture",
        title: "Add mobile quick inbox capture",
        description: "## Goal\n\nMake phone capture fast enough for ideas, errands, and follow-ups.\n\n### Capture fields\n\n- Title.\n- Optional note.\n- Project hint.\n- Inbox default.\n\n### Product principle\n\nMobile capture should stay small and focused. The terminal remains the main place for deep triage.",
        project: "mobile-app",
        status: TaskStatus::Todo,
        priority: TaskPriority::High,
        labels: &["capture"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "support_share_sheet_task_capture",
        title: "Support share-sheet task capture",
        description: "## Goal\n\nCapture tasks from the mobile share sheet without forcing users into the full app.\n\n### Supported inputs\n\n- URLs from browsers.\n- Text snippets from notes or chat.\n- Screenshots from bug reports.\n\n### Result\n\nShared content lands in the mobile inbox with a readable title and a Markdown body.",
        project: "mobile-app",
        status: TaskStatus::Inbox,
        priority: TaskPriority::Medium,
        labels: &["mobile"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    // sync-service
    DemoTask {
        key: "ship_self_hosted_sync_server",
        title: "Ship self-hosted sync server",
        description: "## Goal\n\nShip the self-hosted server that moves changes between local databases.\n\n### Server responsibilities\n\n- Accept pushed changes.\n- Return remote changes after a cursor.\n- Preserve conflict metadata.\n\n### Success signal\n\nTwo devices can edit offline, sync later, and converge without losing task context.",
        project: "sync-service",
        status: TaskStatus::Done,
        priority: TaskPriority::High,
        labels: &["backend"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "show_connected_devices_and_last_sync",
        title: "Show connected devices and last sync",
        description: "## Goal\n\nShow which devices have synced recently.\n\n### Device row fields\n\n- Device name.\n- Last sync time.\n- Last cursor.\n- Pending local changes.\n\n### Use case\n\nA user can tell whether a laptop, desktop, or phone is stale before troubleshooting deeper.",
        project: "sync-service",
        status: TaskStatus::Backlog,
        priority: TaskPriority::Medium,
        labels: &["sync", "ui"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "make_local_first_sync_feel_invisible",
        title: "Make local-first sync feel invisible",
        description: "## Epic outcome\n\nSync feels reliable and quiet while keeping the local-first workflow intact.\n\n### Principles\n\n- Local writes stay fast.\n- Conflict handling is understandable.\n- Diagnostics are available when needed.\n\n### Success signal\n\nUsers trust that tasks are safe on one laptop and available on another.",
        project: "sync-service",
        status: TaskStatus::Inbox,
        priority: TaskPriority::High,
        labels: &["sync", "ux"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: true,
    },
    DemoTask {
        key: "keep_offline_edits_visible_until_synced",
        title: "Keep offline edits visible until synced",
        description: "## Goal\n\nShow local edits clearly while they wait for sync.\n\n### UI signals\n\n- A small pending marker in task detail.\n- Sync status in the header.\n- A diagnostics view for exact pending counts.\n\n### Acceptance criteria\n\n- [ ] Users can tell that work is saved locally.\n- [ ] Users can tell which changes still need to reach other devices.",
        project: "sync-service",
        status: TaskStatus::Todo,
        priority: TaskPriority::High,
        labels: &["sync", "testing"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "explain_sync_conflicts_in_plain_language",
        title: "Explain sync conflicts in plain language",
        description: "## Goal\n\nMake sync state understandable without requiring users to know the replication internals.\n\n### Detail view should explain\n\n- What changed locally.\n- What came from another device.\n- Which version is safe to keep.\n\n### Copy rule\n\nUse short, plain-language messages in the TUI and reserve protocol details for diagnostics.",
        project: "sync-service",
        status: TaskStatus::Todo,
        priority: TaskPriority::High,
        labels: &["docs"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "compact_sync_history_safely",
        title: "Compact sync history safely",
        description: "## Goal\n\nKeep the sync database small while preserving the history needed for conflict detection.\n\n### Approach\n\n1. Identify changes that every known device has acknowledged.\n2. Keep the newest field version for each entity.\n3. Compact old rows in batches so the app stays responsive.\n\n### Open question\n\nHow much audit history should remain visible in diagnostics?",
        project: "sync-service",
        status: TaskStatus::Inbox,
        priority: TaskPriority::None,
        labels: &["backend"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "add_encrypted_backup_export",
        title: "Add encrypted backup export",
        description: "## Goal\n\nProvide a portable encrypted backup that users can store outside the sync server.\n\n### Backup contents\n\n- SQLite database snapshot.\n- Workspace and project metadata.\n- Attachments and task-local Markdown.\n\n### Acceptance criteria\n\n- [ ] Export is encrypted before writing to disk.\n- [ ] Restore validates database integrity before replacing local state.",
        project: "sync-service",
        status: TaskStatus::Inbox,
        priority: TaskPriority::Medium,
        labels: &["security"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    // website
    DemoTask {
        key: "add_terminal_ui_screenshots",
        title: "Add terminal UI screenshots",
        description: "## Goal\n\nAdd terminal screenshots that show Aven as a polished daily tool.\n\n### Screenshots\n\n- Queue view.\n- Task detail.\n- Add-task popup.\n- Project filtering.\n\n### Requirements\n\nUse demo data that is public, readable, and representative of real work.",
        project: "website",
        status: TaskStatus::Done,
        priority: TaskPriority::Medium,
        labels: &["frontend"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "improve_docs_search_loading",
        title: "Improve docs search loading",
        description: "## Goal\n\nKeep docs search responsive as the documentation grows.\n\n### Investigation\n\n- Measure initial search bundle size.\n- Check search index generation.\n- Identify slow assets in the docs shell.\n\n### Acceptance criteria\n\nSearch opens quickly on a cold page load and remains usable on slower connections.",
        project: "website",
        status: TaskStatus::Backlog,
        priority: TaskPriority::Low,
        labels: &["bug", "frontend"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "improve_terminal_theme_contrast",
        title: "Improve terminal theme contrast",
        description: "## Goal\n\nImprove the terminal palette for screenshots and long daily sessions.\n\n### Checkpoints\n\n- Project colors are distinct.\n- Priority markers remain readable.\n- Selected rows keep enough contrast.\n\n### Test cases\n\nReview the UI in a terminal, in README-size screenshots, and in dark-mode browser previews.",
        project: "website",
        status: TaskStatus::Todo,
        priority: TaskPriority::Medium,
        labels: &["frontend"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
    DemoTask {
        key: "create_open_graph_preview_image",
        title: "Create Open Graph preview image",
        description: "## Goal\n\nCreate a social preview image that makes the project easy to recognize when links are shared.\n\n### Ingredients\n\n- Product name and tagline.\n- Curated terminal screenshot.\n- High contrast background.\n\n### Done when\n\nThe generated image works at GitHub, Slack, Discord, and Mastodon preview sizes.",
        project: "website",
        status: TaskStatus::Inbox,
        priority: TaskPriority::Medium,
        labels: &["frontend"],
        due_in_days: None,
        available_in_hours: None,
        is_epic: false,
    },
];

pub(super) const NOTES: &[DemoNote] = &[
    DemoNote {
        task: "add_due_dates_and_scheduling",
        body: "Queue ranking is wired up. I am checking the remaining date-editing edge cases.",
    },
    DemoNote {
        task: "update_onboarding_screenshots",
        body: "The queue composition is approved. The final capture needs the refreshed terminal theme.",
    },
];

pub(super) const DEPENDENCIES: &[DemoDependency] = &[
    DemoDependency {
        task: "attach_screenshots_to_task_descriptions",
        depends_on: "add_labels_in_the_task_composer",
    },
    DemoDependency {
        task: "design_recurring_task_rules",
        depends_on: "add_due_dates_and_scheduling",
    },
    DemoDependency {
        task: "explain_sync_conflicts_in_plain_language",
        depends_on: "keep_offline_edits_visible_until_synced",
    },
    DemoDependency {
        task: "send_reminder_notifications",
        depends_on: "add_mobile_quick_inbox_capture",
    },
    DemoDependency {
        task: "show_today_and_upcoming_views",
        depends_on: "add_due_dates_and_scheduling",
    },
    DemoDependency {
        task: "update_onboarding_screenshots",
        depends_on: "improve_terminal_theme_contrast",
    },
];

pub(super) const RELATED_LINKS: &[DemoRelatedLink] = &[
    DemoRelatedLink {
        task: "create_tasks_from_clipboard_text",
        related: "support_share_sheet_task_capture",
    },
    DemoRelatedLink {
        task: "update_onboarding_screenshots",
        related: "add_terminal_ui_screenshots",
    },
];

pub(super) const EPIC_LINKS: &[DemoEpicLink] = &[
    DemoEpicLink {
        epic: "bring_dates_and_scheduling_into_the_task_flow",
        child: "add_due_dates_and_scheduling",
    },
    DemoEpicLink {
        epic: "bring_dates_and_scheduling_into_the_task_flow",
        child: "design_recurring_task_rules",
    },
    DemoEpicLink {
        epic: "bring_dates_and_scheduling_into_the_task_flow",
        child: "highlight_overdue_work_in_the_queue",
    },
    DemoEpicLink {
        epic: "bring_dates_and_scheduling_into_the_task_flow",
        child: "parse_natural_language_dates_when_adding_tasks",
    },
    DemoEpicLink {
        epic: "bring_dates_and_scheduling_into_the_task_flow",
        child: "show_today_and_upcoming_views",
    },
    DemoEpicLink {
        epic: "make_capture_instant_from_anywhere",
        child: "add_a_one_line_quick_capture_command",
    },
    DemoEpicLink {
        epic: "make_capture_instant_from_anywhere",
        child: "add_labels_in_the_task_composer",
    },
    DemoEpicLink {
        epic: "make_capture_instant_from_anywhere",
        child: "attach_screenshots_to_task_descriptions",
    },
    DemoEpicLink {
        epic: "make_capture_instant_from_anywhere",
        child: "capture_tasks_from_a_tmux_popup",
    },
    DemoEpicLink {
        epic: "make_capture_instant_from_anywhere",
        child: "create_tasks_from_clipboard_text",
    },
    DemoEpicLink {
        epic: "make_local_first_sync_feel_invisible",
        child: "add_encrypted_backup_export",
    },
    DemoEpicLink {
        epic: "make_local_first_sync_feel_invisible",
        child: "compact_sync_history_safely",
    },
    DemoEpicLink {
        epic: "make_local_first_sync_feel_invisible",
        child: "explain_sync_conflicts_in_plain_language",
    },
    DemoEpicLink {
        epic: "make_local_first_sync_feel_invisible",
        child: "keep_offline_edits_visible_until_synced",
    },
    DemoEpicLink {
        epic: "make_local_first_sync_feel_invisible",
        child: "show_connected_devices_and_last_sync",
    },
    DemoEpicLink {
        epic: "polish_the_public_launch_story",
        child: "add_example_workflows_for_busy_projects",
    },
    DemoEpicLink {
        epic: "polish_the_public_launch_story",
        child: "document_automation_workflows",
    },
    DemoEpicLink {
        epic: "polish_the_public_launch_story",
        child: "document_popup_capture_setup",
    },
    DemoEpicLink {
        epic: "polish_the_public_launch_story",
        child: "refresh_homepage_hero_copy",
    },
    DemoEpicLink {
        epic: "polish_the_public_launch_story",
        child: "update_onboarding_screenshots",
    },
    DemoEpicLink {
        epic: "ship_a_companion_mobile_inbox",
        child: "add_mobile_quick_inbox_capture",
    },
    DemoEpicLink {
        epic: "ship_a_companion_mobile_inbox",
        child: "build_a_compact_review_screen",
    },
    DemoEpicLink {
        epic: "ship_a_companion_mobile_inbox",
        child: "send_reminder_notifications",
    },
    DemoEpicLink {
        epic: "ship_a_companion_mobile_inbox",
        child: "support_share_sheet_task_capture",
    },
];

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn demo_dataset_has_the_intended_shape() {
        assert_eq!(PROJECTS.len(), 6);
        assert_eq!(LABELS.len(), 16);
        assert_eq!(TASKS.len(), 41);
        assert_eq!(TASKS.iter().filter(|task| task.is_epic).count(), 5);
        assert_eq!(DEPENDENCIES.len(), 6);
        assert_eq!(EPIC_LINKS.len(), 24);
        assert_eq!(RELATED_LINKS.len(), 2);

        let keys: HashSet<_> = TASKS.iter().map(|task| task.key).collect();
        assert_eq!(keys.len(), TASKS.len());
        assert!(TASKS.iter().all(|task| PROJECTS.contains(&task.project)));
        assert!(
            TASKS
                .iter()
                .flat_map(|task| task.labels.iter())
                .all(|label| LABELS.contains(label))
        );
        assert!(
            LABELS
                .iter()
                .all(|label| TASKS.iter().any(|task| task.labels.contains(label)))
        );
        assert!(TASKS.iter().filter(|task| task.labels.len() > 1).count() >= 5);
        assert!(NOTES.iter().all(|note| keys.contains(note.task)));
        assert!(DEPENDENCIES.iter().all(|dependency| {
            keys.contains(dependency.task) && keys.contains(dependency.depends_on)
        }));
        assert!(
            EPIC_LINKS
                .iter()
                .all(|link| keys.contains(link.epic) && keys.contains(link.child))
        );
        assert!(RELATED_LINKS.iter().all(|link| {
            keys.contains(link.task) && keys.contains(link.related) && link.task != link.related
        }));

        let related_pairs: HashSet<_> = RELATED_LINKS
            .iter()
            .map(|link| {
                if link.task < link.related {
                    (link.task, link.related)
                } else {
                    (link.related, link.task)
                }
            })
            .collect();
        assert_eq!(related_pairs.len(), RELATED_LINKS.len());
        assert!(RELATED_LINKS.iter().any(|link| {
            let task_project = TASKS
                .iter()
                .find(|task| task.key == link.task)
                .unwrap()
                .project;
            let related_project = TASKS
                .iter()
                .find(|task| task.key == link.related)
                .unwrap()
                .project;
            task_project != related_project
        }));
        assert!(RELATED_LINKS.iter().all(|related| {
            DEPENDENCIES.iter().all(|dependency| {
                (dependency.task, dependency.depends_on) != (related.task, related.related)
                    && (dependency.task, dependency.depends_on) != (related.related, related.task)
            }) && EPIC_LINKS.iter().all(|epic| {
                (epic.epic, epic.child) != (related.task, related.related)
                    && (epic.epic, epic.child) != (related.related, related.task)
            })
        }));
    }

    #[test]
    fn demo_dataset_populates_current_tui_surfaces() {
        assert!(TASKS.iter().any(|task| task.due_in_days == Some(-2)));
        assert!(TASKS.iter().any(|task| task.due_in_days == Some(0)));
        assert!(
            TASKS
                .iter()
                .any(|task| task.due_in_days.is_some_and(|days| days > 0))
        );
        assert!(
            TASKS
                .iter()
                .any(|task| task.available_in_hours.is_some_and(|hours| hours < 0))
        );
        assert!(
            TASKS
                .iter()
                .any(|task| task.available_in_hours.is_some_and(|hours| hours > 0))
        );
        assert!(
            TASKS
                .iter()
                .filter(|task| {
                    matches!(task.status, TaskStatus::Inbox | TaskStatus::Backlog)
                        && task.priority == TaskPriority::None
                })
                .count()
                >= 4
        );
        assert!((2..=3).contains(&NOTES.len()));
        assert!(NOTES.iter().all(|note| {
            TASKS
                .iter()
                .any(|task| task.key == note.task && task.status == TaskStatus::Active)
        }));
    }
}
