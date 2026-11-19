use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::response::ResponseHeader;

const MAX_EXAMPLE_LEN: usize = 120;

/// Saved example response attached to an HTTP request document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ResponseExample {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub status_code: u16,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<ResponseHeader>,
    #[serde(default)]
    pub body: String,
}

impl ResponseExample {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        status_code: u16,
        reason: impl Into<String>,
        content_type: Option<String>,
        headers: Vec<ResponseHeader>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            status_code,
            reason: reason.into(),
            content_type,
            headers,
            body: body.into(),
        }
    }
}

/// Tree node representing an extracted response field/entry and its editable description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ResponseMetaField {
    pub name: String,
    #[serde(default, rename = "type", skip_serializing_if = "String::is_empty")]
    pub field_type: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub example: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<ResponseMetaField>,
}

impl ResponseMetaField {
    pub fn new(name: impl Into<String>, field_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            field_type: field_type.into(),
            example: String::new(),
            description: String::new(),
            children: Vec::new(),
        }
    }

    pub fn with_example(mut self, example: impl Into<String>) -> Self {
        self.example = example.into();
        self
    }

    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    pub fn with_children(mut self, children: Vec<ResponseMetaField>) -> Self {
        self.children = children;
        self
    }
}

/// Extracts a meta-info field tree from a JSON or XML response body.
///
/// Returns `None` if the response is neither valid JSON nor well-formed XML
/// (for example, HTML, plain text, YAML, or binary content).
pub fn extract_response_meta(
    body: &str,
    content_type: Option<&str>,
) -> Option<Vec<ResponseMetaField>> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }

    let ct = content_type.unwrap_or("").to_ascii_lowercase();

    // Explicitly skip HTML even though it starts with '<'.
    if ct.contains("html") && !ct.contains("xhtml") {
        return None;
    }

    if ct.contains("json") {
        let val = serde_json::from_str::<serde_json::Value>(trimmed).ok()?;
        return Some(extract_json_root(&val));
    }

    if ct.contains("xml") {
        return extract_xml_root(trimmed);
    }

    // Without an explicit JSON/XML Content-Type, try JSON first, then strict XML.
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed)
    {
        return Some(extract_json_root(&val));
    }

    if trimmed.starts_with('<') && !looks_like_html_doc(trimmed) {
        return extract_xml_root(trimmed);
    }

    None
}

use crate::model::{MultipartFieldType, RawBodyFormat, RequestBody};

/// Extracts meta-info from `body` and merges it into `existing`.
///
/// Returns `true` if `body` was JSON or XML and extraction succeeded, or `false` if
/// extraction was skipped because the response is not JSON or XML.
pub fn extract_and_merge_response_meta(
    existing: &mut Vec<ResponseMetaField>,
    body: &str,
    content_type: Option<&str>,
) -> bool {
    let Some(incoming) = extract_response_meta(body, content_type) else {
        return false;
    };
    merge_response_meta(existing, incoming);
    true
}

/// Extracts structured field metadata from an HTTP [`RequestBody`].
///
/// Supports JSON, XML (in `Raw`), `application/x-www-form-urlencoded`, and
/// `multipart/form-data` payloads. Returns `None` for empty, binary, or
/// unparseable payloads.
pub fn extract_request_body_meta(body: &RequestBody) -> Option<Vec<ResponseMetaField>> {
    match body {
        RequestBody::Json { content } => extract_response_meta(content, Some("application/json")),
        RequestBody::Raw {
            content,
            raw_format,
        } => {
            let ct = match raw_format {
                RawBodyFormat::Xml => Some("application/xml"),
                RawBodyFormat::Html => Some("text/html"),
                RawBodyFormat::Text | RawBodyFormat::Javascript | RawBodyFormat::Yaml => None,
            };
            extract_response_meta(content, ct)
        }
        RequestBody::FormUrlEncoded { fields } => {
            let mut out = Vec::new();
            for row in fields {
                if !row.enabled || row.name.trim().is_empty() {
                    continue;
                }
                let mut f = ResponseMetaField::new(row.name.trim(), "string")
                    .with_example(truncate_example(&row.value));
                if !row.description.trim().is_empty() {
                    f = f.with_description(row.description.trim());
                }
                out.push(f);
            }
            if out.is_empty() { None } else { Some(out) }
        }
        RequestBody::Multipart { fields } => {
            let mut out = Vec::new();
            for field in fields {
                if !field.enabled || field.name.trim().is_empty() {
                    continue;
                }
                let ty = match field.field_type {
                    MultipartFieldType::File => "file",
                    MultipartFieldType::Text => "string",
                };
                let example = if field.value.is_empty() {
                    field.file_name.as_deref().unwrap_or("")
                } else {
                    field.value.as_str()
                };
                let mut f =
                    ResponseMetaField::new(field.name.trim(), ty).with_example(truncate_example(example));
                if !field.description.trim().is_empty() {
                    f = f.with_description(field.description.trim());
                }
                out.push(f);
            }
            if out.is_empty() { None } else { Some(out) }
        }
        RequestBody::None | RequestBody::Binary { .. } => None,
    }
}

