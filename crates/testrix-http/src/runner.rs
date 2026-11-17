use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use reqwest::{
    Method,
    header::{HeaderMap, HeaderName, HeaderValue},
    multipart,
    redirect::Policy,
};
use testrix_core::{
    HttpMethod, HttpResponseSnapshot, PreparedBody, PreparedMultipartPart, PreparedRequest,
    ResponseHeader, SentRequestSnapshot, format_pretty_body,
};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::error::HttpError;

/// Reusable HTTP execution engine backed by a shared Tokio runtime and Reqwest client pool.
#[derive(Clone)]
pub struct HttpRunner {
    inner: Arc<RunnerInner>,
}

struct RunnerInner {
    runtime: tokio::runtime::Runtime,
    client_follow_verified: reqwest::Client,
    client_nofollow_verified: reqwest::Client,
    client_follow_insecure: reqwest::Client,
    client_nofollow_insecure: reqwest::Client,
}

impl HttpRunner {
    /// Creates a new HTTP runner with dedicated connection pools and Tokio worker threads.
    pub fn new() -> Result<Self, HttpError> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("testrix-http")
            .enable_all()
            .build()
            .map_err(|err| HttpError::Runtime(err.to_string()))?;

        let build_client = |follow: bool, verify_tls: bool| -> Result<reqwest::Client, HttpError> {
            let policy = if follow {
                Policy::limited(10)
            } else {
                Policy::none()
            };
            reqwest::Client::builder()
                .redirect(policy)
                .danger_accept_invalid_certs(!verify_tls)
                .user_agent("Testrix/0.1")
                .build()
                .map_err(HttpError::Transport)
        };

        Ok(Self {
            inner: Arc::new(RunnerInner {
                runtime,
                client_follow_verified: build_client(true, true)?,
                client_nofollow_verified: build_client(false, true)?,
                client_follow_insecure: build_client(true, false)?,
                client_nofollow_insecure: build_client(false, false)?,
            }),
        })
    }

    /// Dispatches `request` onto the Tokio runtime and returns a oneshot receiver for UI executors.
    pub fn dispatch(
        &self,
        request: PreparedRequest,
        cancel_token: CancellationToken,
    ) -> oneshot::Receiver<Result<HttpResponseSnapshot, HttpError>> {
        let (tx, rx) = oneshot::channel();
        let inner = Arc::clone(&self.inner);

        self.inner.runtime.spawn(async move {
            let result = execute_inner(&inner, request, cancel_token).await;
            let _ = tx.send(result);
        });

        rx
    }

    /// Executes `request` synchronously on the internal runtime (useful for tests/CLI).
    pub fn execute_blocking(
        &self,
        request: PreparedRequest,
        cancel_token: CancellationToken,
    ) -> Result<HttpResponseSnapshot, HttpError> {
        let inner = Arc::clone(&self.inner);
        self.inner
            .runtime
            .block_on(execute_inner(&inner, request, cancel_token))
    }
}

