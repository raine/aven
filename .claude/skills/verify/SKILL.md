---
name: verify
description: Exercise aven CLI changes through the built binary
---

1. Build the CLI with `cargo build --bin aven`.
2. Create an isolated database with `tmpdir=$(mktemp -d)` and pass
   `--db "$tmpdir/verify.sqlite"` to every command.
3. Seed the smallest state needed through `target/debug/aven`, then invoke the
   changed command through the same binary and capture its stdout and exit code.
4. Probe an adjacent boundary or error case through the CLI.
5. Remove the temporary directory after capturing evidence.

Do not run tests or type checking as runtime verification.
