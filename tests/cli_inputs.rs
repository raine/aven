mod common;

use std::fs;

use common::{TestEnv, contains_all, extract_ref, fail, ok};

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