/// Extracts meta-info from a [`RequestBody`] and merges it into `existing`.
pub fn extract_and_merge_request_body_meta(
    existing: &mut Vec<ResponseMetaField>,
    body: &RequestBody,
) -> bool {
    let Some(incoming) = extract_request_body_meta(body) else {
        return false;
    };
    merge_response_meta(existing, incoming);
    true
}

/// Merges `incoming` fields into `existing`, preserving existing descriptions
/// and combining types and nested children by field name.
pub fn merge_response_meta(
    existing: &mut Vec<ResponseMetaField>,
    incoming: Vec<ResponseMetaField>,
) {
    for inc in incoming {
        if let Some(cur) = existing.iter_mut().find(|f| f.name == inc.name) {
            cur.field_type = merge_field_types(&cur.field_type, &inc.field_type);
            if cur.example.is_empty() && !inc.example.is_empty() {
                cur.example = inc.example;
            }
            if cur.description.is_empty() && !inc.description.is_empty() {
                cur.description = inc.description;
            }
            if !inc.children.is_empty() {
                merge_response_meta(&mut cur.children, inc.children);
            }
        } else {
            existing.push(inc);
        }
    }
}

/// Converts a JSON Schema object (with optional `#/components/schemas/` lookup) into a tree of
/// [`ResponseMetaField`] nodes, preserving field types, examples, and descriptions.
pub(crate) fn json_schema_to_meta_fields(
    raw_schema: &serde_json::Value,
    components: Option<&serde_json::Map<String, serde_json::Value>>,
    depth: usize,
) -> Vec<ResponseMetaField> {
    if depth > 6 {
        return Vec::new();
    }
    let schema = resolve_schema_ref(raw_schema, components);
    let ty = schema.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if ty == "array"
        && let Some(items) = schema.get("items")
    {
        let children = json_schema_to_meta_fields(items, components, depth + 1);
        let desc = schema
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim();
        let mut root = ResponseMetaField::new("[]", "array").with_children(children);
        if !desc.is_empty() {
            root = root.with_description(desc);
        }
        return vec![root];
    }

    let Some(props) = schema.get("properties").and_then(|v| v.as_object()) else {
        return Vec::new();
    };

    props
        .iter()
        .map(|(name, raw_prop)| {
            let prop = resolve_schema_ref(raw_prop, components);
            let prop_ty = prop.get("type").and_then(|v| v.as_str()).unwrap_or_else(|| {
                if prop.get("properties").is_some() {
                    "object"
                } else if prop.get("items").is_some() {
                    "array"
                } else {
                    "string"
                }
            });
            let desc = prop
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .trim();
            let example = prop
                .get("example")
                .or_else(|| prop.get("default"))
                .map(schema_example_preview)
                .unwrap_or_default();
            let children = if prop_ty == "array" {
                prop.get("items")
                    .map(|items| json_schema_to_meta_fields(items, components, depth + 1))
                    .unwrap_or_default()
            } else if prop_ty == "object" || prop.get("properties").is_some() {
                json_schema_to_meta_fields(prop, components, depth + 1)
            } else {
                Vec::new()
            };

            let mut field = ResponseMetaField::new(name, prop_ty).with_children(children);
            if !example.is_empty() {
                field = field.with_example(example);
            }
            if !desc.is_empty() {
                field = field.with_description(desc);
            }
            field
        })
        .collect()
}

