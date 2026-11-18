use crate::materialize::{PreparedBody, PreparedMultipartPart, PreparedRequest};

/// Supported CLI / shell targets for HTTP request code generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodegenTarget {
    Curl,
    CurlWin,
    Wget,
    PowerShell,
}

impl CodegenTarget {
    pub const ALL: [Self; 4] = [Self::Curl, Self::CurlWin, Self::Wget, Self::PowerShell];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Curl => "cURL",
            Self::CurlWin => "cURL (Win)",
            Self::Wget => "wget",
            Self::PowerShell => "PowerShell",
        }
    }
}

/// Converts a materialized [`PreparedRequest`] into a ready-to-run shell snippet.
pub fn generate_request_code(req: &PreparedRequest, target: CodegenTarget) -> String {
    match target {
        CodegenTarget::Curl => generate_curl_posix(req),
        CodegenTarget::CurlWin => generate_curl_win(req),
        CodegenTarget::Wget => generate_wget(req),
        CodegenTarget::PowerShell => generate_powershell(req),
    }
}

fn escape_posix_single(s: &str) -> String {
    s.replace('\'', "'\\''")
}

fn escape_cmd_double(s: &str) -> String {
    s.replace('"', "\\\"")
        .replace('\r', "")
        .replace('\n', "\\n")
}

fn escape_ps_single(s: &str) -> String {
    s.replace('\'', "''")
}

fn escape_ps_double(s: &str) -> String {
    s.replace('`', "``")
        .replace('"', "`\"")
        .replace('$', "`$")
}

fn generate_curl_posix(req: &PreparedRequest) -> String {
    let mut lines = vec![format!(
        "curl -X {} '{}'",
        req.method.as_str(),
        escape_posix_single(&req.url)
    )];

    for h in &req.headers {
        lines.push(format!(
            "  -H '{}: {}'",
            escape_posix_single(&h.name),
            escape_posix_single(&h.value)
        ));
    }

    match &req.body {
        PreparedBody::None => {}
        PreparedBody::Text { content } => {
            lines.push(format!("  --data-raw '{}'", escape_posix_single(content)));
        }
        PreparedBody::FormUrlEncoded { fields } => {
            for (k, v) in fields {
                lines.push(format!(
                    "  --data-urlencode '{}={}'",
                    escape_posix_single(k),
                    escape_posix_single(v)
                ));
            }
        }
        PreparedBody::Multipart { parts } => {
            for part in parts {
                match part {
                    PreparedMultipartPart::Text { name, value, .. } => {
                        lines.push(format!(
                            "  -F '{}={}'",
                            escape_posix_single(name),
                            escape_posix_single(value)
                        ));
                    }
                    PreparedMultipartPart::File { name, path, .. } => {
                        lines.push(format!(
                            "  -F '{}=@{}'",
                            escape_posix_single(name),
                            escape_posix_single(path)
                        ));
                    }
                }
            }
        }
        PreparedBody::Binary { file_path, .. } => {
            lines.push(format!(
                "  --data-binary '@{}'",
                escape_posix_single(file_path)
            ));
        }
    }

    lines.join(" \\\n")
}

fn generate_curl_win(req: &PreparedRequest) -> String {
    let mut lines = vec![format!(
        "curl.exe -X {} \"{}\"",
        req.method.as_str(),
        escape_cmd_double(&req.url)
    )];

    for h in &req.headers {
        lines.push(format!(
            "  -H \"{}: {}\"",
            escape_cmd_double(&h.name),
            escape_cmd_double(&h.value)
        ));
    }

    match &req.body {
        PreparedBody::None => {}
        PreparedBody::Text { content } => {
            lines.push(format!("  --data-raw \"{}\"", escape_cmd_double(content)));
        }
        PreparedBody::FormUrlEncoded { fields } => {
            for (k, v) in fields {
                lines.push(format!(
                    "  --data-urlencode \"{}={}\"",
                    escape_cmd_double(k),
                    escape_cmd_double(v)
                ));
            }
        }
        PreparedBody::Multipart { parts } => {
            for part in parts {
                match part {
                    PreparedMultipartPart::Text { name, value, .. } => {
                        lines.push(format!(
                            "  -F \"{}={}\"",
                            escape_cmd_double(name),
                            escape_cmd_double(value)
                        ));
                    }
                    PreparedMultipartPart::File { name, path, .. } => {
                        lines.push(format!(
                            "  -F \"{}=@{}\"",
                            escape_cmd_double(name),
                            escape_cmd_double(path)
                        ));
                    }
                }
            }
        }
        PreparedBody::Binary { file_path, .. } => {
            lines.push(format!(
                "  --data-binary \"@{}\"",
                escape_cmd_double(file_path)
            ));
        }
    }

    lines.join(" ^\n")
}

