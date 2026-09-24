# Sync protocol maintenance

Read this before changing synchronized operations, their interpretation, or the
protocol constants. [ARCHITECTURE.md](ARCHITECTURE.md) maps the implementation
owners. This document describes the compatibility contract and the evidence
required to change it.

Scope: this workspace ships only end-to-end encrypted sync. Its encrypted tail
validates retained and new operations against replica protocol 18 through the
operation contracts and persisted replica behavior below, so those sections
govern every shared operation change. The unencrypted request and response
envelope survives only behind `aven-core`'s `test-support` feature as an
in-process page simulator for fixtures; discovery, server admission and
release protocol markers no longer exist. Changing an
encrypted operation contract also requires an encrypted tail codec change and
its own security review.

## What a protocol version means

A protocol version identifies a shared-data contract: which operations and values
participants understand and what those operations mean. A new operation, task
status, payload meaning, or conflict-resolution rule can require a new version.
UI changes, local-only settings, and performance changes that preserve the
contract do not ordinarily require one. A SQLite schema migration is not itself
a sync protocol change; database compatibility is a separate concern.

The user-facing mental model is:

> Updating an app keeps it working with your existing server. Some new shared
> features require updating the server first. Updating the server may require
> updating your other apps.

Normally hide protocol numbers in user-facing guidance. Name the app or server
that needs updating and confirm that local work remains saved. Feature-gating
messages should distinguish an unavailable new feature from blocked ordinary
sync: existing tasks and edits continue working under the established contract.

Compatibility is asymmetric:

- A server accepts requests for exactly one active protocol.
- A client genuinely supports every protocol from its maintained baseline through
  its latest supported protocol, inclusively.
- A newer client connected to an older supported server continues using that
  server's contract. Shared features requiring a newer contract are blocked, not
  silently omitted or translated.
- An older client facing an unsupported newer server pauses sync with app-update
  guidance. Pending local work stays saved.

A newer server must still interpret retained older operations with their original
meaning. Requiring a newer request envelope does not give permission to reinterpret
or rewrite the operations inside it.

Genuine fallback includes operation creation, validation, interpretation, merge
semantics, recurrence, attachments, retries, and pending changes. Keep shared
implementation where semantics are unchanged rather than copying the application
for each protocol. A semantic change with unchanged serialized bytes can still
break compatibility; a version check cannot detect it. Review interpretation code
and retain historical replay and mixed-version evidence.

## How it is encoded

### Protocol constants

`crates/aven-core/src/sync/wire.rs` declares `SYNC_PROTOCOL_VERSION`, the active
server protocol and upper bound of production client support.

`crates/aven-core/src/sync/protocol.rs` declares
`MAINTAINED_PROTOCOL_BASELINE`, the oldest protocol the client promises to retain.
Both are currently 18. Protocols 19 and 20 in compatibility tests are internal
test contracts, not advertised production implementations.

For example, a future baseline of 18 and latest version of 20 would promise real
support for 18, 19, and 20. Changing the constants alone cannot fulfill that
promise.

### Request and response envelopes

`SyncRequest.protocol_version` carries the selected protocol as a JSON integer.
It is optional in the Rust decoding type so missing versions can be rejected;
absence does not negotiate a default. `SyncResponse.protocol_version` identifies
the response contract. Server admission checks exact equality with the active
server protocol on every metadata request.

Across acknowledgements, pulled changes, and retained local history, one server
sequence belongs to only one operation identity. An unexpected collision must
reject the page transactionally, not silently omit a history row.

A session discovers compatibility with an authenticated empty `/sync` request
using a maximal cursor, before ordinary metadata or attachment transfer. A
supported version learned from a mismatch requires an exact confirmation probe.
Discovery is advisory: its cursor is not applied and it does not establish local
replica behavior. A server cutover after discovery must still reject a stale
metadata request without accepting any of its changes.

### Operation contracts

`ChangeWire` contains an operation name (`op_type`), optional field, JSON payload,
identity, ordering, and version metadata. It has no per-operation protocol-number
field. An operation's meaning comes from its immutable name and payload contract.

`sync/protocol.rs` registers operations and relevant closed values. The central
creation gate compares their requirements with the replica's established
protocol. An unknown operation fails closed. New values in existing operations
must also be gated; registering only new operation names is insufficient.