fn resolve_schema_ref<'a>(
    val: &'a serde_json::Value,
    components: Option<&'a serde_json::Map<String, serde_json::Value>>,
) -> &'a serde_json::Value {
    if let Some(ref_str) = val.get("$ref").and_then(|v| v.as_str()) {
        let name = ref_str
            .strip_prefix("#/components/schemas/")
            .or_else(|| ref_str.strip_prefix("#/definitions/"));
        if let Some(name) = name
            && let Some(target) = components.and_then(|c| c.get(name))
        {
            return target;
        }
    }
    val
}

fn schema_example_preview(val: &serde_json::Value) -> String {
    let raw = match val {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "null".to_string(),
        other => other.to_string(),
    };
    truncate_example(&raw)
}

fn merge_field_types(existing: &str, incoming: &str) -> String {
    if existing.is_empty() {
        return incoming.to_string();
    }
    if incoming.is_empty() || existing == incoming {
        return existing.to_string();
    }
    if existing == "null" && incoming != "null" {
        return incoming.to_string();
    }
    if incoming == "null" {
        return existing.to_string();
    }
    if existing.split(" | ").any(|part| part == incoming) {
        return existing.to_string();
    }
    format!("{existing} | {incoming}")
}

fn truncate_example(s: &str) -> String {
    let single_line: String = s
        .chars()
        .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
        .collect();
    let trimmed = single_line.trim();
    if trimmed.chars().count() <= MAX_EXAMPLE_LEN {
        trimmed.to_string()
    } else {
        let mut out: String = trimmed.chars().take(MAX_EXAMPLE_LEN - 1).collect();
        out.push('…');
        out
    }
}

fn extract_json_root(value: &serde_json::Value) -> Vec<ResponseMetaField> {
    match value {
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(k, v)| extract_json_field(k, v))
            .collect(),
        _ => vec![extract_json_field("(root)", value)],
    }
}

fn extract_json_field(name: &str, value: &serde_json::Value) -> ResponseMetaField {
    match value {
        serde_json::Value::Null => ResponseMetaField::new(name, "null").with_example("null"),
        serde_json::Value::Bool(b) => {
            ResponseMetaField::new(name, "boolean").with_example(b.to_string())
        }
        serde_json::Value::Number(n) => {
            let ty = if n.is_i64() || n.is_u64() {
                "integer"
            } else {
                "number"
            };
            ResponseMetaField::new(name, ty).with_example(n.to_string())
        }
        serde_json::Value::String(s) => {
            ResponseMetaField::new(name, "string").with_example(truncate_example(s))
        }
        serde_json::Value::Array(items) => {
            if items.is_empty() {
                return ResponseMetaField::new(name, "array").with_example("[]");
            }

            let mut elem_types = Vec::new();
            let mut merged_children = Vec::new();
            let mut first_primitive_example = String::new();

            for item in items {
                match item {
                    serde_json::Value::Object(map) => {
                        if !elem_types.contains(&"object") {
                            elem_types.push("object");
                        }
                        let fields: Vec<ResponseMetaField> = map
                            .iter()
                            .map(|(k, v)| extract_json_field(k, v))
                            .collect();
                        merge_response_meta(&mut merged_children, fields);
                    }
                    serde_json::Value::Array(_) => {
                        if !elem_types.contains(&"array") {
                            elem_types.push("array");
                        }
                        let sub = extract_json_field("[]", item);
                        merge_response_meta(&mut merged_children, vec![sub]);
                    }
                    other => {
                        let sub = extract_json_field("", other);
                        let ty_static = match other {
                            serde_json::Value::Null => "null",
                            serde_json::Value::Bool(_) => "boolean",
                            serde_json::Value::Number(n) => {
                                if n.is_i64() || n.is_u64() {
                                    "integer"
                                } else {
                                    "number"
                                }
                            }
                            serde_json::Value::String(_) => "string",
                            _ => "any",
                        };
                        if !elem_types.contains(&ty_static) {
                            elem_types.push(ty_static);
                        }
                        if first_primitive_example.is_empty() && !sub.example.is_empty() {
                            first_primitive_example = sub.example;
                        }
                    }
                }
            }

            let inner = if elem_types.is_empty() {
                "any".to_string()
            } else {
                elem_types.join(" | ")
            };
            let mut field = ResponseMetaField::new(name, format!("array<{inner}>"));
            if !merged_children.is_empty() {
                field.children = merged_children;
            } else if !first_primitive_example.is_empty() {
                field.example = truncate_example(&format!("[{first_primitive_example}, …]"));
            }
            field
        }
        serde_json::Value::Object(map) => {
            let children = map
                .iter()
                .map(|(k, v)| extract_json_field(k, v))
                .collect();
            ResponseMetaField::new(name, "object").with_children(children)
        }
    }
}

