# CuaBot sandbox verification

CuaBot provides an isolated Ubuntu desktop for manually testing aven without
interacting with the host terminal, database, cache, or installed executable.
It runs an ARM64 Linux container under OrbStack and streams application windows
to macOS through Xpra.

Use this workflow for TUI runtime verification, screenshots, destructive update
fixtures, and Linux-specific behavior.

## Architecture

The local stack has four layers:

1. OrbStack provides the Docker daemon.
2. `trycua/cuabot:latest` provides an Ubuntu desktop container.
3. Xpra streams individual sandbox windows to macOS.
4. The CuaBot CLI sends keystrokes, clicks, shell commands, and screenshot
   requests through the CuaBot server.

The sandbox session name determines its server state and container name. A
session named `aven-update` uses the container
`cuabot-xpra-aven-update`.

## Persistence model

CuaBot recreates its named container when a server starts. Installing packages
interactively inside a running container therefore lasts only until that
container is recreated.

Aven uses a derived image, `aven-cuabot:latest`, to persist Kitty, terminal
fonts, and other desktop dependencies. `scripts/cua-sandbox start` tags that
image as CuaBot's expected `trycua/cuabot:latest` before every launch. CuaBot can
then recreate the container without losing the provisioned tools.

State has three lifetimes:

- Xpra, OrbStack, Playwright browsers, and npm caches persist on the host.
- Kitty, fonts, and apt packages persist in `aven-cuabot:latest`.
- Test binaries, databases, caches, and fixtures live in the named container and
  are disposable.

Rebuild the derived image only when `sandbox/cua/Dockerfile` changes or the image
is removed. Recreate test data after a CuaBot container replacement.

## Multi-agent coordination

The derived image is shared, but every agent gets a distinct named session. A
session owns its server process, port files, Docker container, Xpra window, test
data, and input stream. Two agents must not send input to the same session at
the same time.

Choose a descriptive unique name such as `aven-auto-update-a1` or
`aven-header-review-b2`. The wrapper requires a session name and refuses to
start a second server when that name is active.

Discover existing sessions before starting work:

```sh
scripts/cua-sandbox list
```

The listing reports the session name, whether its server PID is active, its host
port, and container state. An active session belongs to its creator. Treat it as
unavailable unless the creator or user explicitly hands it off. Starting a fresh
session is cheap because all sessions share `aven-cuabot:latest` and the host
Playwright cache.

For an explicit handoff, the receiving agent uses the existing name with
`status`, `cua`, and `container`. It does not call `start` again:

```sh
scripts/cua-sandbox status aven-auto-update-a1
scripts/cua-sandbox cua aven-auto-update-a1 --screenshot /tmp/handoff.jpg
scripts/cua-sandbox container aven-auto-update-a1
```

Stop a session when its owner finishes. This removes ambiguity for later agents
and releases its container resources.

## One-time host setup

Install Xpra:

```sh
brew install --cask xpra
xattr -cr /Applications/Xpra.app
```

Start OrbStack through Cua Driver and wait for Docker:

```sh
cua-driver launch_app '{"bundle_id":"dev.kdrag0n.MacVirt","name":"OrbStack"}'
until docker info >/dev/null 2>&1; do sleep 2; done
```

Provision the host CLI, matching Playwright browser, CuaBot settings, and derived
image:

```sh
just cua-sandbox-setup
```

The setup command stores host-side CLI packages under `target/cua-sandbox`,
which is ignored by Git. It merges the required CuaBot settings without
replacing existing preferences.

The wrapper gives Xpra a sandbox-specific configuration from
`sandbox/cua/xpra`. Host tray and global window menus are disabled because the
sandbox is controlled through its streamed application windows. This also
avoids GTK's macOS menu integration in Xpra 6.5.1, which can abort while
constructing native submenus on recent macOS releases.

