//! Entry points for the fuzz targets in the repository's `fuzz/` crate.
//!
//! Every target accepts arbitrary bytes. Refusal is the expected outcome for
//! most inputs; a panic, hang or unbounded allocation is a finding. Accepted
//! inputs are also checked for canonical round trips, so a decoder that
//! accepts two encodings of one value fails loudly.
//!
//! Structured inputs are a selector byte followed by fields framed by
//! [`frame`]. Openers seal fuzzed plaintext under the fixed publication
//! fixture's key, so fuzzing reaches the checks behind authentication.

/// Bytes split into a selector and length-prefixed fields.
pub(crate) struct Input<'a>(pub(crate) &'a [u8]);

impl<'a> Input<'a> {
    pub(crate) fn byte(&mut self) -> u8 {
        let Some((&b, rest)) = self.0.split_first() else {
            return 0;
        };
        self.0 = rest;
        b
    }
    /// A U16 big-endian length-prefixed field, truncated at the end of input.
    pub(crate) fn part(&mut self) -> &'a [u8] {
        let len = usize::from(u16::from_be_bytes([self.byte(), self.byte()]));
        let (part, rest) = self.0.split_at(len.min(self.0.len()));
        self.0 = rest;
        part
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Encodes raw selector bytes and fields so the last field is the unframed rest.
pub fn frame(head: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let mut out = head.to_vec();
    if let Some((last, framed)) = parts.split_last() {
        for part in framed {
            out.extend(u16::try_from(part.len()).expect("small part").to_be_bytes());
            out.extend(*part);
        }
        out.extend(*last);
    }
    out
}

/// Encrypted tail records: envelope open, sealed arbitrary plaintext,
/// domain decode and projection framing.
pub fn tail(data: &[u8]) {
    super::encrypted_tail::fuzz::tail(data)
}

/// Domain-valid decrypted operations applied in order to a blank replica.
pub fn tail_apply(data: &[u8]) {
    super::encrypted_tail::fuzz::apply(data)
}

/// Encrypted image descriptors and chunk records.
pub fn attachment(data: &[u8]) {
    super::encrypted_tail::fuzz::attachment(data)
}

/// Genesis, publication, evidence, pairing and signed membership records.
pub fn membership(data: &[u8]) {
    super::seed_claim::membership::test_support::fuzz(data)
}

/// Bootstrap descriptors, catalogs, decrypted domain state and upload batches.
pub fn bootstrap(data: &[u8]) {
    super::shared_state::package::publication::fuzz::bootstrap(data)
}

/// Pasted or scanned invitation text.
pub fn invitation(text: &[u8]) {
    use super::client::invitation::{DeviceInvitation, InvitationCheck, SetupInvitation};
    use super::invitation_text;
    let Ok(text) = std::str::from_utf8(text) else {
        return;
    };
    let _ = InvitationCheck::of(text);
    if let Some(decoded) = invitation_text::decode(text) {
        let encoded = invitation_text::encode(decoded.kind, &decoded.server, &decoded.secret)
            .expect("a decoded invitation re-encodes");
        let again = invitation_text::decode(&encoded).expect("re-encoded invitation decodes");
        assert_eq!(
            (again.kind, &again.server, &again.secret),
            (decoded.kind, &decoded.server, &decoded.secret)
        );
    }
    if let Ok(setup) = SetupInvitation::decode(text) {
        let encoded = setup
            .encode()
            .expect("a decoded setup invitation re-encodes");
        let again = SetupInvitation::decode(&encoded).expect("setup round trip");
        assert_eq!(again.setup_id, setup.setup_id);
    }
    if let Ok(device) = DeviceInvitation::decode(text) {
        let encoded = device
            .encode()
            .expect("a decoded device invitation re-encodes");
        let again = DeviceInvitation::decode(&encoded).expect("device round trip");
        assert_eq!(again.invitation.vault(), device.invitation.vault());
    }
}

/// JSON bodies a client or server reads from the other side.
pub fn wire(data: &[u8]) {
    use super::encrypted_tail::{BatchOperation, BatchReply, Operation, Reply, batch};
    use super::seed_claim::membership::{Evidence, Mailbox};
    let mut input = Input(data);
    let selector = input.byte();
    let body = input.0;
    match selector % 7 {
        0 => drop(serde_json::from_slice::<Reply>(body)),
        1 => drop(serde_json::from_slice::<Operation>(body)),
        2 => drop(serde_json::from_slice::<batch::Envelope<BatchReply>>(body)),
        3 => drop(serde_json::from_slice::<batch::Envelope<BatchOperation>>(
            body,
        )),
        4 => drop(Evidence::decode_unverified(body)),
        5 => drop(serde_json::from_slice::<Mailbox>(body)),
        _ => super::encrypted_tail::fuzz::strict_json(body),
    }
}

/// Runs `f` over a replica with a small local task history.
pub(crate) fn with_history<T>(f: impl AsyncFnOnce(&crate::db::Database) -> T) -> T {
    use crate::operations::{TaskDraft, TaskUpdate};
    use crate::test_support as t;
    let dir = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let db = t::open_blank_database(&dir.path().join("seed.db"))
            .await
            .unwrap();
        let mut conn = db.acquire_writer().await.unwrap();
        let w = t::ensure_default_workspace(&mut conn).await.unwrap();
        for label in ["bug", "ux", "later"] {
            t::create_label_operation(&mut conn, &w, label)
                .await
                .unwrap();
        }
        let draft = |title: &str, is_epic| TaskDraft {
            title: title.into(),
            description: "body".into(),
            project: Some("app".into()),
            status: Some("todo".into()),
            priority: "high".into(),
            source: crate::choices::TaskSource::Cli,
            labels: vec!["bug".into()],
            metadata: Vec::new(),
            available_at: None,
            due_on: Some("2026-10-04".into()),
            is_epic,
        };
        let mut ids = Vec::new();
        for (title, is_epic) in [("a", false), ("b", false), ("epic", true)] {
            let task = t::create_task(&mut conn, &w, draft(title, is_epic))
                .await
                .unwrap();
            ids.push(task.task.id);
        }
        let [a, b, epic] = &ids[..] else {
            unreachable!()
        };
        let update = TaskUpdate {
            title: Some("renamed".into()),
            status: Some("active".into()),
            add_labels: vec!["ux".into()],
            remove_labels: vec!["bug".into()],
            ..TaskUpdate::default()
        };
        t::update_task(&mut conn, &w, a, update).await.unwrap();
        t::add_task_dependency(&mut conn, &w, a, b).await.unwrap();
        t::add_task_to_epic(&mut conn, &w, b, epic).await.unwrap();
        t::set_task_deleted(&mut conn, &w, b, true).await.unwrap();
        drop(conn);
        f(&db).await
    })
}

/// Valid inputs for each target, named by target, to seed fuzz corpora.
pub fn seeds() -> Vec<(&'static str, Vec<u8>)> {
    use super::invitation_text::{Kind, encode};
    let mut seeds = Vec::new();
    for (kind, server) in [
        (Kind::Setup, "https://sync.example.com"),
        (Kind::Device, "http://127.0.0.1:8080"),
    ] {
        let secret = vec![7; kind.secret_len()];
        let text = encode(kind, server, &secret).unwrap();
        seeds.push(("invitation", text.as_bytes().to_vec()));
    }
    seeds.extend(super::encrypted_tail::fuzz::seeds());
    seeds.extend(super::seed_claim::membership::test_support::fuzz_seeds());
    seeds.extend(super::shared_state::package::publication::fuzz::seeds());
    seeds
}
