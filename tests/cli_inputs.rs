mod common;

use std::fs;

use common::{TestEnv, contains_all, contains_none, extract_ref, fail, ok};

#[test]
fn description_sources_work() {
    let env = TestEnv::new();
    let db = env.db("descriptions.sqlite");

    let inline = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "inline description",
            "--project",
            "app",
            "--description",
            "inline body",
        ],
    )));

    let description_file = env.path("description.md");
    fs::write(&description_file, "# File description\n\nwith details\n").unwrap();
    let from_file = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "file description",
            "--project",
            "app",
            "--description-file",
            description_file.to_str().unwrap(),
        ],
    )));

    let from_stdin = extract_ref(&ok(env.aven_stdin(
        &db,
        [
            "add",
            "stdin description",
            "--project",
            "app",
            "--description-stdin",
        ],
        "## Stdin description\n\nfrom stdin\n",
    )));

    contains_all(
        &ok(env.aven(&db, ["show", &inline, "--full"])),
        &["description<<EOF", "inline body"],
    );
    contains_all(
        &ok(env.aven(&db, ["show", &from_file, "--full"])),
        &["description<<EOF", "# File description", "with details"],
    );
    contains_all(
        &ok(env.aven(&db, ["show", &from_stdin, "--full"])),
        &["description<<EOF", "## Stdin description", "from stdin"],
    );
}

#[test]
fn note_sources_work() {
    let env = TestEnv::new();
    let db = env.db("notes.sqlite");
    let task_ref = extract_ref(&ok(env.aven(&db, ["add", "noted task", "--project", "app"])));

    ok(env.aven(&db, ["note", &task_ref, "inline note"]));

    let note_file = env.path("note.txt");
    fs::write(&note_file, "file note\n").unwrap();
    ok(env.aven(
        &db,
        ["note", &task_ref, "--file", note_file.to_str().unwrap()],
    ));

    ok(env.aven_stdin(&db, ["note", &task_ref, "--stdin"], "stdin note\n"));

    let shown = ok(env.aven(&db, ["show", &task_ref, "--full"]));
    contains_all(&shown, &["inline note", "file note", "stdin note"]);
    contains_all(&shown, &["note created=", "body<<EOF"]);
}

#[test]
fn rejects_multiple_text_sources() {
    let env = TestEnv::new();
    let db = env.db("sources.sqlite");
    let description_file = env.path("description.md");
    fs::write(&description_file, "file body").unwrap();

    let error = fail(env.aven(
        &db,
        [
            "add",
            "bad description",
            "--project",
            "app",
            "--description",
            "inline",
            "--description-file",
            description_file.to_str().unwrap(),
        ],
    ));
    contains_all(&error, &["multiple-description-sources"]);

    let task_ref = extract_ref(&ok(env.aven(&db, ["add", "noted task", "--project", "app"])));
    let error = fail(env.aven_stdin(
        &db,
        ["note", &task_ref, "inline note", "--stdin"],
        "stdin note\n",
    ));
    contains_all(&error, &["multiple-note-sources"]);
}

#[test]
fn text_get_diff_and_safe_set_description() {
    let env = TestEnv::new();
    let db = env.db("text.sqlite");
    let task_ref = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "markdown edit",
            "--project",
            "app",
            "--description",
            "# Old\n\nbody\n",
        ],
    )));
    let export = env.path("description.md");
    let got = ok(env.aven(
        &db,
        [
            "text",
            "get",
            &task_ref,
            "description",
            "--output",
            export.to_str().unwrap(),
        ],
    ));
    contains_all(&got, &["sha256="]);
    assert_eq!(std::fs::read_to_string(&export).unwrap(), "# Old\n\nbody\n");
    let hash = got
        .split_whitespace()
        .find_map(|part| part.strip_prefix("sha256="))
        .unwrap()
        .to_string();
    std::fs::write(&export, "# New\n\nbody\n").unwrap();
    let diff = ok(env.aven(
        &db,
        [
            "text",
            "diff",
            &task_ref,
            "description",
            "--file",
            export.to_str().unwrap(),
        ],
    ));
    contains_all(&diff, &["--- current", "+++ candidate", "-# Old", "+# New"]);
    ok(env.aven(
        &db,
        [
            "text",
            "set",
            &task_ref,
            "description",
            "--file",
            export.to_str().unwrap(),
            "--if-sha256",
            &hash,
        ],
    ));
    let shown = ok(env.aven(&db, ["show", &task_ref, "--full"]));
    contains_all(&shown, &["description<<EOF", "# New"]);
}

#[test]
fn text_set_rejects_stale_description_hash() {
    let env = TestEnv::new();
    let db = env.db("stale.sqlite");
    let task_ref = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "stale markdown",
            "--project",
            "app",
            "--description",
            "first",
        ],
    )));
    let path = env.path("candidate.md");
    std::fs::write(&path, "second").unwrap();
    let error = fail(env.aven(
        &db,
        [
            "text",
            "set",
            &task_ref,
            "description",
            "--file",
            path.to_str().unwrap(),
            "--if-sha256",
            "0000",
        ],
    ));
    contains_all(&error, &["error text-hash-mismatch", "field=description"]);
    let shown = ok(env.aven(&db, ["show", &task_ref, "--full"]));
    contains_all(&shown, &["first"]);
    contains_none(&shown, &["second"]);
}

#[test]
fn text_set_description_from_stdin() {
    let env = TestEnv::new();
    let db = env.db("text-stdin.sqlite");
    let task_ref = extract_ref(&ok(env.aven(
        &db,
        [
            "add",
            "stdin text edit",
            "--project",
            "app",
            "--description",
            "old\n",
        ],
    )));
    let got = ok(env.aven(&db, ["text", "get", &task_ref, "description"]));
    let hash = got
        .split_whitespace()
        .find_map(|part| part.strip_prefix("sha256="))
        .expect("sha256")
        .to_string();
    ok(env.aven_stdin(
        &db,
        [
            "text",
            "set",
            &task_ref,
            "description",
            "--stdin",
            "--if-sha256",
            &hash,
        ],
        "new from stdin\n",
    ));
    contains_all(
        &ok(env.aven(&db, ["show", &task_ref, "--full"])),
        &["new from stdin"],
    );
}