fn generate_wget(req: &PreparedRequest) -> String {
    let mut lines = vec![format!("wget --method={}", req.method.as_str())];

    for h in &req.headers {
        lines.push(format!(
            "  --header='{}: {}'",
            escape_posix_single(&h.name),
            escape_posix_single(&h.value)
        ));
    }

    match &req.body {
        PreparedBody::None => {}
        PreparedBody::Text { content } => {
            lines.push(format!("  --body-data='{}'", escape_posix_single(content)));
        }
        PreparedBody::FormUrlEncoded { fields } => {
            let encoded = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(fields.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                .finish();
            lines.push(format!("  --body-data='{}'", escape_posix_single(&encoded)));
        }
        PreparedBody::Multipart { .. } => {
            lines.push("  # Note: wget does not natively build multipart/form-data; use cURL".to_string());
        }
        PreparedBody::Binary { file_path, .. } => {
            lines.push(format!(
                "  --body-file='{}'",
                escape_posix_single(file_path)
            ));
        }
    }

    lines.push(format!("  -qO- '{}'", escape_posix_single(&req.url)));
    lines.join(" \\\n")
}

fn generate_powershell(req: &PreparedRequest) -> String {
    let mut out = Vec::new();
    let mut content_type: Option<String> = None;

    let mut header_lines = Vec::new();
    for h in &req.headers {
        if h.name.eq_ignore_ascii_case("content-type") {
            content_type = Some(h.value.clone());
        } else {
            header_lines.push(format!(
                "    \"{}\" = \"{}\"",
                escape_ps_double(&h.name),
                escape_ps_double(&h.value)
            ));
        }
    }

    let has_headers = !header_lines.is_empty();
    if has_headers {
        out.push("$headers = @{".to_string());
        out.extend(header_lines);
        out.push("}".to_string());
    }

    let mut has_body = false;
    match &req.body {
        PreparedBody::None => {}
        PreparedBody::Text { content } => {
            has_body = true;
            if content.contains('\n') {
                out.push(format!("$body = @'\n{content}\n'@"));
            } else {
                out.push(format!("$body = '{}'", escape_ps_single(content)));
            }
        }
        PreparedBody::FormUrlEncoded { fields } => {
            has_body = true;
            out.push("$body = @{".to_string());
            for (k, v) in fields {
                out.push(format!(
                    "    \"{}\" = \"{}\"",
                    escape_ps_double(k),
                    escape_ps_double(v)
                ));
            }
            out.push("}".to_string());
        }
        PreparedBody::Multipart { parts } => {
            has_body = true;
            out.push("$body = @{".to_string());
            for part in parts {
                match part {
                    PreparedMultipartPart::Text { name, value, .. } => {
                        out.push(format!(
                            "    \"{}\" = \"{}\"",
                            escape_ps_double(name),
                            escape_ps_double(value)
                        ));
                    }
                    PreparedMultipartPart::File { name, path, .. } => {
                        out.push(format!(
                            "    \"{}\" = Get-Item -Path '{}'",
                            escape_ps_double(name),
                            escape_ps_single(path)
                        ));
                    }
                }
            }
            out.push("}".to_string());
        }
        PreparedBody::Binary { file_path, .. } => {
            has_body = true;
            out.push(format!(
                "$body = [System.IO.File]::ReadAllBytes('{}')",
                escape_ps_single(file_path)
            ));
        }
    }

    let mut invoke = format!(
        "Invoke-RestMethod -Uri '{}' -Method {}",
        escape_ps_single(&req.url),
        req.method.as_str()
    );
    if has_headers {
        invoke.push_str(" -Headers $headers");
    }
    if let Some(ct) = content_type {
        invoke.push_str(&format!(" -ContentType '{}'", escape_ps_single(&ct)));
    }
    if has_body {
        invoke.push_str(" -Body $body");
    }
    out.push(invoke);

    out.join("\n")
}

#[cfg(test)]
mod tests {
    use crate::{auth::ResolvedAuth, materialize::PreparedHeader, model::{HttpMethod, RequestSettings}};

    use super::*;

    fn sample_request() -> PreparedRequest {
        PreparedRequest {
            request_id: "req-1".to_string(),
            name: "Create User".to_string(),
            method: HttpMethod::Post,
            url: "https://api.example.com/users?active=true".to_string(),
            headers: vec![
                PreparedHeader {
                    name: "Authorization".to_string(),
                    value: "Bearer tok-123".to_string(),
                    secret: true,
                },
                PreparedHeader {
                    name: "Content-Type".to_string(),
                    value: "application/json".to_string(),
                    secret: false,
                },
            ],
            body: PreparedBody::Text {
                content: r#"{"name":"Ada"}"#.to_string(),
            },
            settings: RequestSettings::default(),
            resolved_auth: ResolvedAuth {
                effective: crate::model::EffectiveAuth::None,
                source: None,
                chain: Vec::new(),
            },
            unresolved_variables: Vec::new(),
        }
    }

    #[test]
    fn generates_curl_win_wget_and_powershell_snippets() {
        let req = sample_request();

        let curl = generate_request_code(&req, CodegenTarget::Curl);
        assert!(curl.contains("curl -X POST 'https://api.example.com/users?active=true'"));
        assert!(curl.contains("-H 'Authorization: Bearer tok-123'"));
        assert!(curl.contains("--data-raw '{\"name\":\"Ada\"}'"));

        let curl_win = generate_request_code(&req, CodegenTarget::CurlWin);
        assert!(curl_win.contains("curl.exe -X POST \"https://api.example.com/users?active=true\""));
        assert!(curl_win.contains("^\n"));

        let wget = generate_request_code(&req, CodegenTarget::Wget);
        assert!(wget.contains("wget --method=POST"));
        assert!(wget.contains("--body-data='{\"name\":\"Ada\"}'"));

        let ps = generate_request_code(&req, CodegenTarget::PowerShell);
        assert!(ps.contains("Invoke-RestMethod -Uri 'https://api.example.com/users?active=true' -Method POST"));
        assert!(ps.contains("-ContentType 'application/json'"));
        assert!(ps.contains("-Body $body"));
    }
}
