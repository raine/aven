# Changelog

## v0.1.0 (2026-07-02)

- Initial release of `aven`, a local-first task manager backed by SQLite with stable task refs, projects, labels, Markdown descriptions, and append-only notes.
- Added an agent-friendly CLI for listing, searching, creating, updating, deleting, restoring, and inspecting tasks.
- Added a keyboard-first TUI with project views, filters, sorting, detail view, undo, command search, mouse support, and fast task capture.
- Added workspace isolation so personal and work task sets can share one tool while staying separate.
- Added optional self-hosted sync with a background daemon, conflict reporting, bounded sync batches, and macOS LaunchAgent install commands.
- Added task dependencies and epics for modeling blocked work and larger bodies of work.
- Added full-text task search, deleted-task filters, structured JSON output, and agent primer/context commands.
- Added data safety commands for database backup, restore, export, import, and integrity workflows.
- Added tmux popup task capture and natural-language task intake.
- Added install and release packaging support, including Homebrew and shell installer paths.
