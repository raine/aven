# aven-desktop

The GPUI desktop app. This macOS-only library is linked into `aven` by the
`desktop` feature, and `aven` starts it when Launch Services opens the bundle.
On other platforms the crate is empty.

Run its tests from the repository root:

```sh
cargo test -p aven-desktop
```

Install a release build from this checkout to `/Applications`:

```sh
just install-app
```

This also relinks `~/.cargo/bin/aven` to the new bundle, reinstalls the daemon
when sync is enabled, and removes `~/.local/share/aven/Aven.app`. Quit a running
app first; it keeps its old executable until restarted.
