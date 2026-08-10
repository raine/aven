---
title: Sync scaling measurements
description: Measured synthetic sync scaling results, thresholds, and the resulting protocol recommendation.
---

This report measures protocol 13 replay with generated data. It uses no user data.
The benchmark builds canonical operations through Aven core APIs, synchronizes
through a real loopback server, and checks replica state before reporting a run.

## Recommendation

Implement snapshot bootstrap as the next sync protocol architecture change, then
design retention around snapshots. Keep the current replay path for incremental
sync and compatibility. Keep current own-change filtering unless a snapshot
protocol provides a compatible opportunity to suppress acknowledged operations
from response bodies.

Fresh replay applied 25,609 operations in 7.650 seconds. That exceeds the
5-second apply-time budget even though total wall time, metadata bytes, and
operation-log growth remain within their budgets. Snapshot bootstrap addresses
the measured bottleneck directly. Retention by itself cannot help fresh replicas,
and retention is unsafe until a snapshot covers the history a fresh or lagging
client would otherwise replay.

## Reproduce the measurement

Run from the repository root:

```sh
cargo bench --bench sync_scaling -- \
  --profile report \
  --output /tmp/aven-sync-scaling-report.json
```

The `smoke` profile uses 32 tasks and checks the same scenario quickly:

```sh
cargo bench --bench sync_scaling -- \
  --profile smoke \
  --output /tmp/aven-sync-scaling-smoke.json
```

The report profile is fixed at:

| Synthetic component | Count |
| --- | ---: |
| Ordinary tasks | 5,000 |
| Description update rounds over every task | 4 |
| Recurrence series | 64 |
| Resolved occurrences per series | 1 |
| Distinct 256 by 256 PNG attachments | 32 |
| Divergent title conflicts | 64 |
| Accepted operations after both devices converge | 25,609 |

Task text, recurrence schedules, and image pixels follow fixed formulas in
`benches/sync_scaling.rs`. IDs and mutation timestamps remain intentionally
volatile. Reproducibility means identical workload composition, ordering,
measure definitions, and assertions rather than byte-identical databases.

## Reference environment

| Item | Value |
| --- | --- |
| Commit | `06f1b01739ff55eba9c623b700fb34645a839c14` |
| CPU | Apple M2 Max |
| Memory | 34,359,738,368 bytes |
| macOS | 26.5.2 |
| Rust | `rustc 1.96.0 (ac68faa20 2026-05-25)` |
| Transport | Loopback HTTP, release benchmark build |

## Results

### Sync stages

| Stage | Duration | Sessions | Pages | Pushed | Pulled | Request wire bytes | Decoded response bytes | Apply time | Attachment bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Initial push and acknowledgements | 3.750 s | 2 | 101 | 25,481 | 0 | 1,141,044 | 12,183,451 | 1.027 s | 2,370,244 uploaded |
| Peer bootstrap | 7.860 s | 2 | 51 | 0 | 25,481 | 4,974 | 10,865,520 | 7.146 s | 2,370,244 downloaded |
| Steady-state push and acknowledgements | 11 ms | 1 | 1 | 64 | 0 | 3,223 | 26,769 | 2 ms | 0 |
| Conflicting peer sync | 18 ms | 1 | 1 | 64 | 64 | 3,236 | 49,800 | 9 ms | 0 |
| Source convergence | 9 ms | 1 | 1 | 0 | 64 | 98 | 23,113 | 7 ms | 0 |
| Fresh replica replay | 8.358 s | 2 | 51 | 0 | 25,609 | 4,974 | 10,911,909 | 7.650 s | 2,370,244 downloaded |

The attachment count exceeds the 16-object session transfer budget, so both
bootstrap runs require two sessions. The 25,609-operation history exceeds both
metadata page limits and exercises bounded push and pull loops.

### Correctness checks

The source, peer, and fresh replica each contained 64 open title conflicts. The
fresh replica contained 5,128 tasks, 64 recurrence series, 32 live attachments,
and all 2,370,244 referenced blob bytes. Source, server, and fresh databases each
contained the same 25,609 operations with this composition:

| Operation | Rows |
| --- | ---: |
| `attachment_add` | 32 |
| `create_project` | 1 |
| `create_recurrence_series` | 64 |
| `create_task` | 5,128 |
| `project_recurrence_occurrence` | 128 |
| `resolve_recurrence_occurrence` | 64 |
| `set_field` | 20,192 |

### Database growth

All databases were checkpointed before baseline and final size reads.
Shared-memory sidecars are excluded.

| Database | Baseline | Final | Growth |
| --- | ---: | ---: | ---: |
| Source | 536,576 bytes | 21,725,184 bytes | 21,188,608 bytes |
| Server | 536,576 bytes | 9,011,200 bytes | 8,474,624 bytes |
| Fresh replica | 536,576 bytes | 21,102,592 bytes | 20,566,016 bytes |

