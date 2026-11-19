use crate::{
    error::CoreError,
    materialize::{PreparedBody, PreparedMultipartPart, PreparedRequest, extract_url_query_params},
    model::{
        AuthConfig, HttpMethod, HttpRequestSpec, KeyValueRow, MultipartField, MultipartFieldType,
        RawBodyFormat, RequestBody,
    },
};

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
        "$response = Invoke-WebRequest -Uri '{}' -Method {} -UseBasicParsing",
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
    out.push("$response.Content".to_string());

    out.join("\n")
}

/// Parses a cURL, wget, or PowerShell (`Invoke-WebRequest` / `Invoke-RestMethod`) script
/// into an [`HttpRequestSpec`].
pub fn parse_request_script(input: &str) -> Result<HttpRequestSpec, CoreError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(CoreError::Validation {
            reason: "Script input is empty".to_string(),
        });
    }

    if looks_like_powershell(trimmed) {
        return parse_powershell_script(trimmed);
    }

    let joined = join_shell_continuations(trimmed);
    let tokens = tokenize_shell(&joined);
    if tokens.is_empty() {
        return Err(CoreError::Validation {
            reason: "Could not parse any command tokens".to_string(),
        });
    }

    let first = tokens[0].to_ascii_lowercase();
    if first == "wget" || first == "wget.exe" || first.ends_with("/wget") {
        parse_wget_tokens(&tokens[1..])
    } else if first == "curl" || first == "curl.exe" || first.ends_with("/curl") {
        parse_curl_tokens(&tokens[1..])
    } else {
        parse_curl_tokens(&tokens)
    }
}

fn looks_like_powershell(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    lower.contains("invoke-webrequest")
        || lower.contains("invoke-restmethod")
        || lower.starts_with("$headers")
        || lower.starts_with("$body")
        || lower.starts_with("$response")
        || lower.contains(" -uri ")
}

fn join_shell_continuations(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let lines: Vec<&str> = input.lines().collect();
    for (idx, line) in lines.iter().enumerate() {
        let trimmed_end = line.trim_end();
        let is_last = idx + 1 == lines.len();
        if !is_last
            && (trimmed_end.ends_with('\\') || trimmed_end.ends_with('^'))
        {
            out.push_str(&trimmed_end[..trimmed_end.len() - 1]);
            out.push(' ');
        } else {
            out.push_str(line);
            if !is_last {
                out.push('\n');
            }
        }
    }
    out
}

fn tokenize_shell(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        if ch.is_whitespace() {
            if in_token {
                tokens.push(std::mem::take(&mut current));
                in_token = false;
            }
            i += 1;
        } else if ch == '\'' {
            in_token = true;
            i += 1;
            while i < chars.len() && chars[i] != '\'' {
                current.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
        } else if ch == '"' {
            in_token = true;
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    let next = chars[i + 1];
                    if matches!(next, '"' | '\\' | '$' | '`') {
                        current.push(next);
                        i += 2;
                        continue;
                    } else if next == 'n' {
                        current.push('\n');
                        i += 2;
                        continue;
                    }
                }
                current.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
        } else if ch == '\\' && i + 1 < chars.len() {
            in_token = true;
            current.push(chars[i + 1]);
            i += 2;
        } else {
            in_token = true;
            current.push(ch);
            i += 1;
        }
    }

    if in_token {
        tokens.push(current);
    }
    tokens
}

