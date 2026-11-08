# aven-desktop

The GPUI desktop app. This library is linked into `aven` by the `desktop`
feature on macOS and Linux. On macOS `aven` starts it when Launch Services opens
the bundle. On Linux it starts when run with no arguments and no terminal in a
graphical session, as a desktop launcher runs it. On other platforms the crate
is empty.

macOS services the app manages sit behind `src/platform.rs`, with Linux
stand-ins in `src/platform_linux.rs`. On Linux the app has no install setup,
in-app updates, background agent management or system-wide shortcut, and its
notifications go to the freedesktop notification service without click
handling.

Building on Linux needs a glibc target and the development packages for
xkbcommon (with its X11 support), Wayland, xcb, fontconfig and freetype. At run
time GPUI renders through Vulkan or OpenGL, so it also needs a Vulkan or EGL
driver, such as Mesa's.

Run its tests from the repository root:

```sh
just _test --package aven-desktop --lib --test session
```

Install a release build from this checkout to `/Applications`:

```sh
just install-app
```

This then runs the app's setup from the terminal: links to the old bundle and
the daemon's LaunchAgent move to the new one, keeping the daemon's settings and
whether it runs, `~/.local/share/aven/Aven.app` is removed, and
`~/.local/bin/aven` is created if no `aven` command runs the app. A running
daemon restarts on the new build. Quit a running app first; it keeps its old
executable until restarted. With `/Applications/Aven.app` installed, `just
install` refuses, since the app owns the command and the daemon.

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