Avoid invoking CuaBot repeatedly through `npx`. Each invocation performs package
resolution and is noticeably slower. `bunx cuabot` can produce an incomplete
Sharp installation on Darwin ARM64 and fail before reaching the server.

## Start an isolated desktop

Run the server in a long-lived terminal or background process:

```sh
just cua-sandbox-start aven-auto-update-a1
```

The first run downloads the desktop image. Verify readiness from another shell:

```sh
scripts/cua-sandbox status aven-auto-update-a1
scripts/cua-sandbox cua aven-auto-update-a1 \
  --screenshot /tmp/cuabot-ready.jpg
```

Useful container checks:

```sh
container="$(scripts/cua-sandbox container aven-auto-update-a1)"
docker ps --filter "name=$container"
docker exec "$container" uname -m
```

## Build aven for the sandbox

The sandbox is ARM64 Linux. Build in a matching Rust container rather than
copying the macOS binary:

```sh
rm -rf /tmp/aven-linux-build
mkdir /tmp/aven-linux-build
git archive HEAD | tar -x -C /tmp/aven-linux-build

docker run --rm \
  -v /tmp/aven-linux-build:/src \
  -w /src \
  rust:1.96-bookworm \
  cargo build
```

`git archive HEAD` builds committed code. Copy modified source files into
`/tmp/aven-linux-build` before building when verifying uncommitted changes.

Copy the result into a user-owned location. `docker cp` creates a file whose
ownership may not be writable by the sandbox user, so copy it once more inside
the container:

```sh
container="$(scripts/cua-sandbox container aven-auto-update-a1)"
docker cp /tmp/aven-linux-build/target/debug/aven \
  "$container:/tmp/aven-built"

docker exec -u user "$container" sh -lc '
  mkdir -p /home/user/aven-run
  cp /tmp/aven-built /home/user/aven-run/aven
  chmod 755 /home/user/aven-run/aven
'
```

## Bootstrap representative data

Populate a session from the marketing screenshot database after deploying the
branch's aven binary and before launching the TUI:

```sh
just cua-sandbox-db aven-auto-update-a1
```

The default source is:

```text
~/.local/state/aven/marketing-screenshot.sqlite
```

The bootstrap command uses SQLite's online backup operation to create a
consistent host snapshot and runs `PRAGMA quick_check`. Migration records that
do not exist in the branch are omitted from the disposable snapshot. The
branch binary must successfully open the result before it is atomically
installed as `/home/user/aven-run/aven.db`. The source database remains
unchanged.

An existing sandbox database is preserved by default. Replace it explicitly
when a fresh snapshot is required:

```sh
scripts/cua-sandbox db-bootstrap aven-auto-update-a1 --force
```

Use another source path when needed:

```sh
scripts/cua-sandbox db-bootstrap aven-auto-update-a1 \
  "$HOME/.local/state/aven/db.sqlite" --force
```

`AVEN_SANDBOX_DB` changes the default source. Database replacement is refused
while an `aven tui` process is running in the session.

## Use the provisioned terminal

The derived image includes Kitty, JetBrains Mono, Powerline symbols, and the
JetBrains Mono Nerd Font. The base image's xterm remains available for
diagnostics, but it does not render aven's colors and glyphs reliably.

Launch aven with isolated database and cache paths:

```sh
container="$(scripts/cua-sandbox container aven-auto-update-a1)"
docker exec -d -u user -e DISPLAY=:100 "$container" \
  kitty \
  --title 'Aven Verification' \
  --override 'font_family=JetBrainsMono Nerd Font Mono' \
  --override font_size=11 \
  --override background=#0b0f14 \
  --override foreground=#d8dee9 \
  --override cursor=#88c0d0 \
  --override selection_background=#434c5e \
  --override remember_window_size=no \
  --override initial_window_width=1200 \
  --override initial_window_height=650 \
  sh -lc '
    cd /home/user/aven-run
    XDG_CACHE_HOME=/home/user/aven-run/cache \
    AVEN_DB=/home/user/aven-run/aven.db \
    AVEN_NO_UPDATE_CHECK=1 \
    ./aven tui
  '
```