fn looks_like_html_doc(trimmed: &str) -> bool {
    let lower_prefix: String = trimmed.chars().take(64).collect::<String>().to_ascii_lowercase();
    lower_prefix.starts_with("<!doctype html") || lower_prefix.starts_with("<html")
}

#[derive(Debug)]
struct XmlElement {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<XmlElement>,
    text: String,
}

fn extract_xml_root(input: &str) -> Option<Vec<ResponseMetaField>> {
    let mut parser = XmlParser {
        bytes: input.as_bytes(),
        pos: 0,
    };
    let root = parser.parse_document()?;
    Some(vec![xml_element_to_meta(&root, false)])
}

fn xml_element_to_meta(el: &XmlElement, is_array: bool) -> ResponseMetaField {
    let mut children = Vec::new();

    for (attr_name, attr_val) in &el.attrs {
        children.push(
            ResponseMetaField::new(format!("@{attr_name}"), "string")
                .with_example(truncate_example(attr_val)),
        );
    }

    // Group child elements by tag name, preserving first-seen order.
    let mut order: Vec<String> = Vec::new();
    let mut grouped: BTreeMap<String, Vec<&XmlElement>> = BTreeMap::new();
    for child in &el.children {
        if !grouped.contains_key(&child.name) {
            order.push(child.name.clone());
        }
        grouped.entry(child.name.clone()).or_default().push(child);
    }

    for tag in order {
        let group = &grouped[&tag];
        let repeated = group.len() > 1;
        let mut merged: Option<ResponseMetaField> = None;
        for item in group {
            let next = xml_element_to_meta(item, repeated);
            if let Some(ref mut cur) = merged {
                let mut cur_vec = vec![cur.clone()];
                merge_response_meta(&mut cur_vec, vec![next]);
                *cur = cur_vec.into_iter().next().unwrap();
            } else {
                merged = Some(next);
            }
        }
        if let Some(field) = merged {
            children.push(field);
        }
    }

    let trimmed_text = el.text.trim();
    if !children.is_empty() && !trimmed_text.is_empty() {
        children.push(
            ResponseMetaField::new("#text", "string")
                .with_example(truncate_example(trimmed_text)),
        );
    }

    let has_children = !children.is_empty();
    let field_type = match (is_array, has_children) {
        (true, true) => "array<object>",
        (true, false) => "array<string>",
        (false, true) => "object",
        (false, false) => "string",
    };

    let mut field = ResponseMetaField::new(&el.name, field_type);
    if has_children {
        field.children = children;
    } else if !trimmed_text.is_empty() {
        field.example = truncate_example(trimmed_text);
    }
    field
}

