# aven-desktop

The GPUI desktop app. This macOS-only library is linked into `aven` by the
`desktop` feature, and `aven` starts it when Launch Services opens the bundle.
On other platforms the crate is empty.

Run its tests from the repository root:

```sh
cargo test -p aven-desktop
```
