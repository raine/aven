use super::*;
pub(super) use aven_core::sync::client::tail::IMAGES_PATH as PATH;
use aven_core::sync::encrypted_tail::attachments::{
    self as images, Operation as ImageOperation, Reply as ImageReply,
};

const CODES: http_admission::Codes = http_admission::codes!("encrypted-image");
pub(super) async fn handle(State(server): State<Arc<Server>>, request: Request) -> Response {
    let server = &*server;
    let outcome = http_admission::dispatch(
        &server.gate,
        REQUEST_TIMEOUT,
        request,
        images::HTTP_LIMIT,
        |headers, bytes| async move {
            match dispatch(&server.db, headers, bytes, server.image_policy).await {
                Ok(reply) => http_admission::reply(&CODES, &reply, images::HTTP_LIMIT),
                Err(error) => http_admission::operation_refusal(&CODES, &error),
            }
        },
    )
    .await;
    http_admission::respond(&CODES, outcome)
}
async fn dispatch(
    db: &Database,
    headers: HeaderMap,
    bytes: Option<Bytes>,
    policy: aven_core::attachments::LifecyclePolicy,
) -> Result<Envelope<ImageReply>> {
    let bytes = CODES.json_body(&headers, bytes)?;
    let bearer = CODES.bearer(&headers)?;
    let input: Envelope<ImageOperation> = CODES.parse(&bytes)?;
    if !matches!(input.operation, ImageOperation::Put { .. }) && bytes.len() > tail::CONTROL_LIMIT {
        return Err(CODES.too_large().into());
    }
    let operation = db
        .encrypted_image_exchange(&input.context, &bearer, input.operation, policy)
        .await?;
    Ok(Envelope {
        context: input.context,
        correlation: input.correlation,
        operation,
    })
}