fn finalize_parsed_spec(
    method: Option<HttpMethod>,
    raw_url: Option<String>,
    headers: Vec<KeyValueRow>,
    auth: AuthConfig,
    body: RequestBody,
) -> Result<HttpRequestSpec, CoreError> {
    let raw_url = raw_url
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty())
        .ok_or_else(|| CoreError::Validation {
            reason: "No URL found in script".to_string(),
        })?;

    let (clean_url, params) = match extract_url_query_params(&raw_url) {
        Some((base, pairs)) => (
            base,
            pairs
                .into_iter()
                .map(|(k, v)| KeyValueRow::new(k, v))
                .collect(),
        ),
        None => (raw_url, Vec::new()),
    };
    let has_body = !matches!(body, RequestBody::None);
    let resolved_method = method.unwrap_or(if has_body {
        HttpMethod::Post
    } else {
        HttpMethod::Get
    });

    Ok(HttpRequestSpec {
        method: resolved_method,
        url: clean_url,
        params,
        headers,
        auth,
        body,
        ..Default::default()
    })
}

fn classify_text_body(content: String, headers: &[KeyValueRow]) -> RequestBody {
    let ct = headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-type"))
        .map(|h| h.value.to_ascii_lowercase())
        .unwrap_or_default();
    let trimmed = content.trim();
    if ct.contains("json")
        || ((trimmed.starts_with('{') || trimmed.starts_with('['))
            && serde_json::from_str::<serde_json::Value>(trimmed).is_ok())
    {
        RequestBody::Json { content }
    } else if ct.contains("xml") || trimmed.starts_with('<') {
        RequestBody::Raw {
            raw_format: RawBodyFormat::Xml,
            content,
        }
    } else if ct.contains("html") {
        RequestBody::Raw {
            raw_format: RawBodyFormat::Html,
            content,
        }
    } else {
        RequestBody::Raw {
            raw_format: RawBodyFormat::Text,
            content,
        }
    }
}

fn parse_curl_tokens(tokens: &[String]) -> Result<HttpRequestSpec, CoreError> {
    let mut method: Option<HttpMethod> = None;
    let mut url: Option<String> = None;
    let mut headers: Vec<KeyValueRow> = Vec::new();
    let mut auth = AuthConfig::Inherit;
    let mut raw_data_parts: Vec<String> = Vec::new();
    let mut urlencoded_fields: Vec<KeyValueRow> = Vec::new();
    let mut multipart_fields: Vec<MultipartField> = Vec::new();
    let mut binary_path: Option<String> = None;

    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i].as_str();
        match tok {
            "-X" | "--request" => {
                if let Some(m) = tokens.get(i + 1).and_then(|v| HttpMethod::parse(v)) {
                    method = Some(m);
                }
                i += 2;
            }
            "--url" => {
                if let Some(u) = tokens.get(i + 1) {
                    url = Some(u.clone());
                }
                i += 2;
            }
            "-G" | "--get" => {
                method = Some(HttpMethod::Get);
                i += 1;
            }
            "-H" | "--header" => {
                if let Some(h) = tokens.get(i + 1)
                    && let Some((k, v)) = h.split_once(':')
                {
                    headers.push(KeyValueRow::new(k.trim(), v.trim()));
                }
                i += 2;
            }
            "-A" | "--user-agent" => {
                if let Some(ua) = tokens.get(i + 1) {
                    headers.push(KeyValueRow::new("User-Agent", ua));
                }
                i += 2;
            }
            "-u" | "--user" => {
                if let Some(creds) = tokens.get(i + 1) {
                    let (u, p) = creds.split_once(':').unwrap_or((creds.as_str(), ""));
                    auth = AuthConfig::Basic {
                        username: u.to_string(),
                        password: p.to_string(),
                    };
                }
                i += 2;
            }
            "-d" | "--data" | "--data-raw" | "--data-ascii" => {
                if let Some(d) = tokens.get(i + 1) {
                    raw_data_parts.push(d.clone());
                }
                i += 2;
            }
            "--data-urlencode" => {
                if let Some(d) = tokens.get(i + 1) {
                    let (k, v) = d.split_once('=').unwrap_or((d.as_str(), ""));
                    urlencoded_fields.push(KeyValueRow::new(k, v));
                }
                i += 2;
            }
            "--data-binary" => {
                if let Some(d) = tokens.get(i + 1) {
                    if let Some(path) = d.strip_prefix('@') {
                        binary_path = Some(path.to_string());
                    } else {
                        raw_data_parts.push(d.clone());
                    }
                }
                i += 2;
            }
            "-F" | "--form" => {
                if let Some(d) = tokens.get(i + 1)
                    && let Some((k, v)) = d.split_once('=')
                {
                    if let Some(path) = v.strip_prefix('@') {
                        multipart_fields.push(MultipartField {
                            name: k.to_string(),
                            field_type: MultipartFieldType::File,
                            value: path.to_string(),
                            content_type: None,
                            file_name: None,
                            enabled: true,
                            description: String::new(),
                        });
                    } else {
                        multipart_fields.push(MultipartField::text(k, v));
                    }
                }
                i += 2;
            }
            _ if tok.starts_with("-X") && tok.len() > 2 => {
                if let Some(m) = HttpMethod::parse(&tok[2..]) {
                    method = Some(m);
                }
                i += 1;
            }
            _ if tok.starts_with('-') => {
                i += 1;
            }
            _ => {
                if url.is_none() {
                    url = Some(tok.to_string());
                }
                i += 1;
            }
        }
    }

    let body = if let Some(file_path) = binary_path {
        let ct = headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("content-type"))
            .map(|h| h.value.clone());
        RequestBody::Binary {
            file_path,
            content_type: ct,
        }
    } else if !multipart_fields.is_empty() {
        RequestBody::Multipart {
            fields: multipart_fields,
        }
    } else if !urlencoded_fields.is_empty() {
        RequestBody::FormUrlEncoded {
            fields: urlencoded_fields,
        }
    } else if !raw_data_parts.is_empty() {
        let combined = raw_data_parts.join("&");
        let ct = headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("content-type"))
            .map(|h| h.value.to_ascii_lowercase())
            .unwrap_or_default();
        if ct.contains("application/x-www-form-urlencoded") && combined.contains('=') {
            let fields = url::form_urlencoded::parse(combined.as_bytes())
                .map(|(k, v)| KeyValueRow::new(k.into_owned(), v.into_owned()))
                .collect();
            RequestBody::FormUrlEncoded { fields }
        } else {
            classify_text_body(combined, &headers)
        }
    } else {
        RequestBody::None
    };

    finalize_parsed_spec(method, url, headers, auth, body)
}

