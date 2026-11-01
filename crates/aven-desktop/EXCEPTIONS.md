# Exceptions

Differences from the prototype that are accepted, each with a reason and the
evidence that shows it. Dropped behaviour, broken persistence and inaccessible
controls cannot be excepted. Phases add entries here as they find them.

| Difference | Where | Reason | Evidence |
| --- | --- | --- | --- |
| Letter-spacing | All text | GPUI text has no letter-spacing. Glyph widths stay close, and text boxes are measured by their container. | `queue-*.gpui.png` beside `queue-*.prototype.png` |
| Glyph rasterisation | All text | Core Text in GPUI and Chrome's text stack anti-alias differently, so pixels never match. Screenshots are compared by eye, boxes by measurement. | Paired screenshots |
| Hairlines are elements | Shell dividers | A GPUI border sits inside the capture probe's bounds, which would hide a box difference. Elements give the same 1px line and a measurable box. | `src/shell.rs` |
| Row order within a queue group | Queue list | Order comes from core's `queue_report`, and the prototype sorts its simulated data its own way. Both show the same tasks in the same groups, and `data_parity` fails if they differ. Boxes are compared by position, so row height and spacing are still measured. | `queue-*.measure.json`; for example row 0 is a different task on each side |
| Workspace list includes `default` | Nav | Core always keeps a default workspace and the nav lists what the store holds. The prototype's simulated data has none, so nav sections below it sit one row lower in scenes. | `queue-dark.gpui.png` |
| Label order on a row | Queue list | Labels follow core's order, per Decision 3, and the prototype orders its simulated data its own way, so a row can read `android ios` beside `ios android`. | `queue-dark.gpui.png` |
| Epics count | Nav | The count is core's open epics, which differs from the prototype's simulated count by one on the fixture. | `queue-dark.gpui.png` |
| Row date tone and project order | Queue, nav | Tone follows the prototype's day rule on core's dates; project order is core's. | `src/adapter.rs` |
| Rows measured to row 18 | Scenes | The list is virtual, so rows below the fold are never painted. `row-18` is the last fully visible measured row at 1440x900. | `scenes/manifest.toml` |
| Narrow header alignment | Header | Header actions stay right-aligned at every width. The prototype packs them beside the title once the crumbs hide, which reads as an artefact of its collapsed spacer. | `queue-narrow.gpui.png` |
| Done group in the queue | Queue list | The prototype drops a completed row at once. The app keeps it in a Done group at the end of the list until the user refreshes the view, by owner direction. | `queue-completed.gpui.png` beside `queue-completed.prototype.png` |
| Schedule dates are text fields | Schedule dialog | The kit has no native date picker. Each field accepts `YYYY-MM-DD` and the quick choices fill them in. | `schedule.gpui.png` |
| Schedule saves in two writes | Schedule dialog | Core stores availability and due date through separate undoable writes, so one undo reverts the last of them. | `src/dialogs.rs` |
| One label and one priority filter | Filter menu | Core's `TaskFilters` holds one of each, where the prototype keeps lists. | `queue-filtered.gpui.png` |
| Columns merge Done and Canceled | Board | Core's column configuration has one closed lane. | `views-columns.gpui.png` |
| Changing alt text replaces the attachment | Image viewer | Core has no alt update, so a change stores a new attachment with a new id and cannot be undone. | `src/adapter.rs` |
| Chords lapse after one second | Keyboard | GPUI drops a pending chord after its fixed timeout. | `src/actions.rs` |
| Blocker cycles beyond one step | Blocker menu | Only a direct cycle is greyed out. Longer ones reach core, which refuses the write with its error. | `src/commands.rs` |
| Metadata and Epic composer chips | Composer | The terminal add form offers both, so More details adds them for parity. | `composer-more.gpui.png` |
| Capitalised activity lines | Details | Core's summaries are lowercase for the terminal, and the app capitalises each line to match `Created from`. | `src/adapter.rs` |
| Search result order | Search | Order comes from core's `search_tasks`, and the prototype scores its simulated data its own way. | `search-results.gpui.png` |
| Recent entries | Recent actions | The list reads core's history, which on the fixture holds the seeding writes, where the prototype lists two sample entries. | `views-recent.gpui.png` |
| Fixture note and activity times | Details | The seed writes notes and history at seeding time, so they read `just now` and tie on time, where the prototype shows sample ages. Real notes order by their stored time. | `detail-notes.gpui.png` |
| Status menu anchor from the keyboard | Status menu | `s` opens the menu at the Status property, beside the value it changes, where the prototype opens it under the row. | `menu-status.gpui.png` |
| Completions landing in one list | Done group | Tasks completed faster than a list refresh keep list order among themselves, because the group orders by when each list first shows a task closed. | `src/session.rs` |
| Template edit covers the description | Recurring detail | The prompt edits the text later tasks start from. Title, priority and labels of a series stay as they are; the terminal form that edits them is not rebuilt here. | `src/workflows.rs` |
| Epics list has no groups | Epics | Core returns the Epics list ungrouped, so the prototype's three epic groups do not appear. | `epics.gpui.png` |
| Series history starts empty in scenes | Recurring detail | Core records history as occurrences resolve, so the seeded series have no dated rows and refs are core's own. | `recurring-list.gpui.png` |
| Row leaves on a local write only | List | A row that leaves through sync or another device goes at once, so a refresh never replays the motion. | `src/shell.rs` |
| Conflict sides name no device or time | Resolve dialog | Core keeps which side is local and a change token, with no device name or time on the conflict, so sides read `This device` and `Other device`. | `conflict-resolve.gpui.png` |
| No word-level diff in conflicts | Resolve dialog | The sides show each whole value. The dialog keeps both values in full, so the choice stays readable without the highlight. | `conflict-resolve.gpui.png` |
| Review keys are modal | Triage | While a review runs only its own keys are live, so `t`, `d` or `a` cannot reach the list behind the card. | `src/actions.rs` |
| Settings and Sync focus the first control | Sync dialog | Opening a dialog puts keyboard focus on a control, so the prototype's dialog-wide focus ring does not appear. | `sync-local.gpui.png` |
| Devices show no last seen | Sync tab, Sync dialog | Core's device list has no last-seen time and adding one is a stored format change. Device icons are one generic icon, because a device carries only its label. | `settings-sync.gpui.png` |
| Project dots stay derived | Nav, Projects tab | The store holds no project colour, so New project has no colour field and a dot comes from the prefix. | `settings-projects.gpui.png` |
| No Define field | Metadata fields tab | Core has no field without a value, so a field appears once a task carries it. The tab renames existing fields. | `settings-fields.gpui.png` |
| Custom commands are listed from config | Custom commands tab | Commands come from the shared config, which scenes leave empty, and the tab has no editor in the prototype either. | `settings-commands.gpui.png` |
| Fixture sync facts | Sync scenes | Scenes run offline, so the capture sets the status and device list. Setup, join, removal and Sync now are shown against a real server by `scripts/desktop/app-proof drive`. | `src/bin/capture.rs` |
| Join cannot be abandoned | Sync dialog | Core refuses a reset during a join, so the interrupted join offers Resume and a new invitation in place of Abandon. The earlier database stays listed under Data and recovery. | `src/settings.rs` |
| Sample data row replaced | Data and recovery | The prototype's Restore sample data has no product meaning. That section holds Reset sync data, with the reset the TUI offers. | `data-recovery.gpui.png` |
| Database gate names one failure | Database gate | The app shows core's message with one body, where the prototype words each simulated failure. | `data-recovery-gate.gpui.png` |
| Chosen database lasts for the session | Data and recovery | A launch finds its database the way the CLI does, so a joined or opened database is listed under Other database and reopened from there. | `src/adapter.rs` |
| Version reads 0.0.0 | About, Settings | The desktop crate has its own workspace and version until it joins the root workspace. | `about.gpui.png` |
| Created line names no source | Details activity | Core has no desktop source and adding one is a stored format change, so a task made here stores `unknown` and reads `Created`. | `src/adapter.rs` |
| No focus ring on About | About | Opening a dialog puts focus inside it, so the prototype's dialog-wide ring does not appear. | `about.gpui.png` |
| Menu bar reaches task commands behind a popup menu | Menu bar | Palette entries dispatch from a frame that still shows the popup, so handlers stay registered while one is open. Composer, dialogs, Settings and review do switch them off. | `src/shell.rs` |
| Unresolved description returns as a note | Quit | A description waiting on an incoming change cannot be committed, so its text is kept as a note draft on the task. | `src/shell.rs` |
