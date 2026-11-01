# aven-desktop

The GPUI desktop app, and the harness it is developed in. This crate is a macOS-only
member of the root workspace and a library: `aven` links it behind the `desktop`
feature and starts it when Launch Services opens the bundle. On other platforms the
crate is empty, and root default builds and `just` checks leave it out.

## Builds

| Build | Command | Contents |
| --- | --- | --- |
| Shipped | `cargo build --release --features desktop` | The app, on the owner's real data, with no isolation guard |
| Development | `scripts/desktop/bundle` (feature `desktop-dev`) | The same binary as `Aven Dev.app`, which refuses to run without a disposable root |
| CLI only | `cargo build` | No GPUI |

The crate's default `dev` feature owns the guard, launch records and the capture
tools. `aven` depends on the crate without default features, and
`scripts/verify-macos-release` refuses a binary containing the guard.

## Mode selection

`src/launch_mode.rs` chooses before anything else runs. Arguments or a terminal mean
the CLI. With neither, inside a bundle, the process is the app only if launchd is its
parent and its job is named `application.<bundle identifier>.…`, which is how Launch
Services runs an app. Any other bare caller, such as cron or `ssh -T` through the bin
symlink, exits quietly. A clicked notification opens the app.

GPUI's load-time constructors register Objective-C subclasses in every process, and
the binary links AppKit, but only the app creates `NSApplication` or reaches the
window server.

## Updates

Whatever replaces the bundle, `aven update` or `scripts/install`, the running app
notices that its executable's path now holds a different file. It quits normally and
reopens the next time it is not the active app. Homebrew installs to a new versioned
path, so there the app runs the old version until it is reopened.

## Commands

Run these from the repository root. The prototype lives in `history/`, so capture
needs a worktree with that directory.

| Command | What it does |
| --- | --- |
| `scripts/desktop/check` | Full harness validation |
| `scripts/desktop/cargo test --features capture` | Crate tests |
| `scripts/desktop/cargo build [--features capture]` | Build through the shared cache |
| `scripts/desktop/bundle` | Build `Aven Dev.app` |
| `scripts/desktop/launch [--seed] [--check-only] [--background]` | Start it on a new disposable root |
| `scripts/desktop/capture OUTPUT [SCENE]` | Paired screenshots and box reports |
| `scripts/desktop/sync-proof` | Two clients through a disposable server |
| `scripts/desktop/app-proof start` | Disposable server and two clients to run the app against by hand; its docstring lists the other subcommands |
| `scripts/desktop/app-proof drive` | Setup, join, Sync now and device removal through the app's store and session |
| `crates/aven-desktop/target/bin/aven-desktop-capture check-capabilities` | Compare the capability map with the prototype |
| `crates/aven-desktop/target/bin/aven-desktop-capture check-menus` | Check that every menu item reaches a handler, and that task commands stand down behind the composer |

`scripts/desktop/cargo` keeps build artifacts in
`~/Library/Caches/aven-desktop/target`, so worktrees share compiled GPUI, and copies
the binaries each build produced to this worktree's `target/bin`. A lock serialises build plus copy,
so concurrent worktrees wait for each other.

## Isolation

`src/isolation.rs` is the guard, in development builds only. The app runs it before
anything else, so `Aven Dev.app` refuses when started without the launcher. Everything sits under
`AVEN_DESKTOP_ROOT`: state, config, database, blobs, logs, server storage and
file-backed keys. The guard exits with status 78 when a resolved path overlaps the
owner's data, when a path contains `.aven`, when the root contains `..`, when the
environment points outside the root or sets a variable that moves storage, or when
the root's config is anything but `sync.enabled: false`, a private loopback wake port
and storage paths inside the layout. It also refuses any link below the root and a
`TMPDIR` or capture output in owner data. Capture output outlives any root, so it
must also hold no links, and a relative output resolves against the working directory
before the check.

Every path a launch, capture or script writes is a field of `Isolation`. Scripts read
them from `aven-desktop --print-paths`, which runs the guard first, so a new path is
validated by adding it to `resolved_paths`.

Owner data comes from three sources, all read-only:

