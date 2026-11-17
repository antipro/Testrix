use crate::{materialize::PreparedHeader, model::HttpMethod};

/// Snapshot of the actual request transmitted over the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentRequestSnapshot {
    pub method: HttpMethod,
    pub url: String,
    pub headers: Vec<PreparedHeader>,
    pub body_preview: String,
}

/// Header pair returned by an HTTP response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseHeader {
    pub name: String,
    pub value: String,
}

/// Complete HTTP response snapshot rendered in the workbench response pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponseSnapshot {
    pub status_code: u16,
    pub reason: String,
    pub http_version: String,
    pub duration_ms: u64,
    pub size_bytes: usize,
    pub headers: Vec<ResponseHeader>,
    pub body_bytes: Vec<u8>,
    pub body_text: String,
    pub pretty_body: String,
    pub content_type: Option<String>,
    pub sent_request: SentRequestSnapshot,
}

impl HttpResponseSnapshot {
    /// Returns true for 2xx HTTP responses.
    pub const fn is_success(&self) -> bool {
        self.status_code >= 200 && self.status_code < 300
    }

    /// Returns the preferred syntax highlighter language for the response editor.
    pub fn editor_language(&self) -> &'static str {
        let ct = self
            .content_type
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase();
        if ct.contains("json") || looks_like_json(&self.pretty_body) {
            "json"
        } else if ct.contains("html") || ct.contains("xml") {
            "html"
        } else if ct.contains("yaml") {
            "yaml"
        } else {
            "text"
        }
    }
}

/// Lifecycle state for an in-flight or completed request execution in a workbench tab.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum RequestExecutionState {
    #[default]
    Idle,
    Loading {
        request_revision: u64,
    },
    Completed(Box<HttpResponseSnapshot>),
    Failed {
        message: String,
    },
    Cancelled,
}

/// Attempts to pretty-print JSON payloads; returns input unchanged for non-JSON text.
pub fn format_pretty_body(raw: &str, content_type: Option<&str>) -> String {
    let ct = content_type.unwrap_or("").to_ascii_lowercase();
    if (ct.contains("json") || looks_like_json(raw))
        && let Ok(val) = serde_json::from_str::<serde_json::Value>(raw)
        && let Ok(pretty) = serde_json::to_string_pretty(&val)
    {
        return pretty;
    }
    raw.to_string()
}

fn looks_like_json(text: &str) -> bool {
    let trimmed = text.trim();
    (trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']'))
}
