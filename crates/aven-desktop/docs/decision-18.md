# One `Aven.app`, one binary: findings for integration

Decision 18 ships the desktop inside the existing bundle, with one executable,
`Contents/MacOS/aven`, that starts the GUI when launched as an app. Checked
against `aven update`, `scripts/install`, Homebrew and `src/daemon/service.rs`.

**Result: the decision holds.** No path below forces a second executable or a
bundle change. Integration has to make the five changes under "Required".

## What already fits

- **Update.** `src/update/bundle.rs:install` replaces the whole bundle with
  `RENAME_SWAP`, so everything in it moves together. It runs the staged binary
  as `aven --version`, which has an argument and so stays in CLI mode.
- **Install script.** `scripts/install` swaps the whole bundle too and links
  `aven` in the bin directory to `Contents/MacOS/aven`. It reads no plist keys,
  so a GUI-capable bundle installs unchanged.
- **Homebrew.** `scripts/homebrew-formula` installs `Contents` and symlinks the
  executable, and checks the identifier `fi.zendit.Aven`, which stays.
- **Daemon.** `src/daemon/service.rs` always passes arguments, keys bundle
  detection on `app_bundle::enclosing_bundle`, and associates the agent with
  `fi.zendit.Aven`. One executable and one identifier keep the login item,
  repair and migration logic as they are. An update does not touch the daemon,
  which restarts on the new executable by itself.
- **Identity.** `src/notification.rs` needs the bundle identifier, and the
  Keychain identity follows the signature. A single signed executable keeps
  both.
- **Case-insensitive volumes.** `enclosing_bundle` requires the filename `aven`
  and `CFBundleExecutable` is `aven`, so there is no `Aven` beside it.

## Required

1. **Replace the hook in `launched_as_bare_app`, and tighten it.** `src/lib.rs`
   already detects "no arguments, no `/dev/tty`, inside a bundle" and exits
   quietly. That is the natural mode switch, but the same conditions hold for
   `aven` run through the bin symlink with no terminal, such as cron, a launchd
   job or `ssh -T`, because `enclosing_bundle` is given the canonical path.
   Today those exit quietly. With the GUI they would connect to the window
   server. Mode selection needs a further signal that Launch Services started
   the process, tested with each of those callers.
2. **Decide the notification click.** The comment on `launched_as_bare_app`
   records that clicking a notification launches this executable. That launch
   exits quietly today and would open the GUI. Opening the app is the likely
   behaviour to keep, but it changes what a click does for CLI-only users.
3. **Keep `LSUIElement` and set the activation policy.** `macos/Info.plist` sets
   `LSUIElement=true`, which hides the Dock icon and menu bar. Removing the key
   changes the identity of every bundle launch, including notification clicks
   and the notification code in the daemon. Keeping it and setting a regular
   activation policy from GUI mode confines the change to GUI mode. Confirm on
   the real bundle that the Dock icon and menu bar appear on first launch.
4. **Ship only regular files in the bundle.** `read_app_archive` and
   `extract_app` both reject symlinks and hard links, and a framework bundle
   needs symlinks. GPUI links statically and the harness uses runtime shaders,
   so this is met as long as nothing adds an embedded framework.
5. **Move the Cargo setup into the root at the merge.** The feature must be
   macOS-only so Linux, Nix and container builds keep no GPUI. The Zed
   `[patch.crates-io]` entries for `async-task`, `async-process` and `calloop`
   apply to the root workspace, and `deny.toml` has to accept the new git
   sources and licences. `--version` and every CLI path must not reach AppKit.

## To measure at integration

- **Archive limits.** `aven update` accepts at most a 64 MiB archive
  (`ARCHIVE_LIMIT`) and 256 MiB extracted (`EXTRACTED_LIMIT`). Compare a release
  archive with GPUI linked against them before release. A limit raised in the same release cannot help
  older installs, which enforce the old value when they download it.
- **`--version` start time.** Validation allows 5 seconds. A larger binary must
  still pass on a first, unscanned launch.
- **CLI start time.** Every `aven` call now maps the larger binary.

## Accepted behaviour

- **A running GUI during update.** The swap keeps the old executable mapped, so
  the app runs old code until relaunched while bundle resources on disk are the
  new ones. Nothing relaunches it. The GUI should load no bundle resources
  lazily, or should notice the version change and offer a restart.
- **Downgrade refusal.** `scripts/install` refuses to put a bare binary over a
  bundle install, which also protects a GUI install.