fn parse_wget_tokens(tokens: &[String]) -> Result<HttpRequestSpec, CoreError> {
    let mut method: Option<HttpMethod> = None;
    let mut url: Option<String> = None;
    let mut headers: Vec<KeyValueRow> = Vec::new();
    let mut body_data: Option<String> = None;
    let mut body_file: Option<String> = None;

    let mut i = 0;
    while i < tokens.len() {
        let tok = tokens[i].as_str();
        if let Some(m) = tok.strip_prefix("--method=") {
            method = HttpMethod::parse(m);
            i += 1;
        } else if tok == "--method" {
            method = tokens.get(i + 1).and_then(|v| HttpMethod::parse(v));
            i += 2;
        } else if let Some(h) = tok.strip_prefix("--header=") {
            if let Some((k, v)) = h.split_once(':') {
                headers.push(KeyValueRow::new(k.trim(), v.trim()));
            }
            i += 1;
        } else if tok == "--header" {
            if let Some(h) = tokens.get(i + 1)
                && let Some((k, v)) = h.split_once(':')
            {
                headers.push(KeyValueRow::new(k.trim(), v.trim()));
            }
            i += 2;
        } else if let Some(d) = tok.strip_prefix("--body-data=") {
            body_data = Some(d.to_string());
            i += 1;
        } else if tok == "--body-data" {
            body_data = tokens.get(i + 1).cloned();
            i += 2;
        } else if let Some(d) = tok.strip_prefix("--post-data=") {
            body_data = Some(d.to_string());
            if method.is_none() {
                method = Some(HttpMethod::Post);
            }
            i += 1;
        } else if let Some(f) = tok.strip_prefix("--body-file=") {
            body_file = Some(f.to_string());
            i += 1;
        } else if tok == "--body-file" {
            body_file = tokens.get(i + 1).cloned();
            i += 2;
        } else if tok == "-O" || tok == "--output-document" {
            i += 2;
        } else if tok.starts_with('-') {
            i += 1;
        } else {
            if url.is_none() {
                url = Some(tok.to_string());
            }
            i += 1;
        }
    }

    let body = if let Some(file_path) = body_file {
        RequestBody::Binary {
            file_path,
            content_type: None,
        }
    } else if let Some(data) = body_data {
        let ct = headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("content-type"))
            .map(|h| h.value.to_ascii_lowercase())
            .unwrap_or_default();
        if ct.contains("application/x-www-form-urlencoded") && data.contains('=') {
            let fields = url::form_urlencoded::parse(data.as_bytes())
                .map(|(k, v)| KeyValueRow::new(k.into_owned(), v.into_owned()))
                .collect();
            RequestBody::FormUrlEncoded { fields }
        } else {
            classify_text_body(data, &headers)
        }
    } else {
        RequestBody::None
    };

    finalize_parsed_spec(method, url, headers, AuthConfig::Inherit, body)
}

