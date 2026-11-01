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
| Details actions and sections not built yet | Details | Expand, Add, more actions, images, related, Activity and Description Edit belong to later phases. | `crates/aven-desktop/capabilities.toml` |
| Controls that do nothing yet | Header, nav | Review tasks, Filter, Search, ⌘K, view, workspace and project rows and Settings render for layout and are wired by later phases. | `captures/foundation-taste.md` |
| Copy ref surface | Details | Clicking the details ref copies it. The prototype copies from the more actions menu, which arrives with the details actions. | `src/details.rs` |
| Rows measured to row 18 | Scenes | The list is virtual, so rows below the fold are never painted. `row-18` is the last fully visible measured row at 1440x900. | `scenes/manifest.toml` |
| Narrow header alignment | Header | Header actions stay right-aligned at every width. The prototype packs them beside the title once the crumbs hide, which reads as an artefact of its collapsed spacer. | `queue-narrow.gpui.png` |
| Done group in the queue | Queue list | The prototype drops a completed row at once. The app keeps it in a Done group at the end of the list until the user refreshes the view, by owner direction. | `queue-completed.gpui.png` beside `queue-completed.prototype.png` |
