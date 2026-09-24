//! Conflict commands over conflicts that encrypted sync produced.
use super::*;

async fn edit_both(pair: &Pair, task: &str, flag: &str, a_value: &str, b_value: &str) {
    pair.a.ok(&["edit", task, flag, a_value]).await;
    pair.b.ok(&["edit", task, flag, b_value]).await;
    converge(&[&pair.a, &pair.b, &pair.a]).await;
}

async fn conflict_list(node: &Installation) -> String {
    node.ok(&["conflict", "list"]).await
}

#[tokio::test]
async fn conflict_commands_inspect_and_resolve_encrypted_sync_conflicts() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let seed = Installation::new(root, "a");
    let title = created_ref(&seed.ok(&["add", "Conflict base", "--project", "app"]).await);
    let body = created_ref(
        &seed
            .ok(&["add", "Description base", "--project", "app"])
            .await,
    );
    let deleted = created_ref(&seed.ok(&["add", "Deleted base", "--project", "app"]).await);
    let epic = created_ref(
        &seed
            .ok(&["add", "Conflict epic", "--project", "app", "--epic"])
            .await,
    );
    seed.ok(&["workspace", "create", "client"]).await;
    seed.ok(&[
        "--workspace",
        "client",
        "add",
        "Client seed task",
        "--project",
        "app",
    ])
    .await;
    let pair = pair(root).await;
    let (a, b) = (&pair.a, &pair.b);

    // Workspace records and scoped tasks cross the snapshot, and later scoped
    // tasks cross the tail.
    a.ok(&[
        "--workspace",
        "client",
        "add",
        "Later task",
        "--project",
        "app",
    ])
    .await;
    converge(&[a, b]).await;
    for (workspace, task) in [("client", "Client seed task"), ("client", "Later task")] {
        let listed = b.ok(&["--workspace", workspace, "list"]).await;
        assert!(listed.contains(task), "{listed}");
        let default = b.ok(&["--workspace", "default", "list"]).await;
        assert!(!default.contains(task), "{default}");
    }

    // Concurrent scalar edits produce a protected, listed and searchable conflict.
    edit_both(&pair, &title, "--title", "title from a", "title from b").await;
    let listed = conflict_list(a).await;
    assert!(
        listed.contains(&title) && listed.contains("conflict field=title"),
        "{listed}"
    );
    assert!(a.ok(&["list", "--all"]).await.contains("conflicts=yes"));
    let error = failure(&a.run(&["edit", &title, "--title", "should fail"]).await);
    assert!(error.contains("error conflicted-field"), "{error}");
    a.ok(&["edit", &title, "--priority", "urgent"]).await;
    let context: serde_json::Value =
        serde_json::from_str(&a.ok(&["context", &title, "--json"]).await).unwrap();
    assert_eq!(context["has_conflicts"], true);
    assert_eq!(context["conflicts"][0]["field"], "title");
    assert_eq!(
        context["conflicts"][0]["variants"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let json: serde_json::Value =
        serde_json::from_str(&a.ok(&["conflict", "list", "--json", "--limit", "1"]).await).unwrap();
    assert_eq!(json.as_array().unwrap().len(), 1);
    let shown: serde_json::Value = serde_json::from_str(
        &a.ok(&["conflict", "show", &title, "--field", "title", "--json"])
            .await,
    )
    .unwrap();
    assert_eq!(shown[0]["field"], "title");
    let token = shown[0]["variants"][0]["token"]
        .as_str()
        .unwrap()
        .to_string();
    a.ok(&["conflict", "resolve", &title, "title", "--use", &token])
        .await;
    converge(&[a, b]).await;
    assert!(!conflict_list(a).await.contains(&title));
    assert!(!conflict_list(b).await.contains(&title));

    // Description variants export and diff, and a stdin resolution syncs.
    edit_both(
        &pair,
        &body,
        "--description",
        "description from a\n",
        "description from b\n",
    )
    .await;
    let diff = a.ok(&["conflict", "diff", &body, "description"]).await;
    assert!(diff.contains("---") && diff.contains("+++"), "{diff}");
    let dir = root.join("variants");
    a.ok(&[
        "conflict",
        "export",
        &body,
        "description",
        "--dir",
        &dir.display().to_string(),
    ])
    .await;
    let mut bodies = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<Vec<_>>();
    bodies.sort();
    assert_eq!(bodies, ["description from a\n", "description from b\n"]);
    success(
        &b.run_with_input(
            &["conflict", "resolve", &body, "description", "--value-stdin"],
            "resolved description\n",
        )
        .await,
        &["conflict", "resolve"],
    );
    converge(&[b, a]).await;
    let resolved = a.ok(&["show", &body, "--full"]).await;
    assert!(resolved.contains("resolved description"), "{resolved}");
    assert!(!conflict_list(a).await.contains(&body));

    // An invalid deletion resolution changes nothing.
    b.ok(&["delete", &deleted]).await;
    a.ok(&["delete", &deleted]).await;
    a.ok(&["restore", &deleted]).await;
    converge(&[b, a]).await;
    assert!(conflict_list(a).await.contains("conflict field=deleted"));
    let error = failure(
        &a.run(&[
            "conflict", "resolve", &deleted, "deleted", "--value", "true",
        ])
        .await,
    );
    assert!(error.contains("error invalid-deleted"), "{error}");
    assert!(conflict_list(a).await.contains("conflict field=deleted"));
    assert!(a.ok(&["show", &deleted]).await.contains("Deleted base"));

    // Epic demotion racing a new child shows on/off and keeps the children.
    let child = created_ref(&b.ok(&["add", "Epic child", "--project", "app"]).await);
    b.ok(&["epic", "add", &child, &epic]).await;
    a.ok(&["edit", &epic, "--epic", "off"]).await;
    converge(&[b, a, b]).await;
    let shown = b
        .ok(&["conflict", "show", &epic, "--field", "is_epic"])
        .await;
    assert!(shown.contains("field=is_epic"), "{shown}");
    assert!(shown.contains("on") && shown.contains("off"), "{shown}");
    let error = failure(
        &b.run(&["conflict", "resolve", &epic, "is_epic", "--value", "0"])
            .await,
    );
    assert!(error.contains("error epic-has-children"), "{error}");
}
