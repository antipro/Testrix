use gpui_kit::{
    App, Hsla, IntoElement, ParentElement as _,
    assets::IconName as LucideIcon,
    component::{
        ActiveTheme as _, Sizable as _, StyledExt as _,
        tag::{Tag, TagVariant},
    },
};
use testrix_core::{AuthConfig, EffectiveAuth, HttpMethod};

/// Maps an [`HttpMethod`] to its semantic [`TagVariant`].
pub(crate) fn method_tag_variant(method: HttpMethod) -> TagVariant {
    match method {
        HttpMethod::Get => TagVariant::Success,
        HttpMethod::Post => TagVariant::Info,
        HttpMethod::Put => TagVariant::Warning,
        HttpMethod::Delete => TagVariant::Danger,
        HttpMethod::Patch => TagVariant::Primary,
        HttpMethod::Head | HttpMethod::Options => TagVariant::Secondary,
    }
}

/// Maps an [`HttpMethod`] to its semantic foreground color from the active theme.
pub(crate) fn method_color(method: HttpMethod, cx: &App) -> Hsla {
    match method {
        HttpMethod::Get => cx.theme().success,
        HttpMethod::Post => cx.theme().info,
        HttpMethod::Put => cx.theme().warning,
        HttpMethod::Delete => cx.theme().danger,
        HttpMethod::Patch => cx.theme().primary,
        HttpMethod::Head | HttpMethod::Options => cx.theme().muted_foreground,
    }
}

/// Compact outline [`Tag`] representing an HTTP method in tabs and explorer rows.
pub(crate) fn method_badge(method: HttpMethod, compact: bool) -> impl IntoElement {
    let label = if compact {
        method.short_label()
    } else {
        method.as_str()
    };
    Tag::new()
        .with_variant(method_tag_variant(method))
        .outline()
        .small()
        .font_semibold()
        .child(label)
}

/// Semantic [`Tag`] for an HTTP response status code.
pub(crate) fn status_badge(status_code: u16, reason: &str) -> impl IntoElement {
    let variant = if (200..300).contains(&status_code) {
        TagVariant::Success
    } else if (300..400).contains(&status_code) {
        TagVariant::Info
    } else if (400..500).contains(&status_code) {
        TagVariant::Warning
    } else {
        TagVariant::Danger
    };
    let text = if reason.is_empty() {
        status_code.to_string()
    } else {
        format!("{status_code} {reason}")
    };
    Tag::new().with_variant(variant).small().child(text)
}

/// Icon corresponding to an explicit [`AuthConfig`] mode.
pub(crate) fn auth_icon(auth: &AuthConfig) -> LucideIcon {
    match auth {
        AuthConfig::Inherit => LucideIcon::Layers,
        AuthConfig::None => LucideIcon::ShieldBan,
        AuthConfig::Bearer { .. } => LucideIcon::KeyRound,
        AuthConfig::Basic { .. } => LucideIcon::Lock,
        AuthConfig::ApiKey { .. } => LucideIcon::ShieldCheck,
    }
}

/// Icon corresponding to a resolved [`EffectiveAuth`] mode.
pub(crate) fn effective_auth_icon(auth: &EffectiveAuth) -> LucideIcon {
    match auth {
        EffectiveAuth::None => LucideIcon::ShieldBan,
        EffectiveAuth::Bearer { .. } => LucideIcon::KeyRound,
        EffectiveAuth::Basic { .. } => LucideIcon::Lock,
        EffectiveAuth::ApiKey { .. } => LucideIcon::ShieldCheck,
    }
}

/// Formats byte counts cleanly for response size labels.
pub(crate) fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.2} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}