Server growth was 330.9 bytes per accepted change. Linear projection at 250,000
changes is 83,267,491 bytes, including the measured baseline. SQLite page
allocation and indexes make this projection directional rather than exact.
Payload text totaled 3,916,403 bytes, but payload bytes are not a physical
operation-log size measurement.

## Resource thresholds

These budgets are decision thresholds, not extrapolations selected after the
run.

| Dimension | Budget | Measured | Result |
| --- | ---: | ---: | --- |
| Fresh replay wall time | 10.000 s | 8.358 s | pass |
| Fresh decoded metadata responses | 64 MiB | 10,911,909 bytes | pass |
| Fresh apply time | 5.000 s | 7.650 s | **fail** |
| Own-change pull ratio | 0 | 0 for both push stages | pass |
| Push acknowledgement response bytes | 1 KiB per pushed change | 478.1 initial, 418.3 steady state | pass |
| Server growth | 4 KiB per accepted change | 330.9 bytes | pass |
| Projected server database at 250,000 changes | 1 GiB | 83,267,491 bytes | pass |

Protocol 13 filters acknowledged own changes from local apply work, which is why
both push stages report zero pulled operations. Response bodies still carry
per-change acknowledgement and replay data, so the benchmark records response
bytes separately. At the measured rate, that echo is not the primary scaling
constraint. Apply cost during fresh replay is.

## Snapshot protocol requirements

A snapshot design must preserve these properties before retention can depend on
it:

- **Cursor validation:** Bind each snapshot to an exact `server_seq`. Validate
  that replay starts at that cursor and retains strictly increasing sequence
  checks for every subsequent page.
- **Interrupted push acknowledgements:** Preserve byte-identical outstanding
  push replay and duplicate change ID acknowledgement. Snapshot selection must
  not replace or obscure an unacknowledged prepared request.
- **Compatibility:** Keep protocol 13 replay available while newer clients
  discover or negotiate snapshot support. Servers must not delete history still
  required by supported replay-only clients.
- **Fresh-database restriction:** Install a snapshot only into an empty replica
  after normal server pinning and workspace validation. Existing replicas must
  continue from their validated cursor rather than replacing local state.
- **Recurrence state:** Include series, schedules, template fields, sparse
  occurrences and outcomes, pause intervals, lifecycle state, projected tasks,
  deterministic identities, field versions, and unresolved conflicts.
- **Attachments:** Include complete attachment metadata and a content-addressed
  object manifest. Keep bounded missing-object probes and downloads so snapshot
  metadata does not require one unbounded blob transfer.
- **Log retention:** Retain changes after the snapshot cursor and every change
  needed by a supported active client. Delete earlier operations only after a
  retained snapshot and compatibility policy make fresh and lagging replay safe.

This proposal adds snapshot bootstrap before retention. It does not require
changing incremental replay, conflict semantics, recurrence derivation, or blob
transfer limits.

## Measurement limitations

- The run measures one release build on one local host. Network round trips,
  slower storage, thermal state, and concurrent clients can change wall time.
- `request_wire_bytes` counts encoded metadata request bodies.
  `response_decoded_bytes` counts decoded metadata response bodies, so it is an
  upper bound for compressed response wire cost.
- These metadata counters exclude HTTP headers, framing, missing-blob probe
  bodies, and blob bodies. Attachment transfer bytes are reported separately.
- The benchmark resolves one occurrence per recurrence series because the public
  mutation path reconciles against the runtime clock. It still measures complete
  aggregate creation, projection, outcome, successor, conflict, and replay state.
- Linear database projection does not model SQLite page transitions, changed
  operation composition, or future indexes.

## Raw report

