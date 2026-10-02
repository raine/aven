# macOS release qualification

A successful build or `--help` smoke test is not proof that a release can read
an existing installation's Keychain credentials. Use disposable databases and
accounts for qualification. Never lock the maintainer's login Keychain, change
its search list, print credentials, or copy protected state into evidence logs.

## Qualification without releasing E2EE

`.github/workflows/macos-qualification.yml` runs on pushes to the exact
`e2ee-macos-release` branch. It builds the pinned E2EE baseline
`e3c2bcb0891340479f9a5a674a60665eccfeec87` and the pushed candidate, signs both
macOS architectures using the release signing script, and uploads archives as
Actions artifacts for 14 days. Its continuity jobs require different binaries
with equal, mutually satisfied designated requirements. All jobs have read-only
repository permissions; there are no GitHub release or tap publishing steps.
Do not use `v*` tags for this workflow: they trigger the public release workflow.

To run it, temporarily permit the exact qualification **branch** in the
`macos-signing` environment alongside the existing `v*` **tag** policy. Do not
permit arbitrary branches or remove the tag restriction. Push the reviewed
qualification branch with hooks enabled, record its commit and run URL, then
remove the temporary branch permission after the signing jobs finish.

Download a specific artifact with:

```sh
gh run download RUN_ID --name qualification-baseline-aven-darwin-arm64 --dir baseline
gh run download RUN_ID --name qualification-candidate-aven-darwin-arm64 --dir candidate
```

Each directory contains the archive, archive checksum, `binary.sha256`,
`signature.txt` and `commit-sha`. Check the provenance against the requested
revisions and independently verify the extracted bytes/signature. Use `amd64`
in artifact names and `x86_64` in verifier arguments for Intel.

Use these exact signed bytes for isolated CLI/daemon and local test-tap upgrades.
Never replace the user's installed executable or production tap for qualification.
Both revisions can report the same version; these tests prove changed-code
credential continuity, not latest-release selection. The unchanged installer
and `aven update` cannot consume Actions artifacts directly. Testing a local
copy/rename or a local tap therefore leaves their production download/discovery
paths open, as does bypassing version selection in an updater-internals test.

## Signing and packaged artifacts

- The `macos-signing` GitHub environment must restrict deployments to `v*` tags
  and provide `APPLE_DEVELOPER_ID_CERT_BASE64`,
  `APPLE_DEVELOPER_ID_CERT_PASSWORD`, and `APPLE_DEVELOPER_ID_IDENTITY`.
- Retain Developer ID Application team `YG824X87Y2` and identifier
  `fi.zendit.Aven` across releases and certificate renewal. The certificate
  currently configured expires **2027-02-01**; renew before expiry under the
  same team. Do not treat an ad-hoc or unsigned build as a signed-upgrade baseline.
- Record the tag, commit, GitHub run URL and conclusions for `signing-preflight`,
  both `sign-macos` matrix jobs, `release`, and `update-tap`. Locally signing a
  binary does not qualify secret import, cleanup, or packaging on a CI runner.
- Download the **published** archive and checksum for each architecture. Verify
  the checksum, extract it, run `scripts/verify-macos-release PATH arm64` or
  `scripts/verify-macos-release PATH x86_64`, and run `--version` without a TTY.
  Retain signature metadata and archive/binary SHA-256 values, not secret values.
- The signing job checks the extracted binary's byte identity against its signed
  input, architecture, strict signature and fixed Developer ID requirement.
  Its signature fixtures reject unsigned, ad-hoc, tampered and wrong-architecture
  binaries. Repeat verification on the published downloads to cover publication.
- Compare designated requirements for two independently built signed releases.
  Check each binary against the other's designated requirement with
  `codesign --verify --strict -R="REQUIREMENT" PATH`. Certificate renewal must
  preserve access, not pin a certificate serial, hash, or release cdhash.

## Upgrade and protected-credential matrix

Start with an isolated database/vault on signed release A. Add a task and sync
it. Record the database installation ID and exact Keychain service/account
attributes (never the wrapping key). Keep the database and protected-state
paths fixed when installing release B. Preserve the signed bytes throughout.

For **each** installation method below, independently establish an A baseline,
upgrade to B, and record versions, installed binary hashes, signature metadata,
daemon restarts, and bounded command results:

| Method | Required observation |
| --- | --- |
| Homebrew | Real published formula upgrade; installed bytes match the published archive, launchd follows the new keg, daemon restarts and sync resumes. |
| `scripts/install` | Run with an isolated `AVEN_INSTALL_DIR` and explicit `AVEN_VERSION`; verify copy/rename preserves the signature and existing credential access. |
| `aven update --yes` | Direct installation, not a Cargo/Homebrew checkout; exercise the actual download, checksum, extraction, staged version check and rename path. Verify installed bytes and daemon restart. A manual rename does not qualify this path. |

After every upgrade, verify:

1. Fresh attended CLI sync, non-TTY sync with stdin detached, read-only status,
   and launchd daemon reads succeed silently, with no protected-storage error.
2. Existing local and remote tasks still agree. New local edits still sync.
3. There is still exactly one non-synchronizing wrapping-key Keychain item per
   database; no replacement authority or extra installation identity appeared.
4. On a **disposable** installation, delete only its exact wrapping-key item.
   A fresh process must pause sync with `protected-key-storage-missing`, keep
   ciphertext and local tasks, permit local edits, and not regenerate the key.
   Use the documented recovery/re-pair flow rather than silently resetting it.

Both architectures need published-artifact checks. Running Intel under Rosetta
is execution evidence, not native Intel lifecycle qualification.

## Locked login Keychain and launchd lifecycle

Use a disposable macOS account or machine. The backend queries the login
Keychain; a separate test Keychain does not qualify this behavior.

- Stop existing Aven processes before locking the disposable login Keychain:
  running storage instances cache the wrapping key, so a warm process is not a
  reliable locked-Keychain probe.
- With that Keychain locked, run a **fresh non-TTY** sync, read-only status, and
  a fresh daemon. Use a bounded timeout; unavailable storage must fail promptly
  without SecurityAgent UI. Local task reads/edits must remain usable.
- Unlock normally, start fresh processes, and verify sync resumes with the same
  installation identity and wrapping-key item, without replacement authority.
- Reboot and observe behavior before and after first GUI login. A user launch
  agent may not run before login; record whether it actually launched and in
  which session. Do not label an unstarted agent a passing credential test.
  Distinguish GUI launchd, SSH/non-GUI login, and a deliberately configured
  pre-GUI service. Test any claimed supported unattended context explicitly.
- Uninstalling an executable or a Homebrew keg must not be assumed to delete
  Keychain credentials. A new database installation gets a distinct identity;
  reinstalling the executable alone is not a new database installation.

## Boundaries and open checks

The direct updater currently verifies the archive checksum and staged version,
not the Developer ID signer. The installer also does not enforce the signer.
Release-package validation is not client-side signer enforcement. Notarization
and quarantine/Gatekeeper behavior require separate qualification; the installer
removes quarantine, so its success is not a Gatekeeper assessment.

Source and CI-install builds can be ad-hoc signed. A changed build may require
one wrapping-key authorization per database. Use an attended sync command and
choose **Always Allow** when authorizing that build. Daemon/non-TTY and
read-only status operations suppress prompts and cannot authorize it for you.

Keep unexecuted CI, published upgrade, locked-login and pre-GUI checks open.
Desktop evidence does not complete iOS real-device lock, before-first-unlock,
restore, missing-key, or other iOS-only acceptance criteria. Record dated
execution evidence separately from this checklist and leave the combined task
open until all required platform criteria are satisfied.