fn unquote_ps(s: &str) -> String {
    let t = s.trim();
    if t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2 {
        t[1..t.len() - 1].replace("''", "'")
    } else if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 {
        t[1..t.len() - 1]
            .replace("`\"", "\"")
            .replace("`$", "$")
            .replace("``", "`")
    } else {
        t.to_string()
    }
}

fn parse_ps_hashtable(block: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    for line in block.lines() {
        let l = line.trim().trim_end_matches(';');
        if l.is_empty() || l.starts_with('#') || l == "@{" || l == "}" {
            continue;
        }
        if let Some((k, v)) = l.split_once('=') {
            entries.push((unquote_ps(k), unquote_ps(v)));
        }
    }
    entries
}

fn parse_powershell_script(input: &str) -> Result<HttpRequestSpec, CoreError> {
    let mut headers: Vec<KeyValueRow> = Vec::new();
    let mut body = RequestBody::None;

    // Extract $headers = @{ ... }
    if let Some(start) = input.find("$headers")
        && let Some(open) = input[start..].find("@{")
    {
        let after_open = &input[start + open + 2..];
        if let Some(close) = after_open.find('}') {
            for (k, v) in parse_ps_hashtable(&after_open[..close]) {
                headers.push(KeyValueRow::new(k, v));
            }
        }
    }

    // Extract $body = ...
    if let Some(start) = input.find("$body")
        && let Some(eq_pos) = input[start..].find('=')
    {
        let rhs = input[start + eq_pos + 1..].trim_start();
        if let Some(after_here) = rhs.strip_prefix("@'") {
            let content = after_here
                .trim_start_matches('\r')
                .trim_start_matches('\n');
            if let Some(end) = content.find("\n'@").or_else(|| content.find("'@")) {
                body = classify_text_body(content[..end].trim_end_matches('\r').to_string(), &headers);
            }
        } else if let Some(after_hash) = rhs.strip_prefix("@{") {
            if let Some(close) = after_hash.find('}') {
                let pairs = parse_ps_hashtable(&after_hash[..close]);
                let is_multipart = pairs.iter().any(|(_, v)| v.contains("Get-Item"));
                if is_multipart {
                    let fields = pairs
                        .into_iter()
                        .map(|(k, v)| {
                            if let Some(idx) = v.find("Get-Item") {
                                let rest = v[idx..].split("-Path").nth(1).unwrap_or("");
                                MultipartField {
                                    name: k,
                                    field_type: MultipartFieldType::File,
                                    value: unquote_ps(rest),
                                    content_type: None,
                                    file_name: None,
                                    enabled: true,
                                    description: String::new(),
                                }
                            } else {
                                MultipartField::text(k, v)
                            }
                        })
                        .collect();
                    body = RequestBody::Multipart { fields };
                } else {
                    let fields = pairs
                        .into_iter()
                        .map(|(k, v)| KeyValueRow::new(k, v))
                        .collect();
                    body = RequestBody::FormUrlEncoded { fields };
                }
            }
        } else if let Some(after_bytes) = rhs.strip_prefix("[System.IO.File]::ReadAllBytes(") {
            if let Some(close) = after_bytes.find(')') {
                body = RequestBody::Binary {
                    file_path: unquote_ps(&after_bytes[..close]),
                    content_type: None,
                };
            }
        } else if let Some(line) = rhs.lines().next() {
            body = classify_text_body(unquote_ps(line), &headers);
        }
    }

    // Parse Invoke-WebRequest / Invoke-RestMethod line
    let invoke_line = input
        .lines()
        .find(|l| {
            let low = l.to_ascii_lowercase();
            low.contains("invoke-webrequest")
                || low.contains("invoke-restmethod")
                || low.contains("iwr ")
                || low.contains("irm ")
        })
        .unwrap_or(input);

    let tokens = tokenize_shell(invoke_line);
    let mut url: Option<String> = None;
    let mut method: Option<HttpMethod> = None;

    let mut i = 0;
    while i < tokens.len() {
        let low = tokens[i].to_ascii_lowercase();
        match low.as_str() {
            "-uri" => {
                url = tokens.get(i + 1).cloned();
                i += 2;
            }
            "-method" => {
                method = tokens.get(i + 1).and_then(|m| HttpMethod::parse(m));
                i += 2;
            }
            "-contenttype" => {
                if let Some(ct) = tokens.get(i + 1)
                    && !headers
                        .iter()
                        .any(|h| h.name.eq_ignore_ascii_case("content-type"))
                {
                    headers.push(KeyValueRow::new("Content-Type", ct));
                }
                i += 2;
            }
            "-body" => {
                if let Some(val) = tokens.get(i + 1)
                    && val != "$body"
                    && matches!(body, RequestBody::None)
                {
                    body = classify_text_body(val.clone(), &headers);
                }
                i += 2;
            }
            _ => i += 1,
        }
    }

    // Re-classify text body if Content-Type arrived via -ContentType after $body
    if let RequestBody::Raw { content, .. } = body {
        body = classify_text_body(content, &headers);
    }

    finalize_parsed_spec(method, url, headers, AuthConfig::Inherit, body)
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
        assert!(ps.contains("Invoke-WebRequest -Uri 'https://api.example.com/users?active=true' -Method POST -UseBasicParsing"));
        assert!(ps.contains("-ContentType 'application/json'"));
        assert!(ps.contains("-Body $body"));
        assert!(ps.contains("$response.Content"));
    }

    #[test]
    fn parses_curl_curl_win_wget_and_powershell_scripts() {
        let req = sample_request();

        for target in CodegenTarget::ALL {
            let snippet = generate_request_code(&req, target);
            let parsed = parse_request_script(&snippet).expect("parse snippet");
            assert_eq!(parsed.method, HttpMethod::Post, "target {target:?}");
            assert_eq!(parsed.url, "https://api.example.com/users", "target {target:?}");
            assert_eq!(
                parsed.params,
                vec![KeyValueRow::new("active", "true")],
                "target {target:?}"
            );
            assert!(
                parsed
                    .headers
                    .iter()
                    .any(|h| h.name == "Authorization" && h.value == "Bearer tok-123"),
                "target {target:?}"
            );
            assert_eq!(
                parsed.body,
                RequestBody::Json {
                    content: r#"{"name":"Ada"}"#.to_string()
                },
                "target {target:?}"
            );
        }
    }
}