A future operation registration might look like this schematic example:

```rust
// Illustrative registration, not a production operation.
match op {
    "new_shared_operation" => Ok(19),
    op if BASELINE_OPERATIONS.contains(&op) => Ok(18),
    _ => bail!("unregistered operation"),
}
```

That registration must accompany producer, payload-validation, and apply logic.
It is not a substitute for them. A changed meaning requires a new operation
identity or an explicit revision with retained historical decoding. Existing
pending rows must keep their IDs, payloads, timestamps, order, and canonical
meaning when uploaded through a newer request envelope.

#### Recurrence generation forms

Generated `create_task` and `project_recurrence_occurrence` records have two
derivation forms under the same operation names and payload keys. Validators accept
either form when all of a record's change IDs and seed belong to it, and reject
mixtures.

- Occurrence form: change IDs and the task field-version seed derive from
  workspace, series and slot date only. Local-only databases produce it, as did
  earlier encrypted-sync builds. Two such records with the same ID and unequal
  content remain a same-ID integrity failure and are never rewritten.
- Proposal form: databases with a seed opt-in or peer enrollment produce it. The
  create's change ID derives from a SHA-256 digest over a JSON array of its
  workspace, series, slot, title, description, project, initial status, priority,
  labels (strictly ascending), metadata (`[field_id, key, value]`, strictly
  ascending by key) and editable schedule values. The projection change ID and
  field-version seed derive from that create ID. A projection carries no template
  content, so apply binds it to the referenced create's coordinates, seed and
  schedule context.

A task can therefore have several accepted generations. The first in accepted
order supplies untouched defaults; explicit edits based on another generation's
seed follow ordinary conflict rules. Projection and outcome apply compare only the
series lattice and timezone with the current series, because available time and
due policy are the author's historical context.

### Persisted replica behavior

The SQLite `meta` key `sync_established_protocol` stores the established protocol
as a decimal string. The accepted metadata-page transaction establishes it,
together with application, acknowledgements, and cursor movement. Prepared pages
also capture the previous local behavior so stale local contexts reject
transactionally.

Both ordinary and explicit-identity change insertion helpers in `db.rs` use the
central gate inside the owning transaction, before sequence allocation. An app
update, offline restart, missing sync configuration, or disabled sync must not
silently promote shared-operation creation to a newer protocol.

A missing established protocol defaults to the maintained baseline. This policy
also applies to permanently local-only databases. Local-only interface and query
features remain unrestricted, but newer shared features require an established
compatible server relationship. There is no standalone latest-protocol toggle.

JSON import clears the server relationship and compatibility metadata and
validates restored history and materialized closed values against the baseline.
SQLite backups preserve the relationship. Importing newer-only shared data needs
an explicit policy decision; do not silently relax baseline import validation.

## Adding a protocol version

1. **Identify the contract change.** State why the older contract cannot safely
   represent or interpret the feature. Distinguish a new shared contract from a
   local implementation change. Retain the maintained baseline unless retirement
   is separately authorized.
2. **Implement cumulative semantics.** Add the new operation or revision and any
   version-dependent fields or closed values in `sync/protocol.rs`. Preserve the
   literal older registrations and their interpretation. Do not add newer values
   to the frozen baseline lists or delegate those lists to extensible domain
   enums.
3. **Connect every owner.** Align producers, the transactional creation gate,
   wire validation, remote apply, restored history, outgoing and incoming pages,
   and authoritative server persistence admission. Include undo, recurrence,
   automated mutations, and consumer APIs where affected. Full historical wire
   validation remains separate from the local creation gate; stricter creation
   validation must not invalidate retained history accidentally.
4. **Resolve persistence and import behavior.** Prove existing replicas retain
   their mode offline and pending operations survive the cutover unchanged.
   Decide how any new shared data interacts with baseline-only JSON import and
   local-only databases before shipping it. Do not invent implicit promotion,
   down-conversion, or selective upload.
5. **Advance the active version.** Keep typed errors and status behavior aligned
   with the new meaning, and update the encrypted tail's accepted operation set
   in `encrypted_tail/domain.rs` deliberately.
6. **Prove the actual feature in both modes.** Use the checks below, including a
   real released-server process. Test-only future operation names demonstrate the
   mechanism, not compatibility of a newly implemented production feature.
