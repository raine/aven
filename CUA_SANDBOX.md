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

CuaBot uses Playwright on the host. Its loose Playwright dependency can resolve
to a newer version than the package minimum, so inspect the resolved version and
install its matching browser build:

```sh
rm -rf /tmp/cuabot-cli /tmp/cuabot-playwright
npm install --prefix /tmp/cuabot-cli cuabot@1.0.14
PLAYWRIGHT_VERSION="$(
  node -p "require('/tmp/cuabot-cli/node_modules/playwright/package.json').version"
)"
npm install --prefix /tmp/cuabot-playwright "playwright@$PLAYWRIGHT_VERSION"
/tmp/cuabot-playwright/node_modules/.bin/playwright install chromium
```

Configure telemetry explicitly so dependency checks do not launch onboarding:

```sh
mkdir -p ~/.cuabot
cat > ~/.cuabot/settings.json <<'JSON'
{
  "telemetryEnabled": false,
  "aliasIgnored": true
}
JSON
```

Use the installed CLI directly for the rest of the session:

```sh
CUABOT=/tmp/cuabot-cli/node_modules/.bin/cuabot
```

Avoid invoking CuaBot repeatedly through `npx`. Each invocation performs package
resolution and is noticeably slower. `bunx cuabot` can produce an incomplete
Sharp installation on Darwin ARM64 and fail before reaching the server.

## Start an isolated desktop

Run the server in a long-lived terminal or background process:

```sh
"$CUABOT" -n aven-update --serve
```

The first run downloads the desktop image. Verify readiness from another shell:

```sh
"$CUABOT" -n aven-update --status
"$CUABOT" -n aven-update --screenshot /tmp/cuabot-ready.jpg
```

Useful container checks:

```sh
docker ps --filter name=cuabot-xpra-aven-update
docker exec cuabot-xpra-aven-update uname -m
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
docker cp /tmp/aven-linux-build/target/debug/aven \
  cuabot-xpra-aven-update:/tmp/aven-built

docker exec -u user cuabot-xpra-aven-update sh -lc '
  mkdir -p /home/user/aven-run
  cp /tmp/aven-built /home/user/aven-run/aven
  chmod 755 /home/user/aven-run/aven
'
```

## Use a readable terminal

The image includes xterm, but its default font and color support make the aven
TUI difficult to inspect. Install Kitty and fonts inside the disposable
container:

```sh
docker exec -u 0 cuabot-xpra-aven-update sh -lc '
  mkdir -p /var/lib/apt/lists/partial
  apt-get update
  DEBIAN_FRONTEND=noninteractive apt-get install -y \
    kitty fonts-jetbrains-mono fonts-powerline
'

docker exec -u user cuabot-xpra-aven-update sh -lc '
  mkdir -p /home/user/.local/share/fonts/JetBrainsMonoNerd
  curl -fsSL \
    https://github.com/ryanoasis/nerd-fonts/releases/latest/download/JetBrainsMono.tar.xz \
    | tar -xJ -C /home/user/.local/share/fonts/JetBrainsMonoNerd
  fc-cache -f /home/user/.local/share/fonts
'
```

Launch aven with isolated database and cache paths:

```sh
docker exec -d -u user -e DISPLAY=:100 cuabot-xpra-aven-update \
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

CuaBot key names follow Playwright naming. Use `Enter` and `Escape`, not uppercase
`ENTER` or `ESC`.

```sh
"$CUABOT" -n aven-update --type ':update'
"$CUABOT" -n aven-update --key Enter
"$CUABOT" -n aven-update --screenshot /tmp/01-checking.jpg

# Confirmation dialogs use y and n.
"$CUABOT" -n aven-update --type 'y'
"$CUABOT" -n aven-update --screenshot /tmp/02-progress.jpg

# Cancellation during a cancellable phase.
"$CUABOT" -n aven-update --key Escape
"$CUABOT" -n aven-update --screenshot /tmp/03-cancelled.jpg
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
"$CUABOT" -n aven-update --screenshot /tmp/aven-failure.jpg
docker exec cuabot-xpra-aven-update sh -lc '
  grep GET /tmp/aven-http.log || true
  /home/user/aven-run/aven --version
'
```

The sandbox exposed a Linux-specific updater failure that macOS did not:
executing an open `NamedTempFile` returned `Text file busy (os error 26)`.
Runtime verification across both operating systems is valuable for installer
changes.

## Cleanup

Stop the named CuaBot server and remove its container when the VM should not
remain available for inspection:

```sh
"$CUABOT" -n aven-update --stop
docker rm -f cuabot-xpra-aven-update 2>/dev/null || true
```

OrbStack and Xpra are host dependencies and can remain installed for later
verification sessions.
