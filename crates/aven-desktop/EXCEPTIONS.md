# Exceptions

Differences from the prototype that are accepted, each with a reason and the
evidence that shows it. Dropped behaviour, broken persistence and inaccessible
controls cannot be excepted. Phases add entries here as they find them.

| Difference | Where | Reason | Evidence |
| --- | --- | --- | --- |
| Letter-spacing | All text | GPUI text has no letter-spacing. Glyph widths stay close, and text boxes are measured by their container. | `queue-*.gpui.png` beside `queue-*.prototype.png` |
| Glyph rasterisation | All text | Core Text in GPUI and Chrome's text stack anti-alias differently, so pixels never match. Screenshots are compared by eye, boxes by measurement. | Paired screenshots |
| Hairlines are elements | Shell dividers | A GPUI border sits inside the capture probe's bounds, which would hide a box difference. Elements give the same 1px line and a measurable box. | `src/shell.rs` |
