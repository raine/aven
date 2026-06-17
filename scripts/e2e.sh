#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

cargo build >/dev/null
BIN="${ATM_BIN:-$ROOT/target/debug/atm}"
TMP="$(mktemp -d)"
SERVER_PID=""

cleanup() {
    if [[ -n "$SERVER_PID" ]]; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    rm -rf "$TMP"
}
trap cleanup EXIT

fail() {
    echo "e2e failed: $*" >&2
    exit 1
}

run_db() {
    local db="$1"
    shift
    "$BIN" --db "$db" "$@"
}

need_contains() {
    local haystack="$1"
    local needle="$2"
    [[ "$haystack" == *"$needle"* ]] || fail "missing '$needle' in: $haystack"
}

ref_suffix() {
    awk '{print $2}' | cut -d- -f2
}

start_server() {
    local log="$TMP/server.log"
    "$BIN" server --bind 127.0.0.1:0 --data "$TMP/server.sqlite" >"$log" 2>&1 &
    SERVER_PID="$!"
    for _ in $(seq 1 100); do
        SERVER_URL="$(sed -n 's/^listening url=//p' "$log" | head -1)"
        if [[ -n "${SERVER_URL:-}" ]]; then
            return
        fi
        sleep 0.1
    done
    cat "$log" >&2 || true
    fail "server did not print listening url"
}

A="$TMP/client-a.sqlite"
B="$TMP/client-b.sqlite"
IMPLICIT="$TMP/implicit.sqlite"

ATM_DB="$IMPLICIT" "$BIN" project create implicit >/dev/null
[[ -f "$IMPLICIT" ]] || fail "implicit database was not created"

run_db "$A" label create bug >/dev/null
run_db "$A" label create sync >/dev/null
run_db "$A" label create docs >/dev/null

add_out="$(run_db "$A" add "fix sync conflict display" --project app --label bug --priority high)"
need_contains "$add_out" "created APP-"
REF="$(printf '%s\n' "$add_out" | awk '{print $2}')"
SUFFIX="$(printf '%s\n' "$add_out" | ref_suffix)"

desc_out="$(printf '## Context\nstdin description\n' | run_db "$A" add "document stdin" --project app --description-stdin)"
DESC_REF="$(printf '%s\n' "$desc_out" | awk '{print $2}')"
printf 'note from stdin\n' | run_db "$A" note "$DESC_REF" --stdin >/dev/null
printf '# File description\n' >"$TMP/description.txt"
file_desc_out="$(run_db "$A" add "document file" --project app --description-file "$TMP/description.txt")"
FILE_DESC_REF="$(printf '%s\n' "$file_desc_out" | awk '{print $2}')"
printf 'note from file\n' >"$TMP/note.txt"
run_db "$A" note "$FILE_DESC_REF" --file "$TMP/note.txt" >/dev/null

mkdir -p "$TMP/mapped/sub"
run_db "$A" project create mapped --path "$TMP/mapped" >/dev/null
mapped_out="$(cd "$TMP/mapped/sub" && "$BIN" --db "$A" add "mapped inference")"
need_contains "$mapped_out" "project=mapped"

mkdir -p "$TMP/git-inferred"
git -C "$TMP/git-inferred" init -q
git_out="$(cd "$TMP/git-inferred" && "$BIN" --db "$A" add "git inference")"
need_contains "$git_out" "project=git-inferred"

mkdir -p "$TMP/no-project"
if (cd "$TMP/no-project" && "$BIN" --db "$TMP/no-project.sqlite" add "no project" >"$TMP/no-project.out" 2>&1); then
    fail "projectless task succeeded"
fi
need_contains "$(cat "$TMP/no-project.out")" "error project-required"

list_out="$(run_db "$A" list)"
need_contains "$list_out" 'status=inbox'
need_contains "$list_out" 'labels=bug'

show_out="$(run_db "$A" show "$REF" --full)"
need_contains "$show_out" 'id='

