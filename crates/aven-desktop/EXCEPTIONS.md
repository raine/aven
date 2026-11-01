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
| Controls owned by later phases | Header, palette, overflow menu | Review tasks, epics, marks and gist publishing arrive with workflows; Settings, Sync and About arrive with their phases. The capability map assigns each one. | `capabilities.toml` |
| Schedule dates are text fields | Schedule dialog | The kit has no native date picker. Each field accepts `YYYY-MM-DD` and the quick choices fill them in. | `schedule.gpui.png` |
| Schedule saves in two writes | Schedule dialog | Core stores availability and due date through separate undoable writes, so one undo reverts the last of them. | `src/dialogs.rs` |
| One label and one priority filter | Filter menu | Core's `TaskFilters` holds one of each, where the prototype keeps lists. | `queue-filtered.gpui.png` |
| Columns merge Done and Canceled | Board | Core's column configuration has one closed lane. | `views-columns.gpui.png` |
| Changing alt text replaces the attachment | Image viewer | Core has no alt update, so a change stores a new attachment with a new id and cannot be undone. | `src/adapter.rs` |
| Chords lapse after one second | Keyboard | GPUI drops a pending chord after its fixed timeout. | `src/actions.rs` |
| Blocker cycles beyond one step | Blocker menu | Only a direct cycle is greyed out. Longer ones reach core, which refuses the write with its error. | `src/commands.rs` |
| Metadata and Epic composer chips | Composer | The terminal add form offers both, so More details adds them for parity. | `composer-more.gpui.png` |
| Search result order | Search | Order comes from core's `search_tasks`, and the prototype scores its simulated data its own way. | `search-results.gpui.png` |
| Recent entries | Recent actions | The list reads core's history, which on the fixture holds the seeding writes, where the prototype lists two sample entries. | `views-recent.gpui.png` |