async fn execute_inner(
    inner: &RunnerInner,
    request: PreparedRequest,
    cancel_token: CancellationToken,
) -> Result<HttpResponseSnapshot, HttpError> {
    if cancel_token.is_cancelled() {
        return Err(HttpError::Cancelled);
    }

    let client = match (
        request.settings.follow_redirects,
        request.settings.verify_tls,
    ) {
        (true, true) => &inner.client_follow_verified,
        (false, true) => &inner.client_nofollow_verified,
        (true, false) => &inner.client_follow_insecure,
        (false, false) => &inner.client_nofollow_insecure,
    };

    let method = map_method(request.method);
    let mut header_map = HeaderMap::new();
    for h in &request.headers {
        let name = HeaderName::from_bytes(h.name.trim().as_bytes()).map_err(|err| {
            HttpError::InvalidHeader {
                name: h.name.clone(),
                reason: err.to_string(),
            }
        })?;
        let val = HeaderValue::from_str(&h.value).map_err(|err| HttpError::InvalidHeader {
            name: h.name.clone(),
            reason: err.to_string(),
        })?;
        header_map.insert(name, val);
    }

    let sent_snapshot = SentRequestSnapshot {
        method: request.method,
        url: request.url.clone(),
        headers: request.headers.clone(),
        body_preview: request.body.preview(),
    };

    let timeout = Duration::from_millis(request.settings.timeout_ms.max(1));
    let mut builder = client
        .request(method, &request.url)
        .headers(header_map)
        .timeout(timeout);

    builder = match request.body {
        PreparedBody::None => builder,
        PreparedBody::Text { content } => builder.body(content),
        PreparedBody::FormUrlEncoded { fields } => builder.form(&fields),
        PreparedBody::Multipart { parts } => {
            let mut form = multipart::Form::new();
            for part in parts {
                match part {
                    PreparedMultipartPart::Text {
                        name,
                        value,
                        content_type,
                    } => {
                        let mut p = multipart::Part::text(value);
                        if let Some(ct) = content_type {
                            p = p.mime_str(&ct).map_err(|err| HttpError::InvalidHeader {
                                name: "Content-Type".to_string(),
                                reason: err.to_string(),
                            })?;
                        }
                        form = form.part(name, p);
                    }
                    PreparedMultipartPart::File {
                        name,
                        path,
                        file_name,
                        content_type,
                    } => {
                        let path_buf = Path::new(&path).to_path_buf();
                        let bytes = tokio::fs::read(&path_buf).await.map_err(|source| {
                            HttpError::BodyFileRead {
                                path: path_buf.clone(),
                                source,
                            }
                        })?;
                        let derived_name = file_name.unwrap_or_else(|| {
                            path_buf
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("upload.bin")
                                .to_string()
                        });
                        let mut p = multipart::Part::bytes(bytes).file_name(derived_name);
                        if let Some(ct) = content_type {
                            p = p.mime_str(&ct).map_err(|err| HttpError::InvalidHeader {
                                name: "Content-Type".to_string(),
                                reason: err.to_string(),
                            })?;
                        }
                        form = form.part(name, p);
                    }
                }
            }
            builder.multipart(form)
        }
        PreparedBody::Binary { file_path, .. } => {
            let path_buf = Path::new(&file_path).to_path_buf();
            let bytes =
                tokio::fs::read(&path_buf)
                    .await
                    .map_err(|source| HttpError::BodyFileRead {
                        path: path_buf,
                        source,
                    })?;
            builder.body(bytes)
        }
    };

    debug!(method = %request.method, url = %request.url, "sending HTTP request");
    let started = Instant::now();

    let response = tokio::select! {
        biased;
        () = cancel_token.cancelled() => {
            warn!(url = %request.url, "HTTP request cancelled");
            return Err(HttpError::Cancelled);
        }
        res = builder.send() => res?,
    };

    let status = response.status();
    let version = format!("{:?}", response.version());
    let headers: Vec<ResponseHeader> = response
        .headers()
        .iter()
        .map(|(name, value)| ResponseHeader {
            name: name.to_string(),
            value: String::from_utf8_lossy(value.as_bytes()).into_owned(),
        })
        .collect();

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let body_bytes = tokio::select! {
        biased;
        () = cancel_token.cancelled() => return Err(HttpError::Cancelled),
        res = response.bytes() => res?.to_vec(),
    };

    let duration_ms = started.elapsed().as_millis() as u64;
    let size_bytes = body_bytes.len();
    let body_text = String::from_utf8_lossy(&body_bytes).into_owned();
    let pretty_body = tokio::task::spawn_blocking({
        let body_text = body_text.clone();
        let content_type = content_type.clone();
        move || format_pretty_body(&body_text, content_type.as_deref())
    })
    .await
    .unwrap_or_else(|_| body_text.clone());

    Ok(HttpResponseSnapshot {
        status_code: status.as_u16(),
        reason: status.canonical_reason().unwrap_or("").to_string(),
        http_version: version,
        duration_ms,
        size_bytes,
        headers,
        body_bytes,
        body_text,
        pretty_body,
        content_type,
        sent_request: sent_snapshot,
    })
}

const fn map_method(method: HttpMethod) -> Method {
    match method {
        HttpMethod::Get => Method::GET,
        HttpMethod::Post => Method::POST,
        HttpMethod::Put => Method::PUT,
        HttpMethod::Delete => Method::DELETE,
        HttpMethod::Patch => Method::PATCH,
        HttpMethod::Head => Method::HEAD,
        HttpMethod::Options => Method::OPTIONS,
    }
}
