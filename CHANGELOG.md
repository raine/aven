# Changelog

## v0.1.1 (2026-07-03)

- Made natural task creation from the full TUI continue reliably after exiting
  the interface, while keeping created tasks undoable when the TUI remains open.
- Improved daemon update handling so launchd services use a stable executable
  path, restart cleanly, and show richer status in `aven doctor`.

## v0.1.0 (2026-07-02)

- Initial release of `aven`, a local-first task manager.
