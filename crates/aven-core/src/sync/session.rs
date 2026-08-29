use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use flate2::write::GzEncoder;
use getrandom::fill as fill_random;

use super::blob::{
    MissingLocalBlob, attachment_blob_contract, contract_by_hash, unique_blob_contracts,
};
use super::persistence::{ApplySyncPage, ClientSyncPage};
use super::planner::{
    PendingChange, TransferBudget, TransferObject, plan_change_prefix, plan_transfers,
};
use super::wire::{
    BlobUploadContract, MAX_BLOB_TRANSFER_BYTES, MAX_BLOB_TRANSFER_OBJECTS, MAX_PULL_BATCH,
    MAX_PUSH_BATCH, MissingBlobsRequest, MissingBlobsResponse, SyncResponse,
    sync_server_url_is_valid, validate_blob_hashes,
};
use crate::attachments::lifecycle::{ByteCount, LifecyclePolicy};
use crate::db::Database;
use crate::ids::{new_id, now};

const GZIP_THRESHOLD: usize = 256;
const MAX_HTTP_ERROR_DETAIL_BYTES: usize = 1024;
const MAX_SYNC_ATTEMPTS: u32 = 4;
const BASE_RETRY_DELAY_MS: u64 = 250;
const MAX_RETRY_DELAY_MS: u64 = 4_000;
const MAX_RETRY_AFTER_MS: u64 = 30_000;
const METADATA_ATTEMPT_TIMEOUT_MS: u64 = 30_000;
const METADATA_INACTIVITY_TIMEOUT_MS: u64 = 10_000;
const BLOB_INACTIVITY_TIMEOUT_MS: u64 = 30_000;
const BLOB_ATTEMPT_BASE_TIMEOUT_MS: u64 = 60_000;
const BLOB_FLOOR_BYTES_PER_SECOND: u64 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncRequestContext {
    session_id: String,
    request: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncHttpHeader {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncRequestTimeout {
    pub attempt_ms: u64,
    pub inactivity_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncRetryDecision {
    RetryAfter { delay_ms: u64 },
    Stop,
}

#[derive(Clone)]
pub struct PreparedSyncRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<SyncHttpHeader>,
    pub body: Vec<u8>,
    pub timeout: SyncRequestTimeout,
    pub context: SyncRequestContext,
}

pub struct SyncHttpResponse {
    pub status: u16,
    pub headers: Vec<SyncHttpHeader>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPageOutcome {
    pub page: usize,
    pub pushed: usize,
    pub pulled: usize,
    pub blob_uploaded: usize,
    pub blob_uploaded_bytes: u64,
    pub blob_downloaded: usize,
    pub blob_downloaded_bytes: u64,
    pub cursor: i64,
    pub complete: bool,
    pub has_more: bool,
    pub local_more: bool,
    pub request_bytes: usize,
    pub request_wire_bytes: usize,
    pub response_decoded_bytes: usize,
    pub response_compression: String,
    pub apply_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncSessionSummary {
    pub pushed: i64,
    pub pulled: usize,
    pub blob_uploaded: usize,
    pub blob_uploaded_bytes: u64,
    pub blob_downloaded: usize,
    pub blob_downloaded_bytes: u64,
    pub blob_upload_remaining: usize,
    pub blob_upload_remaining_bytes: u64,
    pub blob_download_remaining: usize,
    pub blob_download_remaining_bytes: u64,
    pub cursor: i64,
    pub complete: bool,
    pub pages: usize,
    pub request_bytes: usize,
    pub request_wire_bytes: usize,
    pub response_decoded_bytes: usize,
    pub response_compression: String,
    pub apply_ms: u128,
}

struct ActivePage {
    page: ClientSyncPage,
    contracts: Vec<BlobUploadContract>,
    probed: bool,
    missing: HashSet<String>,
    uploads: Vec<BlobUploadContract>,
    upload_index: usize,
    confirmed: bool,
    metadata_sent: bool,
    downloads: Vec<MissingLocalBlob>,
    download_index: usize,
    has_more: bool,
    page_pushed: usize,
    page_pulled: usize,
    page_uploaded: usize,
    page_uploaded_bytes: u64,
    page_downloaded: usize,
    page_downloaded_bytes: u64,
    transfer_budget_blocked: bool,
}

impl ActivePage {
    fn new(mut page: ClientSyncPage) -> Result<Self> {
        let mut hashes = HashSet::new();
        let mut count = page.request.changes.len();
        for (index, change) in page.request.changes.iter().enumerate() {
            if let Some(contract) = attachment_blob_contract(change)?
                && hashes.insert(contract.sha256)
                && hashes.len() > MAX_BLOB_TRANSFER_OBJECTS
            {
                count = index;
                break;
            }
        }
        page.request.changes.truncate(count);
        page.pending = count;
        let contracts = unique_blob_contracts(&page.request.changes)?;
        Ok(Self {
            page,
            contracts,
            probed: false,
            missing: HashSet::new(),
            uploads: Vec::new(),
            upload_index: 0,
            confirmed: false,
            metadata_sent: false,
            downloads: Vec::new(),
            download_index: 0,
            has_more: false,
            page_pushed: 0,
            page_pulled: 0,
            page_uploaded: 0,
            page_uploaded_bytes: 0,
            page_downloaded: 0,
            page_downloaded_bytes: 0,
            transfer_budget_blocked: false,
        })
    }
}

#[derive(Clone)]
enum RequestKind {
    Missing,
    Upload {
        lease_id: String,
        contract: BlobUploadContract,
    },
    Confirm,
    Metadata {
        request_bytes: usize,
    },
    Download {
        blob: MissingLocalBlob,
    },
}

struct OutstandingRequest {
    prepared: PreparedSyncRequest,
    kind: RequestKind,
    attempts: u32,
}

pub struct SyncSession {
    database: Database,
    server: String,
    auth_token: Option<String>,
    blob_dir: PathBuf,
    lifecycle_policy: LifecyclePolicy,
    page_budget: Option<usize>,
    attempted_at: String,
    session_id: String,
    request_number: usize,
    active: Option<ActivePage>,
    outstanding: Option<OutstandingRequest>,
    known_server_blobs: HashSet<String>,
    transfer_budget: TransferBudget,
    summary: SyncSessionSummary,
    last_has_more: bool,
    last_local_more: bool,
    stopped: bool,
}

impl SyncSession {
    pub async fn start(
        database: Database,
        server: String,
        auth_token: Option<String>,
        page_budget: Option<usize>,
    ) -> Result<Self> {
        let blob_dir = crate::attachments::default_blob_dir(database.path());
        Self::start_with_attachment_storage(
            database,
            server,
            auth_token,
            page_budget,
            blob_dir,
            LifecyclePolicy::default(),
        )
        .await
    }

    pub async fn start_with_attachment_storage(
        database: Database,
        server: String,
        auth_token: Option<String>,
        page_budget: Option<usize>,
        blob_dir: PathBuf,
        lifecycle_policy: LifecyclePolicy,
    ) -> Result<Self> {
        let attempted_at = now();
        database.begin_sync_attempt(attempted_at.clone()).await?;
        if !sync_server_url_is_valid(&server) {
            let message = "invalid sync server URL";
            database.record_sync_error(message.to_string()).await?;
            bail!("{message}");
        }
        Ok(Self {
            database,
            server: server.trim_end_matches('/').to_string(),
            auth_token,
            blob_dir,
            lifecycle_policy,
            page_budget,
            attempted_at,
            session_id: new_id(),
            request_number: 0,
            active: None,
            outstanding: None,
            known_server_blobs: HashSet::new(),
            transfer_budget: TransferBudget {
                objects: MAX_BLOB_TRANSFER_OBJECTS,
                bytes: MAX_BLOB_TRANSFER_BYTES,
                completed_objects: 0,
            },
            summary: SyncSessionSummary {
                pushed: 0,
                pulled: 0,
                blob_uploaded: 0,
                blob_uploaded_bytes: 0,
                blob_downloaded: 0,
                blob_downloaded_bytes: 0,
                blob_upload_remaining: 0,
                blob_upload_remaining_bytes: 0,
                blob_download_remaining: 0,
                blob_download_remaining_bytes: 0,
                cursor: 0,
                complete: false,
                pages: 0,
                request_bytes: 0,
                request_wire_bytes: 0,
                response_decoded_bytes: 0,
                response_compression: "none".to_string(),
                apply_ms: 0,
            },
            last_has_more: false,
            last_local_more: false,
            stopped: false,
        })
    }

    pub async fn prepare_request(&mut self) -> Result<Option<PreparedSyncRequest>> {
        if self.stopped {
            return Ok(None);
        }
        if let Some(outstanding) = &self.outstanding {
            return Ok(Some(outstanding.prepared.clone()));
        }
        if self.active.is_none() {
            let page = self
                .database
                .prepare_client_sync_page(self.server.clone(), MAX_PUSH_BATCH, MAX_PULL_BATCH)
                .await?;
            self.active = Some(ActivePage::new(page)?);
        }

        let active = self.active.as_ref().expect("active sync page");
        if !active.contracts.is_empty() && !active.probed {
            return self.prepare_json_request(
                "POST",
                "/sync/blobs/missing",
                &MissingBlobsRequest {
                    blobs: active.contracts.clone(),
                },
                RequestKind::Missing,
            );
        }
        if active.upload_index < active.uploads.len() {
            let contract = active.uploads[active.upload_index].clone();
            let upload = self
                .database
                .prepare_blob_upload(&self.blob_dir, &contract)
                .await?;
            let mut headers = vec![header("content-type", &contract.media_type)];
            headers.push(header("x-aven-workspace-id", &contract.workspace_id));
            headers.push(header("x-aven-byte-size", &contract.byte_size.to_string()));
            headers.push(header("x-aven-width", &contract.width.to_string()));
            headers.push(header("x-aven-height", &contract.height.to_string()));
            return self.prepare_raw_request(
                "PUT",
                &format!("/sync/blobs/{}", contract.sha256),
                headers,
                upload.bytes,
                RequestKind::Upload {
                    lease_id: upload.lease_id,
                    contract,
                },
            );
        }
        if !active.contracts.is_empty() && !active.confirmed {
            return self.prepare_json_request(
                "POST",
                "/sync/blobs/missing",
                &MissingBlobsRequest {
                    blobs: active.contracts.clone(),
                },
                RequestKind::Confirm,
            );
        }
        if !active.metadata_sent {
            let body = serde_json::to_vec(&active.page.request).context("encode sync request")?;
            let request_bytes = body.len();
            let mut headers = vec![header("content-type", "application/json")];
            let body = if request_bytes > GZIP_THRESHOLD {
                headers.push(header("content-encoding", "gzip"));
                gzip_encode(&body)?
            } else {
                body
            };
            return self.prepare_raw_request(
                "POST",
                "/sync",
                headers,
                body,
                RequestKind::Metadata { request_bytes },
            );
        }
        if active.download_index < active.downloads.len() {
            let blob = active.downloads[active.download_index].clone();
            return self.prepare_raw_request(
                "GET",
                &format!("/sync/blobs/{}", blob.sha256),
                vec![header("accept-encoding", "identity")],
                Vec::new(),
                RequestKind::Download { blob },
            );
        }

        self.finish_page().await?;
        if self.stopped {
            Ok(None)
        } else {
            Box::pin(self.prepare_request()).await
        }
    }

    fn prepare_json_request<T: serde::Serialize>(
        &mut self,
        method: &str,
        path: &str,
        value: &T,
        kind: RequestKind,
    ) -> Result<Option<PreparedSyncRequest>> {
        self.prepare_raw_request(
            method,
            path,
            vec![header("content-type", "application/json")],
            serde_json::to_vec(value)?,
            kind,
        )
    }

    fn prepare_raw_request(
        &mut self,
        method: &str,
        path: &str,
        mut headers: Vec<SyncHttpHeader>,
        body: Vec<u8>,
        kind: RequestKind,
    ) -> Result<Option<PreparedSyncRequest>> {
        if let Some(token) = &self.auth_token {
            headers.push(header("authorization", &format!("Bearer {token}")));
        }
        self.request_number += 1;
        let timeout = request_timeout(&kind);
        let prepared = PreparedSyncRequest {
            method: method.to_string(),
            url: format!("{}{}", self.server, path),
            headers,
            body,
            timeout,
            context: SyncRequestContext {
                session_id: self.session_id.clone(),
                request: self.request_number,
            },
        };
        self.outstanding = Some(OutstandingRequest {
            prepared: prepared.clone(),
            kind,
            attempts: 1,
        });
        Ok(Some(prepared))
    }

    pub fn register_transport_failure(
        &mut self,
        context: &SyncRequestContext,
    ) -> Result<SyncRetryDecision> {
        self.register_retry(context, None)
    }

    pub fn register_http_failure(
        &mut self,
        context: &SyncRequestContext,
        status: u16,
        headers: &[SyncHttpHeader],
    ) -> Result<SyncRetryDecision> {
        {
            let outstanding = self
                .outstanding
                .as_ref()
                .context("no outstanding sync request")?;
            validate_context(&outstanding.prepared.context, context)?;
        }
        if !retryable_http_status(status) {
            return Ok(SyncRetryDecision::Stop);
        }
        let retry_after = retry_after_ms(headers, Utc::now().timestamp());
        self.register_retry(context, retry_after)
    }

    fn register_retry(
        &mut self,
        context: &SyncRequestContext,
        retry_after_ms: Option<u64>,
    ) -> Result<SyncRetryDecision> {
        let outstanding = self
            .outstanding
            .as_mut()
            .context("no outstanding sync request")?;
        validate_context(&outstanding.prepared.context, context)?;
        if outstanding.attempts >= MAX_SYNC_ATTEMPTS {
            return Ok(SyncRetryDecision::Stop);
        }
        let delay_ms = retry_after_ms.unwrap_or_else(|| retry_delay_ms(outstanding.attempts));
        outstanding.attempts += 1;
        Ok(SyncRetryDecision::RetryAfter { delay_ms })
    }

    pub async fn accept_response(
        &mut self,
        context: &SyncRequestContext,
        response: SyncHttpResponse,
    ) -> Result<SyncPageOutcome> {
        {
            let outstanding = self
                .outstanding
                .as_ref()
                .context("no outstanding sync request")?;
            validate_context(&outstanding.prepared.context, context)?;
        }
        if !(200..300).contains(&response.status) {
            bail!("{}", sync_http_error(&response));
        }
        let outstanding = self
            .outstanding
            .as_ref()
            .expect("validated outstanding request");
        let prepared_body_len = outstanding.prepared.body.len();
        let request_kind = outstanding.kind.clone();
        let response_bytes = response.body.len();
        match request_kind {
            RequestKind::Missing => {
                let decoded: MissingBlobsResponse = serde_json::from_slice(&response.body)
                    .context("decode missing blobs response")?;
                validate_blob_hashes(&decoded.missing)?;
                let active = self.active.as_mut().expect("active sync page");
                let requested = active
                    .contracts
                    .iter()
                    .map(|c| c.sha256.as_str())
                    .collect::<HashSet<_>>();
                if decoded
                    .missing
                    .iter()
                    .any(|hash| !requested.contains(hash.as_str()))
                {
                    bail!("error invalid-blob-missing-response");
                }
                let missing = decoded.missing.into_iter().collect::<HashSet<_>>();
                self.known_server_blobs.extend(
                    active
                        .contracts
                        .iter()
                        .filter(|contract| !missing.contains(&contract.sha256))
                        .map(|contract| contract.sha256.clone()),
                );
                active.probed = true;
                active.missing = missing;
                let pending = active
                    .page
                    .request
                    .changes
                    .iter()
                    .map(|change| {
                        let blob = match attachment_blob_contract(change)? {
                            Some(contract) if active.missing.contains(&contract.sha256) => {
                                Some(TransferObject {
                                    sha256: contract.sha256,
                                    byte_size: u64::try_from(contract.byte_size)
                                        .context("attachment bytes exceed u64")?,
                                })
                            }
                            _ => None,
                        };
                        Ok(PendingChange { missing_blob: blob })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let plan = plan_change_prefix(&pending, self.transfer_budget);
                active.transfer_budget_blocked = plan.change_count < pending.len();
                active.page.request.changes.truncate(plan.change_count);
                active.page.pending = plan.change_count;
                active.contracts = unique_blob_contracts(&active.page.request.changes)?;
                active.uploads = plan
                    .transfers
                    .iter()
                    .map(|object| contract_by_hash(&active.contracts, &object.sha256))
                    .collect::<Result<Vec<_>>>()?;
            }
            RequestKind::Upload { lease_id, contract } => {
                self.database.finish_blob_upload(&lease_id).await?;
                let bytes =
                    u64::try_from(contract.byte_size).context("attachment bytes exceed u64")?;
                self.known_server_blobs.insert(contract.sha256.clone());
                self.transfer_budget.consume(&TransferObject {
                    sha256: contract.sha256,
                    byte_size: bytes,
                });
                let active = self.active.as_mut().expect("active sync page");
                active.upload_index += 1;
                active.page_uploaded += 1;
                active.page_uploaded_bytes += bytes;
                self.summary.blob_uploaded += 1;
                self.summary.blob_uploaded_bytes += bytes;
            }
            RequestKind::Confirm => {
                let decoded: MissingBlobsResponse = serde_json::from_slice(&response.body)
                    .context("decode missing blobs response")?;
                validate_blob_hashes(&decoded.missing)?;
                if !decoded.missing.is_empty() {
                    bail!("error attachment-blob-admission-missing");
                }
                self.active.as_mut().expect("active sync page").confirmed = true;
            }
            RequestKind::Metadata { request_bytes } => {
                let decoded: SyncResponse =
                    serde_json::from_slice(&response.body).context("decode sync response")?;
                let active = self.active.as_mut().expect("active sync page");
                let request = active.page.request.clone();
                let pending = active.page.pending;
                let has_more = decoded.has_more;
                let cursor = decoded.cursor;
                let apply_started = Instant::now();
                let pulled = self
                    .database
                    .apply_client_sync_page(ApplySyncPage {
                        request,
                        response: decoded,
                        attempted_at: self.attempted_at.clone(),
                        previous_pushed: self.summary.pushed,
                        previous_pulled: self.summary.pulled,
                    })
                    .await?;
                let apply_ms = apply_started.elapsed().as_millis();
                active.metadata_sent = true;
                active.has_more = has_more;
                active.page_pushed = pending;
                active.page_pulled = pulled;
                self.summary.pushed += pending as i64;
                self.summary.pulled += pulled;
                self.summary.cursor = cursor;
                self.summary.request_bytes += request_bytes;
                self.summary.request_wire_bytes += prepared_body_len;
                self.summary.response_decoded_bytes += response_bytes;
                self.summary.response_compression =
                    header_value(&response.headers, "content-encoding")
                        .unwrap_or("none")
                        .to_string();
                self.summary.apply_ms += apply_ms;
                let missing_page = self
                    .database
                    .missing_local_blob_page(MAX_BLOB_TRANSFER_OBJECTS)
                    .await?;
                let has_more_missing = missing_page.has_more;
                let missing = missing_page.blobs;
                let objects = missing
                    .iter()
                    .map(|blob| {
                        Ok(TransferObject {
                            sha256: blob.sha256.clone(),
                            byte_size: u64::try_from(blob.byte_size)
                                .context("attachment bytes exceed u64")?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let plan = plan_transfers(&objects, self.transfer_budget);
                active.transfer_budget_blocked |= has_more_missing || plan.len() < objects.len();
                let planned = plan
                    .iter()
                    .map(|o| o.sha256.as_str())
                    .collect::<HashSet<_>>();
                active.downloads = missing
                    .into_iter()
                    .filter(|b| planned.contains(b.sha256.as_str()))
                    .collect();
            }
            RequestKind::Download { blob } => {
                let expected =
                    usize::try_from(blob.byte_size).context("attachment bytes exceed usize")?;
                let content_length_valid = header_value(&response.headers, "content-length")
                    .map(|length| length.parse::<usize>().ok() == Some(expected))
                    .unwrap_or(true);
                if !content_length_valid || response.body.len() != expected {
                    bail!("error attachment-blob-remote-invalid");
                }
                self.database
                    .store_downloaded_blob(
                        &self.blob_dir,
                        self.lifecycle_policy,
                        &blob,
                        response.body,
                    )
                    .await?;
                let bytes = u64::try_from(blob.byte_size).context("attachment bytes exceed u64")?;
                self.transfer_budget.consume(&TransferObject {
                    sha256: blob.sha256,
                    byte_size: bytes,
                });
                let active = self.active.as_mut().expect("active sync page");
                active.download_index += 1;
                active.page_downloaded += 1;
                active.page_downloaded_bytes += bytes;
                self.summary.blob_downloaded += 1;
                self.summary.blob_downloaded_bytes += bytes;
            }
        }
        self.outstanding = None;
        let page_finished = self.active.as_ref().is_some_and(|active| {
            active.metadata_sent && active.download_index >= active.downloads.len()
        });
        if page_finished {
            let mut outcome = self.outcome();
            self.finish_page().await?;
            outcome.page = self.summary.pages;
            outcome.complete = self.summary.complete;
            outcome.has_more = self.last_has_more;
            outcome.local_more = self.last_local_more;
            return Ok(outcome);
        }
        Ok(self.outcome())
    }

    async fn finish_page(&mut self) -> Result<()> {
        let active = self.active.take().expect("active sync page");
        self.summary.pages += 1;
        self.last_has_more = active.has_more;
        let local_more = self.database.pending_sync_changes_exist().await?;
        self.last_local_more = local_more;
        let complete = !local_more && !active.has_more && !active.transfer_budget_blocked;
        self.summary.complete = complete;
        let budget_exhausted = self
            .page_budget
            .is_some_and(|budget| self.summary.pages >= budget);
        let transfer_stalled = active.transfer_budget_blocked
            || self.transfer_budget.objects == 0
            || self.transfer_budget.bytes == 0;
        let should_stop = complete || budget_exhausted || transfer_stalled;
        if should_stop {
            let download_counts = if complete {
                ByteCount::default()
            } else {
                self.database.missing_local_blob_counts().await?
            };
            self.summary.blob_download_remaining = usize::try_from(download_counts.count)?;
            self.summary.blob_download_remaining_bytes = download_counts.bytes;
            let upload_counts = self
                .database
                .pending_blob_counts(&self.known_server_blobs)
                .await?;
            self.summary.blob_upload_remaining = usize::try_from(upload_counts.count)?;
            self.summary.blob_upload_remaining_bytes = upload_counts.bytes;
            self.stop();
        }
        Ok(())
    }

    fn stop(&mut self) {
        self.stopped = true;
    }

    fn outcome(&self) -> SyncPageOutcome {
        let active = self.active.as_ref();
        SyncPageOutcome {
            page: self.summary.pages + usize::from(active.is_some_and(|page| page.metadata_sent)),
            pushed: active.map_or(0, |page| page.page_pushed),
            pulled: active.map_or(0, |page| page.page_pulled),
            blob_uploaded: active.map_or(0, |page| page.page_uploaded),
            blob_uploaded_bytes: active.map_or(0, |page| page.page_uploaded_bytes),
            blob_downloaded: active.map_or(0, |page| page.page_downloaded),
            blob_downloaded_bytes: active.map_or(0, |page| page.page_downloaded_bytes),
            cursor: self.summary.cursor,
            complete: self.summary.complete,
            has_more: active.map_or(self.last_has_more, |page| page.has_more),
            local_more: self.last_local_more,
            request_bytes: self.summary.request_bytes,
            request_wire_bytes: self.summary.request_wire_bytes,
            response_decoded_bytes: self.summary.response_decoded_bytes,
            response_compression: self.summary.response_compression.clone(),
            apply_ms: self.summary.apply_ms,
        }
    }

    pub async fn fail_request(
        &mut self,
        context: &SyncRequestContext,
        error: impl Into<String>,
    ) -> Result<()> {
        let outstanding = self
            .outstanding
            .as_ref()
            .context("no outstanding sync request")?;
        validate_context(&outstanding.prepared.context, context)?;
        self.fail(error).await
    }

    pub async fn fail(&mut self, error: impl Into<String>) -> Result<()> {
        if let Some(OutstandingRequest {
            kind: RequestKind::Upload { lease_id, .. },
            ..
        }) = &self.outstanding
        {
            self.database.finish_blob_upload(lease_id).await?;
        }
        self.database.record_sync_error(error.into()).await?;
        self.outstanding = None;
        self.stop();
        Ok(())
    }

    pub fn summary(&self) -> SyncSessionSummary {
        self.summary.clone()
    }
}

fn request_timeout(kind: &RequestKind) -> SyncRequestTimeout {
    match kind {
        RequestKind::Missing | RequestKind::Confirm | RequestKind::Metadata { .. } => {
            SyncRequestTimeout {
                attempt_ms: METADATA_ATTEMPT_TIMEOUT_MS,
                inactivity_ms: METADATA_INACTIVITY_TIMEOUT_MS,
            }
        }
        RequestKind::Upload { contract, .. } => blob_request_timeout(contract.byte_size),
        RequestKind::Download { blob } => blob_request_timeout(blob.byte_size),
    }
}

fn blob_request_timeout(byte_size: i64) -> SyncRequestTimeout {
    let bytes = u64::try_from(byte_size).unwrap_or(0);
    let transfer_ms = bytes
        .saturating_mul(1_000)
        .div_ceil(BLOB_FLOOR_BYTES_PER_SECOND);
    SyncRequestTimeout {
        attempt_ms: BLOB_ATTEMPT_BASE_TIMEOUT_MS.saturating_add(transfer_ms),
        inactivity_ms: BLOB_INACTIVITY_TIMEOUT_MS,
    }
}

fn retryable_http_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500 | 502 | 503 | 504)
}

fn retry_delay_ms(failed_attempts: u32) -> u64 {
    let mut entropy = [0_u8; 8];
    if fill_random(&mut entropy).is_err() {
        entropy = u64::from(failed_attempts).to_le_bytes();
    }
    retry_delay_with_entropy(failed_attempts, u64::from_le_bytes(entropy))
}

fn retry_delay_with_entropy(failed_attempts: u32, entropy: u64) -> u64 {
    let exponent = failed_attempts.saturating_sub(1);
    let cap = (BASE_RETRY_DELAY_MS * (1_u64 << exponent)).min(MAX_RETRY_DELAY_MS);
    entropy % (cap + 1)
}

fn retry_after_ms(headers: &[SyncHttpHeader], now_unix_seconds: i64) -> Option<u64> {
    let mut values = headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("retry-after"));
    let value = values.next()?.value.trim();
    if values.next().is_some() {
        return None;
    }
    let milliseconds = if let Ok(seconds) = value.parse::<u64>() {
        seconds.saturating_mul(1_000)
    } else {
        let timestamp = parse_http_date(value)?;
        u64::try_from(timestamp.saturating_sub(now_unix_seconds))
            .unwrap_or(0)
            .saturating_mul(1_000)
    };
    Some(milliseconds.min(MAX_RETRY_AFTER_MS))
}

fn parse_http_date(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc2822(value)
        .ok()
        .map(|value| value.timestamp())
}

fn header(name: &str, value: &str) -> SyncHttpHeader {
    SyncHttpHeader {
        name: name.to_string(),
        value: value.to_string(),
    }
}

fn validate_context(expected: &SyncRequestContext, actual: &SyncRequestContext) -> Result<()> {
    if expected != actual {
        bail!("sync request context does not match the outstanding page");
    }
    Ok(())
}

fn header_value<'a>(headers: &'a [SyncHttpHeader], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value.as_str())
}

fn sync_http_error(response: &SyncHttpResponse) -> String {
    let detail = header_value(&response.headers, "content-type")
        .filter(|content_type| {
            content_type
                .split(';')
                .next()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/plain"))
        })
        .and_then(|_| std::str::from_utf8(&response.body).ok())
        .map(str::trim)
        .filter(|detail| {
            detail.starts_with("error ")
                && detail.len() <= MAX_HTTP_ERROR_DETAIL_BYTES
                && detail.is_ascii()
                && !detail.chars().any(char::is_control)
        });

    let detail = match detail {
        Some(detail) => actionable_sync_http_error_detail(detail).unwrap_or_else(|| detail.to_string()),
        None => match response.status {
            400 => {
                "server rejected the sync request; check server logs and verify client/server version compatibility"
            }
            401 => "authentication failed; check sync.auth_token",
            403 => "server denied the sync request; check server access settings",
            404 => "sync endpoint not found; check sync.server_url",
            429 => "server rate limit exceeded; retry later",
            507 => "error attachment-quota-exceeded",
            500..=599 => "sync server failed; check server logs",
            _ => "server returned no readable error details",
        }
        .to_string(),
    };
    format!(
        "sync request failed with HTTP status {}: {detail}",
        response.status
    )
}

fn actionable_sync_http_error_detail(detail: &str) -> Option<String> {
    let fields = detail.strip_prefix("error sync-protocol-unsupported ")?;
    let mut fields = fields.split_whitespace();
    let client = fields
        .next()?
        .strip_prefix("client=")?
        .parse::<u32>()
        .ok()?;
    let server = fields
        .next()?
        .strip_prefix("server=")?
        .parse::<u32>()
        .ok()?;
    if fields.next().is_some() || client == server {
        return None;
    }
    let action = if client > server {
        "upgrade the sync server"
    } else {
        "upgrade the sync client"
    };
    Some(format!(
        "client and server use incompatible sync protocol versions (client {client}, server {server}); {action}"
    ))
}

fn gzip_encode(body: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(body)
        .context("gzip encode sync request body")?;
    encoder.finish().context("finish sync request compression")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_accepts_http_forms_and_caps_delays() {
        let now = 1_445_412_480;
        let headers = [header("retry-after", "Wed, 21 Oct 2015 07:28:02 GMT")];
        assert_eq!(retry_after_ms(&headers, now), Some(2_000));
        assert_eq!(
            retry_after_ms(&[header("retry-after", "86400")], now),
            Some(MAX_RETRY_AFTER_MS)
        );
        assert_eq!(
            retry_after_ms(
                &[header("retry-after", "1"), header("retry-after", "2")],
                now
            ),
            None
        );
        assert_eq!(
            retry_after_ms(&[header("retry-after", "invalid")], now),
            None
        );
    }

    #[test]
    fn backoff_uses_full_bounded_jitter() {
        assert_eq!(retry_delay_with_entropy(1, 0), 0);
        assert_eq!(retry_delay_with_entropy(1, 250), 250);
        assert_eq!(retry_delay_with_entropy(2, 500), 500);
        assert_eq!(retry_delay_with_entropy(3, 1_000), 1_000);
    }

    #[test]
    fn blob_timeout_scales_with_transfer_size() {
        let small = blob_request_timeout(1);
        let large = blob_request_timeout(64 * 1024 * 1024);
        assert_eq!(small.inactivity_ms, BLOB_INACTIVITY_TIMEOUT_MS);
        assert_eq!(large.inactivity_ms, BLOB_INACTIVITY_TIMEOUT_MS);
        assert!(large.attempt_ms > METADATA_ATTEMPT_TIMEOUT_MS);
        assert!(large.attempt_ms > small.attempt_ms);
    }

    #[test]
    fn retryable_statuses_exclude_deterministic_failures() {
        for status in [408, 425, 429, 500, 502, 503, 504] {
            assert!(retryable_http_status(status));
        }
        for status in [400, 401, 403, 404, 501, 505, 507] {
            assert!(!retryable_http_status(status));
        }
    }
}
