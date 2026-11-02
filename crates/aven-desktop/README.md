# aven-desktop

The GPUI desktop app. This macOS-only library is linked into `aven` by the
`desktop` feature, and `aven` starts it when Launch Services opens the bundle.
On other platforms the crate is empty.

Run its tests from the repository root:

```sh
just _test --package aven-desktop --lib --test session
```

Install a release build from this checkout to `/Applications`:

```sh
just install-app
```

This also relinks `~/.cargo/bin/aven` to the new bundle, reinstalls the daemon
when sync is enabled, and removes `~/.local/share/aven/Aven.app`. Quit a running
app first; it keeps its old executable until restarted.

## Startup timing

Quit the app, then launch a release bundle with opt-in timing:

```sh
logs="$(mktemp -d)"
open --env AVEN_STARTUP_TIMING=1 --stderr "$logs/startup.log" /Applications/Aven.app
```

Milestones cover runtime setup, database open (including migrations), the initial
projection, native platform and window creation, fonts, and the first render.
`elapsed_ms` starts at entry to the desktop library; `unix_ms` lets an external
launcher include process-launch overhead. Database loading and native platform
initialization overlap, so their durations must not be added together.

`first-frame-ready` is GPUI's next-frame callback after initial layout, before
presentation. It is not a GPU-completion or display-scanout measurement. Timing
is disabled by default and records no task data. No extra frame callbacks are
scheduled when it is disabled.

Use a populated, local-only backup for isolated benchmarks. Copying an enrolled
SQLite database alone does not copy its protected sync keys and can produce a
recovery screen instead of a populated window. Check the actual window, and
report first launch separately from repeated launches; neither establishes a
reboot-cold OS or Metal cache. This is a native GPUI app, with no JavaScript
bundle or frontend hydration stage.