- fixed paths in `OWNER_PATHS` and `OWNER_SYSTEM_PATHS`, audited from every home and
  XDG lookup in the root and core crates. `every_owner_path_source_is_audited` fails
  when a new lookup appears, until it is audited,
- names starting with `aven`, `com.raine.aven` or `fi.zendit.aven` directly in the
  home or in `Library` directories that macOS keys by app,
- whatever the owner's `config.yaml` and installed daemon plist name: the database
  with its sidecars, blobs, config directory, logs and wake port.

Owner directories come from the password database rather than `$HOME`. Paths are
compared by device and inode of each resolved component, following every symlink,
so links, firmlinks and case cannot respell owner data. Components that do not
exist yet compare by name, ignoring case.

`--check-paths` runs the path checks alone, and `--check-isolation` adds the config.
Neither writes anything.

Agents never run the desktop against the owner's database, Keychain, daemon, `Aven.app` or
`fi.zendit.Aven`.

## Scenes

`scenes/manifest.toml` names the elements and scenes. A run:

1. picks an anchor at the current minute, refusing within 10 minutes of local
   midnight,
2. shifts `scenes/fixture.json` to it, seeds a disposable database through
   `aven-core`, and backdates activity through `test_support::pool`,
3. renders the prototype in headless Chrome with `Date` frozen at the anchor and
   the same data in `localStorage`, using one DevTools session for the exact
   viewport, the page's result and the screenshot, so they show one page state,
4. renders the GPUI shell offscreen with the same anchor, then writes
   `<scene>.prototype.png`, `<scene>.gpui.png`, `<scene>.measure.json` and
   `report.json`.

**Clock.** Scenes use relative fixtures, not an injected clock, because core reads
wall time throughout. Timestamps move by the exact offset and dates by whole days,
so each keeps its distance from now. A test keeps fixture timestamps away from the
frozen clock, so the seconds between anchor and query cannot change a result.

**Fixture limits.** The prototype's copy omits conflicts, series and recurrence
until the phase that seeds them. `report.json` fails on any queue section whose
tasks differ between core and prototype.

**Scene input.** `gpui_keys` runs after launch, and with no store behind the view,
its own list stands in for each write's reply. `gpui_pointer` rests the pointer, and
`gpui_frame_ms` takes the picture that long after the keys, to catch a motion in
flight. Scenes with `prototype_steps` skip the data check, which compares seeded
state.

**Prototype requirements.** Chrome, or `AVEN_DESKTOP_CHROME` pointing at a Chromium
binary. It runs with a throwaway profile and the web font link removed, so results
do not depend on the network.

`aven-desktop-capture export-fixture` regenerates the fixture when the prototype
changes.

## Measurement

Each scene compares the left, top, width and height of its elements. A box missing
from either side fails, and elements under `absent` fail if either side draws them.
Scenes measure their last visible row, where drift down the list is largest.

**Tolerance.** `default_px` is 1.0 logical pixel, which absorbs pixel snapping at
2x and no more. An element may set its own `tolerance_px` only with an entry in
`EXCEPTIONS.md`.

**Probe limit.** GPUI's frame bounds are private, so `measure::measured` records
prepaint bounds through a canvas inside the element. The canvas sits inside any
border or padding, so measured elements carry none. It records nothing in the
application binary, which has no `capture` feature.

## Capability map

`capabilities.toml` links each prototype action to its core or host operation,
owning phase, acceptance and scenes. `check-capabilities` fails on missing or stale
entries, comparing the map with an inventory of the frozen prototype:

- `act:` and `chord:`, from the running page's tables,
- `key:`, `composer:` and `palette:`, from the keymap, composer and palette source,
- `dialog:`, `field:`, `listener:`, `handler-key:` and `attribute:`, from every
  `data-d` control, input with an id, assigned or added handler, key read outside
  the main keymap, and other data attribute. Ids name the owning function, because
  names such as `ok` repeat across dialogs.

Typed text, browser defaults and handlers built from computed names are not
inventoried, and the check says so in its output. Pointer gestures have no id in the
prototype, so they use hand-written `gesture:` entries. Demo controls are `test_only`.
Scene names may be absent from the manifest.

## Exceptions

`EXCEPTIONS.md` lists accepted differences. Decision 18 findings are in
`docs/decision-18.md`.
