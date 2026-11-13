# Releases

`just release` bumps the binary version and creates the GitHub release tag using
`cargo-release --skip-publish`. Additional options, such as `--dry-run` or
`--continue`, are forwarded to the helper. The tag triggers
`.github/workflows/release.yml` to build release binaries and update Homebrew,
Nix, and container distribution. The `aven` package has `publish = false`; its
crates.io name is retained, with old versions yanked rather than deleted.

Library crates remain publishable independently of the binary. For each release:

1. Review library changes and version bumps, including dependency version pins.
   The release helper bumps changed published workspace dependencies; review its
   diff before confirming the release.
2. Publish changed library versions in dependency order: `aven-protocol`,
   `aven-ai-contract`, `aven-core`, then `aven-host`. Skip versions already
   published. Use `cargo publish --locked -p <package>` for each library.
3. Verify the GitHub release workflow and the library versions on crates.io.
   To resume the binary release, use `just release --continue`; retry any failed
   library publication separately.

`aven-mobile` consumes `aven-core` (and transitively `aven-protocol`) from pinned
Git revisions. `aven-cloud` also consumes `aven-protocol` and `aven-ai-contract`
from Git. Keep library versions and APIs compatible with their consumers.
