//! Admission precedence is endpoint-specific, including public enrollment mailboxes.
use crate::driver::{Driver, Request, exchange};
use aven_protocol::wire::{bootstrap, enrollment, images, tail};

pub async fn ordering(driver: &impl Driver) {
    for (path, family, limit) in [
        (
            bootstrap::PATH,
            "bootstrap",
            bootstrap::batch::MAX_BYTES.max(bootstrap::REQUEST_LIMIT),
        ),
        (enrollment::PATH, "enrollment", enrollment::CONTROL_LIMIT),
        (tail::PATH, "encrypted-tail", tail::APPEND_LIMIT),
        (tail::BATCH_PATH, "encrypted-tail", tail::BATCH_APPEND_LIMIT),
        (images::PATH, "encrypted-image", images::HTTP_LIMIT),
    ] {
        for (content_type, authorization, body, status, suffix) in [
            ("text/plain", None, b"invalid".to_vec(), 415, "content-type"),
            (
                "application/json",
                Some("Bearer invalid".into()),
                b"invalid".to_vec(),
                401,
                "credential",
            ),
            (
                "application/json",
                Some("Bearer invalid".into()),
                vec![b' '; limit + 1],
                413,
                "limit",
            ),
            (
                "application/json",
                Some(format!("Bearer {}", "00".repeat(32))),
                b"invalid".to_vec(),
                400,
                "malformed",
            ),
        ] {
            exchange(
                driver,
                Request {
                    path,
                    content_type,
                    authorization,
                    body,
                },
            )
            .await
            .refusal(status, &format!("{family}-{suffix}"));
        }
        // Only enrollment parses an anonymous operation before deciding whether it needs a bearer.
        let response = exchange(
            driver,
            Request {
                path,
                content_type: "application/json",
                authorization: None,
                body: b"invalid".to_vec(),
            },
        )
        .await;
        if path == enrollment::PATH {
            response.refusal(400, "enrollment-malformed");
        } else {
            response.refusal(401, &format!("{family}-credential"));
        }
    }
}