update_out="$(run_db "$A" update "$REF" --title "fix conflict display" --status active --priority urgent --label sync --remove-label bug --project homelab)"
need_contains "$update_out" "updated HML-"
MOVED_REF="$(printf '%s\n' "$update_out" | awk '{print $2}')"
MOVED_SUFFIX="$(printf '%s\n' "$update_out" | awk '{print $2}' | cut -d- -f2)"
[[ "$SUFFIX" == "$MOVED_SUFFIX" ]] || fail "project move changed suffix"

short_ref="${MOVED_SUFFIX:0:3}"
short_show="$(run_db "$A" show "$short_ref")"
need_contains "$short_show" "HML-$MOVED_SUFFIX"

if run_db "$A" add "bad label" --project homelab --label bux >"$TMP/bad-label.out" 2>&1; then
    fail "unknown label succeeded"
fi
need_contains "$(cat "$TMP/bad-label.out")" "choice bug"

if run_db "$A" add "near project" --project home-lab >"$TMP/near-project.out" 2>&1; then
    fail "near project succeeded"
fi
need_contains "$(cat "$TMP/near-project.out")" "choice homelab"

run_db "$A" project create ambig >/dev/null
sqlite3 "$A" "INSERT INTO tasks(id,title,description,project_key,status,priority,created_at,updated_at) VALUES ('7KQ1111111111111','ambig one','','ambig','inbox','none','t','t')"
sqlite3 "$A" "INSERT INTO tasks(id,title,description,project_key,status,priority,created_at,updated_at) VALUES ('7KQ2222222222222','ambig two','','ambig','inbox','none','t','t')"
if run_db "$A" show 7KQ >"$TMP/ambig.out" 2>&1; then
    fail "ambiguous ref succeeded"
fi
need_contains "$(cat "$TMP/ambig.out")" "error ambiguous-ref"

run_db "$A" delete "$MOVED_REF" >/dev/null
normal_after_delete="$(run_db "$A" list)"
if [[ "$normal_after_delete" == *"$MOVED_SUFFIX"* ]]; then
    fail "deleted task appeared in normal list"
fi
all_after_delete="$(run_db "$A" list --all)"
need_contains "$all_after_delete" "$MOVED_SUFFIX"
run_db "$A" restore "$MOVED_REF" >/dev/null
normal_after_restore="$(run_db "$A" list)"
need_contains "$normal_after_restore" "$MOVED_SUFFIX"

