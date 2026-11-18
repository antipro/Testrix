use std::collections::BTreeMap;

use crate::model::EnvironmentDoc;

/// Result of expanding `{{variable}}` placeholders against an environment profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateResult {
    pub rendered: String,
    pub unresolved: Vec<String>,
}

/// Builds an enabled variable lookup map from an optional environment profile.
pub fn env_variable_map(env: Option<&EnvironmentDoc>) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if let Some(env) = env {
        for var in &env.variables {
            let key = var.key.trim();
            if var.enabled && !key.is_empty() {
                map.insert(key.to_string(), var.value.clone());
            }
        }
    }
    map
}

/// Expands `{{var_name}}` placeholders using the supplied environment profile.
///
/// Unmatched placeholders remain intact in `rendered` and are collected in `unresolved`.
pub fn interpolate_template(input: &str, env: Option<&EnvironmentDoc>) -> TemplateResult {
    let vars = env_variable_map(env);
    interpolate_with_map(input, &vars)
}

/// Expands `{{var_name}}` placeholders using a pre-built variable map.
pub fn interpolate_with_map(input: &str, vars: &BTreeMap<String, String>) -> TemplateResult {
    let mut rendered = String::with_capacity(input.len());
    let mut unresolved = Vec::new();
    let mut rest = input;

    while let Some(start) = rest.find("{{") {
        rendered.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        if let Some(end) = after_open.find("}}") {
            let raw_name = &after_open[..end];
            let name = raw_name.trim();
            if !name.is_empty()
                && let Some(value) = vars.get(name)
            {
                rendered.push_str(value);
            } else {
                rendered.push_str(&rest[start..start + 2 + end + 2]);
                if !name.is_empty() && !unresolved.iter().any(|u| u == name) {
                    unresolved.push(name.to_string());
                }
            }
            rest = &after_open[end + 2..];
        } else {
            rendered.push_str("{{");
            rest = after_open;
        }
    }

    rendered.push_str(rest);
    TemplateResult {
        rendered,
        unresolved,
    }
}

/// Rewrites `{{old_name}}` placeholders according to `(old_name, new_name)` pairs.
///
/// Returns `Some(updated)` only when at least one placeholder was rewritten.
pub fn rename_template_variables(input: &str, renames: &[(String, String)]) -> Option<String> {
    if renames.is_empty() || !input.contains("{{") {
        return None;
    }

    let mut rendered = String::with_capacity(input.len());
    let mut changed = false;
    let mut rest = input;

    while let Some(start) = rest.find("{{") {
        rendered.push_str(&rest[..start]);
        let after_open = &rest[start + 2..];
        if let Some(end) = after_open.find("}}") {
            let raw_name = &after_open[..end];
            let name = raw_name.trim();
            if let Some((_, next_name)) = renames.iter().find(|(old, _)| old == name) {
                rendered.push_str("{{");
                rendered.push_str(next_name);
                rendered.push_str("}}");
                if next_name != raw_name {
                    changed = true;
                }
            } else {
                rendered.push_str(&rest[start..start + 2 + end + 2]);
            }
            rest = &after_open[end + 2..];
        } else {
            rendered.push_str("{{");
            rest = after_open;
        }
    }

    if !changed {
        return None;
    }
    rendered.push_str(rest);
    Some(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_template_variables_without_double_swapping() {
        let renames = vec![
            ("base_url".to_string(), "base_url_emr".to_string()),
            ("a".to_string(), "b".to_string()),
            ("b".to_string(), "a".to_string()),
        ];

        assert_eq!(
            rename_template_variables("{{base_url}}/{{a}}/{{b}}/{{other}}", &renames),
            Some("{{base_url_emr}}/{{b}}/{{a}}/{{other}}".to_string())
        );
        assert_eq!(
            rename_template_variables("https://example.com/{{other}}", &renames),
            None
        );
    }
}
