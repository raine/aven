use super::*;
pub(super) use aven_core::sync::client::tail::IMAGES_PATH as PATH;
use aven_core::sync::encrypted_tail::attachments::{
    self as images, Operation as ImageOperation, Reply as ImageReply,
};

pub(super) async fn handle(State(server): State<Arc<Server>>, request: Request) -> Response {
    let response = match http_admission::dispatch(
        &server.gate,
        REQUEST_TIMEOUT,
        request,
        images::HTTP_LIMIT,
        |headers, bytes| dispatch(&server.db, headers, bytes, server.image_policy),
    )
    .await
    {
        Outcome::Dispatched(Ok(reply)) => http_admission::json(&reply, images::HTTP_LIMIT)
            .unwrap_or_else(|| {
                (StatusCode::INTERNAL_SERVER_ERROR, "encrypted_image_refused").into_response()
            }),
        Outcome::Dispatched(Err(error)) if is_stale(&error) => {
            (StatusCode::CONFLICT, "membership-stale").into_response()
        }
        Outcome::Dispatched(Err(_)) => {
            (StatusCode::CONFLICT, "encrypted_image_refused").into_response()
        }
        Outcome::DispatchTimeout => {
            (StatusCode::REQUEST_TIMEOUT, "encrypted_image_timeout").into_response()
        }
        Outcome::PermitTimeout => {
            let mut response =
                (StatusCode::SERVICE_UNAVAILABLE, "encrypted_image_busy").into_response();
            http_admission::mark_busy(&mut response);
            response
        }
    };
    http_admission::no_store(response)
}
async fn dispatch(
    db: &Database,
    headers: HeaderMap,
    bytes: Option<Bytes>,
    policy: aven_core::attachments::LifecyclePolicy,
) -> Result<Envelope<ImageReply>> {
    ensure!(
        http_admission::is_json(&headers),
        "error encrypted-image-http"
    );
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(http_admission::bearer)
        .ok_or_else(|| anyhow::anyhow!("error encrypted-image-credential"))?;
    let bytes = bytes.ok_or_else(|| anyhow::anyhow!("error encrypted-image-limit"))?;
    let input: Envelope<ImageOperation> = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("error encrypted-image-http"))?;
    ensure!(
        matches!(input.operation, ImageOperation::Put { .. }) || bytes.len() <= tail::CONTROL_LIMIT,
        "error encrypted-image-limit"
    );
    let operation = db
        .encrypted_image_exchange(&input.context, &bearer, input.operation, policy)
        .await?;
    Ok(Envelope {
        context: input.context,
        correlation: input.correlation,
        operation,
    })
}