`AVEN_NO_UPDATE_CHECK=1` suppresses only the startup check. Explicit `:update`
checks still run.

## Drive and capture the TUI

Inspect screenshots directly with the Read tool. Use the model's image understanding
to evaluate layout, spacing, colors, focus styling, clipping, alignment, visual
hierarchy, and overall appearance. Do not use OCR tools such as Tesseract as a
substitute for direct image inspection. OCR is suitable only as a supplemental
check when exact text extraction is useful.

CuaBot key names follow Playwright naming. Use `Enter` and `Escape`, not uppercase
`ENTER` or `ESC`.

```sh
scripts/cua-sandbox cua aven-auto-update-a1 --type ':update'
scripts/cua-sandbox cua aven-auto-update-a1 --key Enter
scripts/cua-sandbox cua aven-auto-update-a1 \
  --screenshot /tmp/01-checking.jpg

# Confirmation dialogs use y and n.
scripts/cua-sandbox cua aven-auto-update-a1 --type 'y'
scripts/cua-sandbox cua aven-auto-update-a1 \
  --screenshot /tmp/02-progress.jpg

# Cancellation during a cancellable phase.
scripts/cua-sandbox cua aven-auto-update-a1 --key Escape
scripts/cua-sandbox cua aven-auto-update-a1 \
  --screenshot /tmp/03-cancelled.jpg
```

Allow enough time for each asynchronous state before sending the next key. A
key sent while the checking overlay is active does not carry into the later
confirmation dialog.

## Local update fixtures

Keep update tests disposable:

- Place the test executable under `/home/user/aven-run/aven`.
- Use `XDG_CACHE_HOME=/home/user/aven-run/cache`.
- Seed release metadata at
  `/home/user/aven-run/cache/aven/update.json`.
- Serve archives from inside the container on `127.0.0.1`.
- Use version `99.0.0` so it is newer than development builds.
- Use the platform artifact name `aven-linux-arm64.tar.gz`.
- Package exactly one regular root file named `aven`.
- Generate the checksum with the archive filename:
  `sha256sum aven-linux-arm64.tar.gz > aven-linux-arm64.sha256`.

A throttled local server keeps the download overlay visible long enough to
capture and cancel. The release and checksum URLs in the seeded cache can point
to `http://127.0.0.1:<port>/...`. Fetched GitHub release metadata has stricter
URL validation, but cached fixture metadata is suitable for isolated UI tests.

Set an unreachable HTTPS proxy when the explicit live check should fail into the
cached fallback while localhost remains reachable:

```sh
HTTPS_PROXY=http://10.255.255.1:9
NO_PROXY=127.0.0.1
```

## Inspect failures

Capture the application state before changing the fixture:

```sh
scripts/cua-sandbox cua aven-auto-update-a1 \
  --screenshot /tmp/aven-failure.jpg
container="$(scripts/cua-sandbox container aven-auto-update-a1)"
docker exec "$container" sh -lc '
  grep GET /tmp/aven-http.log || true
  /home/user/aven-run/aven --version
'
```

The sandbox exposed a Linux-specific updater failure that macOS did not:
executing an open `NamedTempFile` returned `Text file busy (os error 26)`.
Runtime verification across both operating systems is valuable for installer
changes.

## Cleanup

Stop the named CuaBot server when the desktop is idle:

```sh
just cua-sandbox-stop aven-auto-update-a1
```

The derived image remains available, so the next container has Kitty and fonts
without another apt or font installation. Remove disposable container state when
needed:

```sh
container="$(scripts/cua-sandbox container aven-auto-update-a1)"
docker rm -f "$container" 2>/dev/null || true
```

Remove `aven-cuabot:latest` only when a full image rebuild is desired. OrbStack,
Xpra, Playwright browsers, and npm caches can remain installed for later
verification sessions.