struct XmlParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> XmlParser<'a> {
    fn remaining(&self) -> &str {
        std::str::from_utf8(&self.bytes[self.pos..]).unwrap_or("")
    }

    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn starts_with(&self, pat: &str) -> bool {
        self.remaining().starts_with(pat)
    }

    fn skip_prolog(&mut self) -> bool {
        loop {
            self.skip_ws();
            if self.starts_with("<?") {
                let Some(end) = self.remaining().find("?>") else {
                    return false;
                };
                self.pos += end + 2;
            } else if self.starts_with("<!--") {
                let Some(end) = self.remaining().find("-->") else {
                    return false;
                };
                self.pos += end + 3;
            } else if self.starts_with("<!DOCTYPE") || self.starts_with("<!doctype") {
                let Some(end) = self.remaining().find('>') else {
                    return false;
                };
                self.pos += end + 1;
            } else {
                break;
            }
        }
        true
    }

    fn parse_document(&mut self) -> Option<XmlElement> {
        if !self.skip_prolog() {
            return None;
        }
        if !self.starts_with("<") || self.starts_with("</") || self.starts_with("<!") {
            return None;
        }
        let root = self.parse_element()?;
        self.skip_prolog();
        if self.pos < self.bytes.len() {
            return None;
        }
        Some(root)
    }

    fn parse_name(&mut self) -> Option<String> {
        let start = self.pos;
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b':' | b'.') {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos == start {
            None
        } else {
            Some(std::str::from_utf8(&self.bytes[start..self.pos]).ok()?.to_string())
        }
    }

    fn parse_attr_value(&mut self) -> Option<String> {
        self.skip_ws();
        let quote = *self.bytes.get(self.pos)?;
        if quote != b'"' && quote != b'\'' {
            return None;
        }
        self.pos += 1;
        let start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos] != quote {
            self.pos += 1;
        }
        if self.pos >= self.bytes.len() {
            return None;
        }
        let val = std::str::from_utf8(&self.bytes[start..self.pos])
            .ok()?
            .to_string();
        self.pos += 1;
        Some(val)
    }

    fn parse_element(&mut self) -> Option<XmlElement> {
        if !self.starts_with("<") || self.starts_with("</") {
            return None;
        }
        self.pos += 1;
        let name = self.parse_name()?;
        let mut attrs = Vec::new();

        loop {
            self.skip_ws();
            if self.starts_with("/>") {
                self.pos += 2;
                return Some(XmlElement {
                    name,
                    attrs,
                    children: Vec::new(),
                    text: String::new(),
                });
            }
            if self.starts_with(">") {
                self.pos += 1;
                break;
            }
            let attr_name = self.parse_name()?;
            self.skip_ws();
            if !self.starts_with("=") {
                return None;
            }
            self.pos += 1;
            let attr_val = self.parse_attr_value()?;
            attrs.push((attr_name, attr_val));
        }

        let mut children = Vec::new();
        let mut text = String::new();

        loop {
            if self.pos >= self.bytes.len() {
                return None;
            }
            if self.starts_with("</") {
                self.pos += 2;
                let close_name = self.parse_name()?;
                self.skip_ws();
                if !self.starts_with(">") || close_name != name {
                    return None;
                }
                self.pos += 1;
                break;
            } else if self.starts_with("<!--") {
                let end = self.remaining().find("-->")?;
                self.pos += end + 3;
            } else if self.starts_with("<![CDATA[") {
                self.pos += 9;
                let end = self.remaining().find("]]>")?;
                text.push_str(&self.remaining()[..end]);
                self.pos += end + 3;
            } else if self.starts_with("<") {
                children.push(self.parse_element()?);
            } else {
                let next_lt = self.remaining().find('<').unwrap_or(self.remaining().len());
                text.push_str(&self.remaining()[..next_lt]);
                self.pos += next_lt;
            }
        }

        Some(XmlElement {
            name,
            attrs,
            children,
            text,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_nested_json_object_and_array_of_objects() {
        let json = r#"{
            "code": 200,
            "ok": true,
            "data": {
                "users": [
                    { "id": 1, "name": "Alice" },
                    { "id": 2, "name": "Bob", "email": "bob@example.com" }
                ],
                "tags": ["admin", "staff"]
            }
        }"#;

        let fields = extract_response_meta(json, Some("application/json")).expect("json");
        assert_eq!(fields.len(), 3);
        let code = fields.iter().find(|f| f.name == "code").unwrap();
        assert_eq!(code.field_type, "integer");
        assert_eq!(code.example, "200");

        let data = fields.iter().find(|f| f.name == "data").unwrap();
        assert_eq!(data.field_type, "object");
        let users = data.children.iter().find(|f| f.name == "users").unwrap();
        assert_eq!(users.field_type, "array<object>");
        assert_eq!(users.children.len(), 3);
        assert!(users.children.iter().any(|f| f.name == "email"));

        let tags = data.children.iter().find(|f| f.name == "tags").unwrap();
        assert_eq!(tags.field_type, "array<string>");
    }

    #[test]
    fn extracts_xml_with_attributes_and_repeated_elements() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <response status="ok">
            <!-- user list -->
            <user id="1"><name>Alice</name></user>
            <user id="2"><name>Bob</name><role>admin</role></user>
        </response>"#;

        let fields = extract_response_meta(xml, Some("application/xml")).expect("xml");
        assert_eq!(fields.len(), 1);
        let root = &fields[0];
        assert_eq!(root.name, "response");
        assert_eq!(root.field_type, "object");
        assert!(root.children.iter().any(|c| c.name == "@status"));

        let user = root.children.iter().find(|c| c.name == "user").unwrap();
        assert_eq!(user.field_type, "array<object>");
        assert!(user.children.iter().any(|c| c.name == "@id"));
        assert!(user.children.iter().any(|c| c.name == "name"));
        assert!(user.children.iter().any(|c| c.name == "role"));
    }

    #[test]
    fn merges_different_responses_and_preserves_descriptions() {
        let mut meta = extract_response_meta(
            r#"{"id": 1, "name": "Alice", "extra": null}"#,
            Some("application/json"),
        )
        .unwrap();
        meta.iter_mut()
            .find(|f| f.name == "name")
            .unwrap()
            .description = "User display name".to_string();

        let ok = extract_and_merge_response_meta(
            &mut meta,
            r#"{"id": 2, "name": "Bob", "extra": "vip", "active": true}"#,
            Some("application/json"),
        );
        assert!(ok);

        let name = meta.iter().find(|f| f.name == "name").unwrap();
        assert_eq!(name.description, "User display name");

        let extra = meta.iter().find(|f| f.name == "extra").unwrap();
        assert_eq!(extra.field_type, "string");

        assert!(meta.iter().any(|f| f.name == "active" && f.field_type == "boolean"));
    }

    #[test]
    fn gives_up_extracting_non_json_and_non_xml() {
        assert!(extract_response_meta("hello world", Some("text/plain")).is_none());
        assert!(extract_response_meta("<html><body>hi</body></html>", Some("text/html")).is_none());
        assert!(extract_response_meta("<!DOCTYPE html><html></html>", None).is_none());
        assert!(extract_response_meta("{broken json", Some("application/json")).is_none());
        assert!(extract_response_meta("<unclosed><tag>", Some("application/xml")).is_none());
    }

    #[test]
    fn extracts_and_merges_request_body_meta_across_payload_modes() {
        use crate::model::{KeyValueRow, MultipartField, RawBodyFormat};

        let mut meta = Vec::new();
        let json_body = RequestBody::Json {
            content: r#"{"username":"alice","profile":{"age":30}}"#.to_string(),
        };
        assert!(extract_and_merge_request_body_meta(&mut meta, &json_body));
        meta.iter_mut()
            .find(|f| f.name == "username")
            .unwrap()
            .description = "Login account".to_string();

        let json_body_v2 = RequestBody::Json {
            content: r#"{"username":"bob","role":"admin"}"#.to_string(),
        };
        assert!(extract_and_merge_request_body_meta(&mut meta, &json_body_v2));
        assert_eq!(
            meta.iter().find(|f| f.name == "username").unwrap().description,
            "Login account"
        );
        assert!(meta.iter().any(|f| f.name == "role" && f.field_type == "string"));

        let form_body = RequestBody::FormUrlEncoded {
            fields: vec![KeyValueRow::new("grant_type", "password").with_description("OAuth grant")],
        };
        let form_meta = extract_request_body_meta(&form_body).unwrap();
        assert_eq!(form_meta.len(), 1);
        assert_eq!(form_meta[0].name, "grant_type");
        assert_eq!(form_meta[0].description, "OAuth grant");

        let multipart_body = RequestBody::Multipart {
            fields: vec![MultipartField::file("avatar", "/tmp/a.png")],
        };
        let mp_meta = extract_request_body_meta(&multipart_body).unwrap();
        assert_eq!(mp_meta[0].name, "avatar");
        assert_eq!(mp_meta[0].field_type, "file");

        let xml_body = RequestBody::Raw {
            raw_format: RawBodyFormat::Xml,
            content: "<user><id>1</id></user>".to_string(),
        };
        assert!(extract_request_body_meta(&xml_body).is_some());
        assert!(extract_request_body_meta(&RequestBody::None).is_none());
    }
}