```json
{
  "schema_version": 1,
  "profile": {
    "name": "report",
    "ordinary_tasks": 5000,
    "scalar_update_rounds": 4,
    "recurrence_series": 64,
    "outcomes_per_series": 1,
    "attachments": 32,
    "image_width": 256,
    "image_height": 256,
    "conflict_tasks": 64
  },
  "fixed_date": "2026-01-01",
  "metric_notes": {
    "blob_bytes": "Content bytes uploaded or downloaded through attachment transfer requests.",
    "request_wire_bytes": "Encoded metadata request bodies only; excludes HTTP framing, blob probes, and blob bodies.",
    "response_decoded_bytes": "Decoded metadata response bodies only; excludes HTTP framing, blob probes, and blob bodies."
  },
  "stages": {
    "initial_push_ack": {
      "duration_ms": 3750,
      "sessions": 2,
      "complete": true,
      "pages": 101,
      "pushed": 25481,
      "pulled": 0,
      "request_bytes": 10856782,
      "request_wire_bytes": 1141044,
      "response_decoded_bytes": 12183451,
      "apply_ms": 1027,
      "blob_uploaded": 32,
      "blob_uploaded_bytes": 2370244,
      "blob_downloaded": 0,
      "blob_downloaded_bytes": 0
    },
    "peer_bootstrap": {
      "duration_ms": 7860,
      "sessions": 2,
      "complete": true,
      "pages": 51,
      "pushed": 0,
      "pulled": 25481,
      "request_bytes": 4974,
      "request_wire_bytes": 4974,
      "response_decoded_bytes": 10865520,
      "apply_ms": 7146,
      "blob_uploaded": 0,
      "blob_uploaded_bytes": 0,
      "blob_downloaded": 32,
      "blob_downloaded_bytes": 2370244
    },
    "steady_state_push_ack": {
      "duration_ms": 11,
      "sessions": 1,
      "complete": true,
      "pages": 1,
      "pushed": 64,
      "pulled": 0,
      "request_bytes": 23393,
      "request_wire_bytes": 3223,
      "response_decoded_bytes": 26769,
      "apply_ms": 2,
      "blob_uploaded": 0,
      "blob_uploaded_bytes": 0,
      "blob_downloaded": 0,
      "blob_downloaded_bytes": 0
    },
    "conflicting_device_sync": {
      "duration_ms": 18,
      "sessions": 1,
      "complete": true,
      "pages": 1,
      "pushed": 64,
      "pulled": 64,
      "request_bytes": 23064,
      "request_wire_bytes": 3236,
      "response_decoded_bytes": 49800,
      "apply_ms": 9,
      "blob_uploaded": 0,
      "blob_uploaded_bytes": 0,
      "blob_downloaded": 0,
      "blob_downloaded_bytes": 0
    },
    "source_convergence": {
      "duration_ms": 9,
      "sessions": 1,
      "complete": true,
      "pages": 1,
      "pushed": 0,
      "pulled": 64,
      "request_bytes": 98,
      "request_wire_bytes": 98,
      "response_decoded_bytes": 23113,
      "apply_ms": 7,
      "blob_uploaded": 0,
      "blob_uploaded_bytes": 0,
      "blob_downloaded": 0,
      "blob_downloaded_bytes": 0
    },
    "fresh_replica_replay": {
      "duration_ms": 8358,
      "sessions": 2,
      "complete": true,
      "pages": 51,
      "pushed": 0,
      "pulled": 25609,
      "request_bytes": 4974,
      "request_wire_bytes": 4974,
      "response_decoded_bytes": 10911909,
      "apply_ms": 7650,
      "blob_uploaded": 0,
      "blob_uploaded_bytes": 0,
      "blob_downloaded": 32,
      "blob_downloaded_bytes": 2370244
    }
  },
  "database_growth": {
    "source": {
      "baseline_main_bytes": 536576,
      "baseline_wal_bytes": 0,
      "final_main_bytes": 21725184,
      "final_wal_bytes": 0,
      "growth_bytes": 21188608
    },
    "server": {
      "baseline_main_bytes": 536576,
      "baseline_wal_bytes": 0,
      "final_main_bytes": 9011200,
      "final_wal_bytes": 0,
      "growth_bytes": 8474624
    },
    "fresh": {
      "baseline_main_bytes": 536576,
      "baseline_wal_bytes": 0,
      "final_main_bytes": 21102592,
      "final_wal_bytes": 0,
      "growth_bytes": 20566016
    }
  },
  "operation_logs": {
    "source": {
      "change_rows": 25609,
      "payload_bytes": 3916403,
      "by_operation": {
        "attachment_add": 32,
        "create_project": 1,
        "create_recurrence_series": 64,
        "create_task": 5128,
        "project_recurrence_occurrence": 128,
        "resolve_recurrence_occurrence": 64,
        "set_field": 20192
      }
    },
    "server": {
      "change_rows": 25609,
      "payload_bytes": 3916403,
      "by_operation": {
        "attachment_add": 32,
        "create_project": 1,
        "create_recurrence_series": 64,
        "create_task": 5128,
        "project_recurrence_occurrence": 128,
        "resolve_recurrence_occurrence": 64,
        "set_field": 20192
      }
    },
    "fresh": {
      "change_rows": 25609,
      "payload_bytes": 3916403,
      "by_operation": {
        "attachment_add": 32,
        "create_project": 1,
        "create_recurrence_series": 64,
        "create_task": 5128,
        "project_recurrence_occurrence": 128,
        "resolve_recurrence_occurrence": 64,
        "set_field": 20192
      }
    },
    "server_growth_bytes_per_change": 330.92365965090397,
    "projected_server_bytes_at_250k_changes": 83267491
  },
  "verification": {
    "source_open_title_conflicts": 64,
    "peer_open_title_conflicts": 64,
    "fresh_open_title_conflicts": 64,
    "fresh_tasks": 5128,
    "fresh_recurrence_series": 64,
    "fresh_live_attachments": 32,
    "server_distinct_referenced_blob_bytes": 2370244,
    "fresh_distinct_blob_bytes": 2370244
  },
  "derived": {
    "initial_own_change_pull_ratio": 0.0,
    "steady_state_own_change_pull_ratio": 0.0,
    "initial_ack_response_bytes_per_push": 478.13865232918647,
    "steady_state_ack_response_bytes_per_push": 418.265625
  }
}
```