7. **Coordinate publication.** Before publishing the protocol-changing server,
   compatible readers must be publicly available on every supported platform.
   Warn operators that the server cutover can pause older clients' sync while
   retaining their local work. External mobile-store availability must be checked
   separately from this workspace's builds.

The baseline vocabulary test currently compares all production operation names
and domain values with the frozen protocol-18 contract. At the first extension,
separate baseline-contract assertions from completeness checks for newer
registrations. Do not make the test pass by declaring the new vocabulary part of
protocol 18 or by removing registration coverage.

## Required compatibility evidence

For a protocol addition, retain evidence for:

- A newer client using the unchanged released baseline server, including ordinary
  edits and attachment transfer.
- An existing replica reopened by the newer client offline, followed by reconnect.
- Real newer-only operations and values rejected atomically in baseline mode,
  without domain, change-log, sequence, field-version, or undo residue.
- No attachment inventory or transfer before compatibility is confirmed.
- A server cutover between discovery and metadata admission rejecting the stale
  request without acknowledgements, cursor movement, or partial acceptance.
- Successful rediscovery and acceptance on the newer server without rewriting
  pending older operations; retries preserve canonical identity and meaning.
- Unsupported clients retaining pending work and receiving actionable guidance.
- Unknown operations rejected at both creation and server admission, even when
  submitted under a supported protocol number.
- Unknown or unsupported incoming data rejected without advancing the cursor past
  it; acknowledgements and local application remain transactional.
- Historical replay preserving materialized state, conflict behavior, and
  deterministic identities across the supported contracts.
- Import, standalone behavior, and host surfaces affected by the new contract.

Start with `cargo test -p aven-core --lib sync::`, then choose focused CLI sync,
conflict, recurrence, encrypted tail, and consumer tests for the affected paths.
Follow the validation guidance in `ARCHITECTURE.md`; automated tests do not replace
an unchanged released-server interoperability exercise.

`crates/aven-core/tests/fixtures/sync-protocol-18.json` is a frozen historical
oracle from unchanged `v0.1.40`, revision
`7c1f7ca415fc53468f7457bbb64bc7c8c7b42cd0`. It contains 33 synthetic changes and
expected state across 20 tables. The replay test excludes receiver-local task
activity timestamps from convergence comparisons. Attachment metadata is present;
blob bytes are tested through transfer exercises. Do not regenerate the expected
state from the implementation under test to make a regression pass. Add fixtures
for uncovered contracts without erasing the older oracle.

## Retiring a maintained baseline

**Do not bump `MAINTAINED_PROTOCOL_BASELINE` alongside the active protocol.**
There is no automatic rolling support window. Leave it at 18 unless explicitly
authorized to design and implement retirement of protocol-18 support.

Retirement is a product compatibility decision, potentially justified by an
unsustainable maintenance burden or a security constraint. It requires an
announced support change, a supported upgrade path, and explicit treatment of:

- Older servers and clients across every supported platform.
- Existing replicas persisted at the retired protocol and their offline pending
  work.
- Local-only databases, older exports, and backups.
- Release metadata, user guidance, and validation of transition behavior.

There is no implemented baseline-retirement migration. Database open validates
persisted replica behavior, so simply raising the constant can reject existing
databases at open rather than merely pausing their sync. Do not treat a constant
edit as a safe retirement procedure.

## Limits of this contract

Protocol support does not guarantee that an older binary can open a newer SQLite
schema. It also does not detect restored or replaced server history at the same
URL. Dataset epochs, fleet tracking, selective synchronization, capability
negotiation, and automatic operation down-conversion are not part of this model.
Do not skip incompatible pending operations to upload later ones, introduce
client-specific task representations, or add historical compaction as part of a
protocol bump. User-managed transitions and automatic first-feature-use cutovers
are also outside this contract.

A dataset epoch would address a different product requirement: a server update
must keep old clients syncing until incompatible shared data is actually
committed. Supporting that requirement would mean serving multiple session
protocols against one dataset, with a persisted dataset compatibility floor and
an atomic transition when newer data is accepted. It is not required here:
deliberate server upgrades are the accepted cutover point. Revisit that design
only if the product requirement changes.
