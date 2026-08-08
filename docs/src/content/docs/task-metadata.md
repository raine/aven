---
title: Task metadata
description: Add workspace-scoped custom string values to tasks and recurring task templates.
---

Task metadata stores custom `key=value` information alongside Aven's built-in task fields. Use it for workflow state, source information, estimates, review details, or integration-specific values that do not belong in titles, descriptions, or labels.

:::note[TUI support]
The TUI does not yet show or edit custom task metadata. Use the CLI, Rust consumer API, or UniFFI facade to work with it.
:::

```sh
aven add "Review migration plan" \
  --metadata source=github \
  --metadata review-state=pending
```

## How metadata fields work

A metadata key becomes a workspace field the first time you assign it. You do not need to define fields before using them. Every field has a stable internal ID, while its key is the name shown in commands and task details.

Keys are normalized to lowercase and must:

- Begin with an ASCII letter.
- Contain only ASCII letters, numbers, `.`, `_`, or `-`.
- Contain at most 64 bytes.
- Avoid the reserved `aven.` prefix.

Fields belong to one workspace. The same key in another workspace represents a separate field.

Values are opaque UTF-8 strings. Aven does not infer numbers, dates, booleans, or other types, and task lists cannot sort by metadata values. Each value can contain up to 4 KiB. A task or recurring task template can hold up to 128 values and 32 KiB of metadata value data in total.

## Assign and remove values

Use repeatable `--metadata KEY=VALUE` options when creating or editing a task:

```sh
aven add "Prepare release" \
  --metadata effort=large \
  --metadata release-channel=stable

aven edit APP-7KQ9 \
  --metadata effort=medium \
  --metadata review-state=approved
```

Aven splits an assignment on the first `=`, so the value can contain additional equals signs:

```sh
aven edit APP-7KQ9 --metadata query='status=ready&team=docs'
```

An empty string is a present value:

```sh
aven edit APP-7KQ9 --metadata review-note=
```

Remove a value explicitly with `--remove-metadata`:

```sh
aven edit APP-7KQ9 --remove-metadata review-note
```

Setting and removing the same key in one command is rejected.

## Find tasks by metadata

`aven list` and `aven search` support exact, present, and missing predicates:

```sh
aven list --metadata review-state=pending
aven list --has-metadata effort
aven list --missing-metadata release-channel

aven search "migration" --metadata source=github
aven search "handoff" --has-metadata review-state
```

- `--metadata KEY=VALUE` requires that exact string value.
- `--has-metadata KEY` matches any present value, including an empty string.
- `--missing-metadata KEY` matches tasks without a value for that field.

Metadata predicates can be repeated and combine with the other task filters. Search text is matched against ordinary searchable task content, while metadata predicates restrict the candidate tasks.

## Inspect task metadata

Full task details include each key, stable field ID, and value:

```sh
aven show APP-7KQ9 --full
aven show APP-7KQ9 --full --json
```

Detailed JSON includes a `metadata` object keyed by the current field names and a `metadata_details` array containing `field_id`, `key`, and `value`. Compact task rows and summary task records omit metadata.

Task metadata is available through the CLI, Rust consumer API, and UniFFI facade.

## Bulk updates

Apply metadata changes to every task selected by `aven bulk-update`:

```sh
aven bulk-update --project app --status todo \
  --metadata review-state=pending \
  --dry-run

aven bulk-update --filter-label reviewed \
  --metadata review-state=approved \
  --remove-metadata review-note
```

A dry run reports the tasks that would change without creating fields or writing values. Bulk metadata changes share the command's all-or-nothing transaction behavior.

## Manage fields

List workspace fields and their task and recurring-template usage:

```sh
aven metadata list
aven metadata list --json
aven metadata show review-state
```

Rename a field with:

```sh
aven metadata rename review-state approval-state
```

A rename preserves the field's stable identity and updates how every related value is displayed without rewriting each task. The previous key becomes available for a separate field. Metadata fields have no delete command, but their values can be removed from every task and recurring task template.

## Recurring task metadata

Metadata supplied during recurring task creation belongs to both the template and its first task:

```sh
aven add "Daily review" \
  --repeat daily \
  --repeat-at 09:00 \
  --time-zone Europe/Stockholm \
  --metadata review-state=pending
```

Edit metadata used by tasks created later with:

```sh
aven recur edit RCR-7KP2 --metadata review-state=approved
aven recur edit RCR-7KP2 --remove-metadata review-state
```

Template edits do not rewrite the current task or historical tasks. Each task keeps the values copied into it when Aven created that task.

## Sync and data safety

Metadata fields, values, renames, recurring templates, and conflicts sync between replicas. Concurrent creation of the same key converges through replica-local field aliases. Conflicting edits retain empty strings and absent values as distinct choices during conflict resolution.

Metadata also participates in task undo and is included in portable export, import, database backup, restore, and integrity checks.

See the [command reference](/command-reference/) for complete option lists and [Sync across devices](/sync/) for sync setup and conflict behavior.