start_server
[[ "$SERVER_URL" == http://127.0.0.1:* ]] || fail "unexpected server url $SERVER_URL"

a_task="$(run_db "$A" add "offline from a" --project app)"
b_task="$(run_db "$B" add "offline from b" --project app)"
A_REF="$(printf '%s\n' "$a_task" | awk '{print $2}')"
B_REF="$(printf '%s\n' "$b_task" | awk '{print $2}')"

run_db "$A" sync --server "$SERVER_URL" >/dev/null
run_db "$B" sync --server "$SERVER_URL" >/dev/null
run_db "$A" sync --server "$SERVER_URL" >/dev/null
list_a="$(run_db "$A" list --all)"
list_b="$(run_db "$B" list --all)"
need_contains "$list_a" "$B_REF"
need_contains "$list_b" "$A_REF"

run_db "$A" update "$A_REF" --status active >/dev/null
run_db "$B" update "$A_REF" --priority high >/dev/null
run_db "$A" sync --server "$SERVER_URL" >/dev/null
run_db "$B" sync --server "$SERVER_URL" >/dev/null
run_db "$A" sync --server "$SERVER_URL" >/dev/null
merged="$(run_db "$A" show "$A_REF")"
need_contains "$merged" "status=active"
need_contains "$merged" "priority=high"

printf 'note a\n' | run_db "$A" note "$A_REF" --stdin >/dev/null
printf 'note b\n' | run_db "$B" note "$A_REF" --stdin >/dev/null
run_db "$A" update "$A_REF" --label docs >/dev/null
run_db "$B" update "$A_REF" --label sync >/dev/null
run_db "$A" sync --server "$SERVER_URL" >/dev/null
run_db "$B" sync --server "$SERVER_URL" >/dev/null
run_db "$A" sync --server "$SERVER_URL" >/dev/null
full="$(run_db "$A" show "$A_REF" --full)"
need_contains "$full" "note a"
need_contains "$full" "note b"
need_contains "$full" "labels=docs,sync"

run_db "$A" update "$A_REF" --remove-label docs >/dev/null
run_db "$B" update "$A_REF" --label bug >/dev/null
run_db "$A" sync --server "$SERVER_URL" >/dev/null
run_db "$B" sync --server "$SERVER_URL" >/dev/null
run_db "$A" sync --server "$SERVER_URL" >/dev/null
labels_after_remove="$(run_db "$A" show "$A_REF")"
need_contains "$labels_after_remove" "labels=bug,sync"

conflict_task="$(run_db "$A" add "conflict base" --project app)"
CONFLICT_REF="$(printf '%s\n' "$conflict_task" | awk '{print $2}')"
run_db "$A" sync --server "$SERVER_URL" >/dev/null
run_db "$B" sync --server "$SERVER_URL" >/dev/null
run_db "$A" update "$CONFLICT_REF" --title "title from a" >/dev/null
run_db "$B" update "$CONFLICT_REF" --title "title from b" >/dev/null
run_db "$A" sync --server "$SERVER_URL" >/dev/null
run_db "$B" sync --server "$SERVER_URL" >/dev/null
run_db "$A" sync --server "$SERVER_URL" >/dev/null

conflicts="$(run_db "$A" conflict list)"
need_contains "$conflicts" "conflict field=title"
conflicted_list="$(run_db "$A" list --all)"
need_contains "$conflicted_list" "conflicts=yes"
if run_db "$A" update "$CONFLICT_REF" --title "should fail" >"$TMP/conflicted-update.out" 2>&1; then
    fail "conflicted field update succeeded"
fi
need_contains "$(cat "$TMP/conflicted-update.out")" "error conflicted-field"
run_db "$A" update "$CONFLICT_REF" --priority urgent >/dev/null
conflicted_show="$(run_db "$A" show "$CONFLICT_REF")"
need_contains "$conflicted_show" "conflicts=yes"
shown="$(run_db "$A" conflict show "$CONFLICT_REF" --field title)"
TOKEN="$(printf '%s\n' "$shown" | awk '/^variant / {print $2; exit}')"
[[ -n "$TOKEN" ]] || fail "missing conflict variant token"
run_db "$A" conflict resolve "$CONFLICT_REF" title --use "$TOKEN" >/dev/null
run_db "$A" sync --server "$SERVER_URL" >/dev/null
run_db "$B" sync --server "$SERVER_URL" >/dev/null
conflicts_b_after_resolve="$(run_db "$B" conflict list)"
if [[ "$conflicts_b_after_resolve" == *"$CONFLICT_REF"* ]]; then
    fail "resolved variant conflict still visible on client b"
fi

desc_conflict="$(run_db "$A" add "description conflict" --project app)"
DESC_CONFLICT_REF="$(printf '%s\n' "$desc_conflict" | awk '{print $2}')"
run_db "$A" sync --server "$SERVER_URL" >/dev/null
run_db "$B" sync --server "$SERVER_URL" >/dev/null
run_db "$A" update "$DESC_CONFLICT_REF" --description "description from a" >/dev/null
run_db "$B" update "$DESC_CONFLICT_REF" --description "description from b" >/dev/null
run_db "$A" sync --server "$SERVER_URL" >/dev/null
run_db "$B" sync --server "$SERVER_URL" >/dev/null
printf 'resolved description\n' | run_db "$B" conflict resolve "$DESC_CONFLICT_REF" description --value-stdin >/dev/null
run_db "$B" sync --server "$SERVER_URL" >/dev/null
run_db "$A" sync --server "$SERVER_URL" >/dev/null
resolved_description="$(run_db "$A" show "$DESC_CONFLICT_REF" --full)"
need_contains "$resolved_description" "resolved description"

just check

echo "e2e ok"
