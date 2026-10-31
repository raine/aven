# aven-desktop

The GPUI desktop app, and the harness every desktop phase works in. This crate is
its own Cargo workspace, excluded from the root, so the root lockfile, CI, Nix and
`cargo-deny` do not see GPUI.

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
| `crates/aven-desktop/target/bin/aven-desktop-capture check-capabilities` | Compare the capability map with the prototype |

`scripts/desktop/cargo` keeps build artifacts in
`~/Library/Caches/aven-desktop/target`, so worktrees share compiled GPUI, and copies
final binaries to this worktree's `target/bin`. A lock serialises build plus copy,
so concurrent worktrees wait for each other.

## Isolation

`src/isolation.rs` is the guard. The app runs it before anything else, so it also
refuses when started without the launcher. Everything sits under
`AVEN_DESKTOP_ROOT`: state, config, database, blobs, logs, server storage and
file-backed keys. A launch has its own wake port. The guard exits with status 78
when a resolved path overlaps the owner's directories, which it derives from the
password database rather than `$HOME`, when a path contains `.aven`, or when the
environment points outside the root.

Never run the desktop against the owner's database, Keychain, daemon, `Aven.app` or
`fi.zendit.Aven`.

## Scenes

`scenes/manifest.toml` names the elements and scenes. A run:

1. picks an anchor at the current minute, refusing within 10 minutes of local
   midnight,
2. shifts `scenes/fixture.json` to it, seeds a disposable database through
   `aven-core`, and backdates activity through `test_support::pool`,
3. renders the prototype in headless Chrome with `Date` frozen at the anchor and
   the same data in `localStorage`,
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

**Prototype requirements.** Chrome, or `AVEN_DESKTOP_CHROME` pointing at a Chromium
binary. It runs with a throwaway profile and the web font link removed, so results
do not depend on the network.

`aven-desktop-capture export-fixture` regenerates the fixture when the prototype
changes.

## Measurement

Each scene compares the left, top, width and height of its elements. A box missing
from either side fails.

**Tolerance.** `default_px` is 1.0 logical pixel, which absorbs pixel snapping at
2x and no more. An element may set its own `tolerance_px` only with an entry in
`EXCEPTIONS.md`.

**Probe limit.** GPUI's frame bounds are private, so `measure::measured` records
prepaint bounds through a canvas inside the element. The canvas sits inside any
border or padding, so measured elements carry none. It records nothing in the
application binary, which has no `capture` feature.

## Capability map

`capabilities.toml` links each prototype action to its core or host operation,
owning phase, acceptance and scenes. `check-capabilities` extracts action ids from
the frozen prototype and fails on missing or stale entries. Gestures have no id in
the prototype, so they use `gesture:` entries. Demo controls are `test_only`. Scene
names are plans until the owning phase adds them to the manifest.

## Exceptions

`EXCEPTIONS.md` lists accepted differences. Decision 18 findings are in
`docs/decision-18.md`.
