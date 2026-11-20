use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window,
    component::{
        ActiveTheme as _, Icon, Sizable as _, StyledExt as _,
        button::{Button, ButtonVariants as _},
        form::{Field, Form},
        h_flex,
        input::{Input, InputEvent, InputState},
        select::{Select, SelectEvent, SelectItem, SelectState},
        tag::{Tag, TagVariant},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};
use testrix_core::{
    ApiKeyLocation, AuthConfig, AuthNodeRef, EffectiveAuth, EnvironmentDoc, ResolvedAuth,
    interpolate_template, rename_template_variables, replace_template_variables,
};

use crate::theme_helpers::{apply_unresolved_var_border, auth_icon, effective_auth_icon};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthModeChoice {
    Inherit,
    None,
    Bearer,
    Basic,
    ApiKey,
}

impl AuthModeChoice {
    fn label(self) -> &'static str {
        match self {
            Self::Inherit => "Inherit from parent",
            Self::None => "No Auth",
            Self::Bearer => "Bearer Token",
            Self::Basic => "Basic Auth",
            Self::ApiKey => "API Key",
        }
    }

    fn from_config(cfg: &AuthConfig) -> Self {
        match cfg {
            AuthConfig::Inherit => Self::Inherit,
            AuthConfig::None => Self::None,
            AuthConfig::Bearer { .. } => Self::Bearer,
            AuthConfig::Basic { .. } => Self::Basic,
            AuthConfig::ApiKey { .. } => Self::ApiKey,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuthModeOption(AuthModeChoice);

impl SelectItem for AuthModeOption {
    type Value = AuthModeChoice;

    fn title(&self) -> SharedString {
        self.0.label().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub(crate) enum AuthEditorEvent {
    Changed,
    OpenSourceNode(AuthNodeRef),
}

/// Reusable stateful authentication editor + inheritance chain display.
pub(crate) struct AuthEditor {
    id_prefix: SharedString,
    allow_inherit: bool,
    mode: AuthModeChoice,
    mode_select: Entity<SelectState<Vec<AuthModeOption>>>,
    bearer_token: Entity<InputState>,
    bearer_prefix: Entity<InputState>,
    basic_username: Entity<InputState>,
    basic_password: Entity<InputState>,
    api_key_name: Entity<InputState>,
    api_key_value: Entity<InputState>,
    api_key_location: ApiKeyLocation,
    resolved_preview: Option<ResolvedAuth>,
    active_env: Option<EnvironmentDoc>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<AuthEditorEvent> for AuthEditor {}

impl AuthEditor {
    pub(crate) fn new(
        id_prefix: impl Into<SharedString>,
        allow_inherit: bool,
        initial: &AuthConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut choices = Vec::new();
        if allow_inherit {
            choices.push(AuthModeOption(AuthModeChoice::Inherit));
        }
        choices.extend([
            AuthModeOption(AuthModeChoice::None),
            AuthModeOption(AuthModeChoice::Bearer),
            AuthModeOption(AuthModeChoice::Basic),
            AuthModeOption(AuthModeChoice::ApiKey),
        ]);

        let initial_mode = if !allow_inherit && matches!(initial, AuthConfig::Inherit) {
            AuthModeChoice::None
        } else {
            AuthModeChoice::from_config(initial)
        };
        let initial_ix = choices
            .iter()
            .position(|c| c.0 == initial_mode)
            .map(|row| gpui_kit::component::IndexPath::default().row(row));

        let mode_select = cx.new(|cx| SelectState::new(choices, initial_ix, window, cx));

        let (token, prefix) = match initial {
            AuthConfig::Bearer { token, prefix } => (token.clone(), prefix.clone()),
            _ => (String::new(), "Bearer".to_string()),
        };
        let (username, password) = match initial {
            AuthConfig::Basic { username, password } => (username.clone(), password.clone()),
            _ => (String::new(), String::new()),
        };
        let (key, value, location) = match initial {
            AuthConfig::ApiKey {
                key,
                value,
                location,
            } => (key.clone(), value.clone(), *location),
            _ => (String::new(), String::new(), ApiKeyLocation::Header),
        };

        let bearer_token = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("{{api_token}}")
                .default_value(token)
        });
        let bearer_prefix = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Bearer")
                .default_value(prefix)
        });
        let basic_username = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Username")
                .default_value(username)
        });
        let basic_password = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Password")
                .default_value(password)
                .masked(true)
        });
        let api_key_name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("X-API-Key")
                .default_value(key)
        });
        let api_key_value = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("{{api_key}}")
                .default_value(value)
        });

        let mut subscriptions = vec![cx.subscribe_in(
            &mode_select,
            window,
            |this, _, event: &SelectEvent<Vec<AuthModeOption>>, _, cx| {
                if let SelectEvent::Confirm(Some(choice)) = event {
                    this.mode = *choice;
                    cx.emit(AuthEditorEvent::Changed);
                    cx.notify();
                }
            },
        )];

        for input in [
            &bearer_token,
            &bearer_prefix,
            &basic_username,
            &basic_password,
            &api_key_name,
            &api_key_value,
        ] {
            subscriptions.push(cx.subscribe(input, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.emit(AuthEditorEvent::Changed);
                    cx.notify();
                }
            }));
        }

        Self {
            id_prefix: id_prefix.into(),
            allow_inherit,
            mode: initial_mode,
            mode_select,
            bearer_token,
            bearer_prefix,
            basic_username,
            basic_password,
            api_key_name,
            api_key_value,
            api_key_location: location,
            resolved_preview: None,
            active_env: None,
            _subscriptions: subscriptions,
        }
    }

    pub(crate) fn set_context(
        &mut self,
        resolved: Option<ResolvedAuth>,
        active_env: Option<EnvironmentDoc>,
        cx: &mut Context<Self>,
    ) {
        self.resolved_preview = resolved;
        self.active_env = active_env;
        cx.notify();
    }

    pub(crate) fn rewrite_variables(
        &mut self,
        pairs: &[(String, String)],
        wrap_braces: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut changed = false;
        for input in [
            &self.bearer_token,
            &self.bearer_prefix,
            &self.basic_username,
            &self.basic_password,
            &self.api_key_name,
            &self.api_key_value,
        ] {
            let current = input.read(cx).value().to_string();
            let updated = if wrap_braces {
                rename_template_variables(&current, pairs)
            } else {
                replace_template_variables(&current, pairs)
            };
            if let Some(updated) = updated {
                input.update(cx, |state, cx| state.set_value(updated, window, cx));
                changed = true;
            }
        }
        if changed {
            cx.notify();
        }
        changed
    }

    pub(crate) fn set_auth(
        &mut self,
        auth: &AuthConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let next_mode = if !self.allow_inherit && matches!(auth, AuthConfig::Inherit) {
            AuthModeChoice::None
        } else {
            AuthModeChoice::from_config(auth)
        };
        self.mode = next_mode;
        let mut choices = Vec::new();
        if self.allow_inherit {
            choices.push(AuthModeChoice::Inherit);
        }
        choices.extend([
            AuthModeChoice::None,
            AuthModeChoice::Bearer,
            AuthModeChoice::Basic,
            AuthModeChoice::ApiKey,
        ]);
        if let Some(row) = choices.iter().position(|c| *c == next_mode) {
            self.mode_select.update(cx, |sel, cx| {
                sel.set_selected_index(
                    Some(gpui_kit::component::IndexPath::default().row(row)),
                    window,
                    cx,
                );
            });
        }
        match auth {
            AuthConfig::Bearer { token, prefix } => {
                self.bearer_token
                    .update(cx, |s, cx| s.set_value(token.clone(), window, cx));
                self.bearer_prefix
                    .update(cx, |s, cx| s.set_value(prefix.clone(), window, cx));
            }
            AuthConfig::Basic { username, password } => {
                self.basic_username
                    .update(cx, |s, cx| s.set_value(username.clone(), window, cx));
                self.basic_password
                    .update(cx, |s, cx| s.set_value(password.clone(), window, cx));
            }
            AuthConfig::ApiKey {
                key,
                value,
                location,
            } => {
                self.api_key_name
                    .update(cx, |s, cx| s.set_value(key.clone(), window, cx));
                self.api_key_value
                    .update(cx, |s, cx| s.set_value(value.clone(), window, cx));
                self.api_key_location = *location;
            }
            AuthConfig::Inherit | AuthConfig::None => {}
        }
        cx.notify();
    }

    pub(crate) fn snapshot(&self, cx: &App) -> AuthConfig {
        match self.mode {
            AuthModeChoice::Inherit => AuthConfig::Inherit,
            AuthModeChoice::None => AuthConfig::None,
            AuthModeChoice::Bearer => {
                let prefix = self.bearer_prefix.read(cx).value().trim().to_string();
                AuthConfig::Bearer {
                    token: self.bearer_token.read(cx).value().to_string(),
                    prefix: if prefix.is_empty() {
                        "Bearer".to_string()
                    } else {
                        prefix
                    },
                }
            }
            AuthModeChoice::Basic => AuthConfig::Basic {
                username: self.basic_username.read(cx).value().to_string(),
                password: self.basic_password.read(cx).value().to_string(),
            },
            AuthModeChoice::ApiKey => AuthConfig::ApiKey {
                key: self.api_key_name.read(cx).value().to_string(),
                value: self.api_key_value.read(cx).value().to_string(),
                location: self.api_key_location,
            },
        }
    }

    fn render_inheritance_chain(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(resolved) = &self.resolved_preview else {
            return div().into_any_element();
        };

        let env_ref = self.active_env.as_ref();
        let effective_preview = match &resolved.effective {
            EffectiveAuth::None => "No authentication headers will be sent.".to_string(),
            EffectiveAuth::Bearer { token, prefix } => {
                let rendered = interpolate_template(token, env_ref).rendered;
                let masked = mask_secret(&rendered);
                format!("Authorization: {} {masked}", prefix)
            }
            EffectiveAuth::Basic { username, .. } => {
                let rendered_user = interpolate_template(username, env_ref).rendered;
                format!("Authorization: Basic <base64({rendered_user}:•••)>")
            }
            EffectiveAuth::ApiKey {
                key,
                value,
                location,
            } => {
                let k = interpolate_template(key, env_ref).rendered;
                let v = mask_secret(&interpolate_template(value, env_ref).rendered);
                format!("{} `{k}` = {v}", location.as_str())
            }
        };

        let source_node = resolved.source.clone();
        let is_inherited = resolved.is_inherited();

        v_flex()
            .w_full()
            .p_3()
            .gap_2()
            .rounded(cx.theme().radius)
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().group_box)
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Icon::new(effective_auth_icon(&resolved.effective))
                                    .small()
                                    .text_color(cx.theme().primary),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .child(format!("Effective: {}", resolved.summary())),
                            ),
                    )
                    .when(is_inherited, |this| {
                        this.when_some(source_node.clone(), |this, src| {
                            let label = format!("Edit {} Auth", src.kind_label());
                            this.child(
                                Button::new(format!("{}-edit-parent-auth", self.id_prefix))
                                    .outline()
                                    .xsmall()
                                    .label(label)
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(AuthEditorEvent::OpenSourceNode(src.clone()));
                                    })),
                            )
                        })
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_color(cx.theme().muted_foreground)
                    .child(effective_preview),
            )
            .when(resolved.chain.len() > 1, |this| {
                this.child(h_flex().flex_wrap().gap_1p5().pt_1().children(
                    resolved.chain.iter().enumerate().map(|(ix, step)| {
                        let is_winner = source_node.as_ref() == Some(&step.node);
                        h_flex()
                            .gap_1()
                            .when(ix > 0, |row| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("→"),
                                )
                            })
                            .child(
                                Tag::new()
                                    .with_variant(if is_winner {
                                        TagVariant::Primary
                                    } else {
                                        TagVariant::Secondary
                                    })
                                    .outline()
                                    .small()
                                    .child(format!(
                                        "{} \"{}\" ({})",
                                        step.node.kind_label(),
                                        step.node.name(),
                                        step.configured.mode_label()
                                    )),
                            )
                    }),
                ))
            })
            .into_any_element()
    }
}

impl Render for AuthEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prefix = self.id_prefix.clone();
        v_flex()
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        h_flex()
                            .gap_1p5()
                            .w(px(120.))
                            .text_sm()
                            .font_medium()
                            .child(Icon::new(auth_icon(&self.snapshot(cx))).small())
                            .child("Auth Type"),
                    )
                    .child(
                        div().w(px(240.)).flex_none().child(
                            Select::new(&self.mode_select)
                                .id(format!("{prefix}-auth-mode"))
                                .small(),
                        ),
                    ),
            )
            .when(self.mode == AuthModeChoice::Inherit, |this| {
                this.child(self.render_inheritance_chain(cx))
            })
            .when(self.mode == AuthModeChoice::None, |this| {
                this.child(
                    div()
                        .p_3()
                        .rounded(cx.theme().radius)
                        .border_1()
                        .border_color(cx.theme().border)
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Authentication is disabled on this node; parent auth will not be sent."),
                )
            })
            .when(self.mode == AuthModeChoice::Bearer, |this| {
                this.child(
                    Form::new()
                        .columns(2)
                        .child(
                            Field::new()
                                .label("Token")
                                .child({
                                    let text = self.bearer_token.read(cx).value().to_string();
                                    apply_unresolved_var_border(
                                        Input::new(&self.bearer_token).small(),
                                        &text,
                                        self.active_env.as_ref(),
                                        cx,
                                    )
                                }),
                        )
                        .child(
                            Field::new()
                                .label("Header Prefix")
                                .child({
                                    let text = self.bearer_prefix.read(cx).value().to_string();
                                    apply_unresolved_var_border(
                                        Input::new(&self.bearer_prefix).small(),
                                        &text,
                                        self.active_env.as_ref(),
                                        cx,
                                    )
                                }),
                        ),
                )
            })
            .when(self.mode == AuthModeChoice::Basic, |this| {
                this.child(
                    Form::new()
                        .columns(2)
                        .child(
                            Field::new()
                                .label("Username")
                                .child({
                                    let text = self.basic_username.read(cx).value().to_string();
                                    apply_unresolved_var_border(
                                        Input::new(&self.basic_username).small(),
                                        &text,
                                        self.active_env.as_ref(),
                                        cx,
                                    )
                                }),
                        )
                        .child(
                            Field::new().label("Password").child({
                                let text = self.basic_password.read(cx).value().to_string();
                                apply_unresolved_var_border(
                                    Input::new(&self.basic_password).small().mask_toggle(),
                                    &text,
                                    self.active_env.as_ref(),
                                    cx,
                                )
                            }),
                        ),
                )
            })
            .when(self.mode == AuthModeChoice::ApiKey, |this| {
                this.child(
                    Form::new()
                        .columns(3)
                        .child(
                            Field::new()
                                .label("Key Name")
                                .child({
                                    let text = self.api_key_name.read(cx).value().to_string();
                                    apply_unresolved_var_border(
                                        Input::new(&self.api_key_name).small(),
                                        &text,
                                        self.active_env.as_ref(),
                                        cx,
                                    )
                                }),
                        )
                        .child(
                            Field::new()
                                .label("Value")
                                .child({
                                    let text = self.api_key_value.read(cx).value().to_string();
                                    apply_unresolved_var_border(
                                        Input::new(&self.api_key_value).small(),
                                        &text,
                                        self.active_env.as_ref(),
                                        cx,
                                    )
                                }),
                        )
                        .child(
                            Field::new().label("Add To").child(
                                h_flex()
                                    .gap_1p5()
                                    .child(
                                        Button::new(format!("{prefix}-apikey-header"))
                                            .small()
                                            .label("Header")
                                            .when(self.api_key_location == ApiKeyLocation::Header, |b| b.primary())
                                            .when(self.api_key_location != ApiKeyLocation::Header, |b| b.outline())
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.api_key_location = ApiKeyLocation::Header;
                                                cx.emit(AuthEditorEvent::Changed);
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new(format!("{prefix}-apikey-query"))
                                            .small()
                                            .label("Query Param")
                                            .when(self.api_key_location == ApiKeyLocation::Query, |b| b.primary())
                                            .when(self.api_key_location != ApiKeyLocation::Query, |b| b.outline())
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.api_key_location = ApiKeyLocation::Query;
                                                cx.emit(AuthEditorEvent::Changed);
                                                cx.notify();
                                            })),
                                    ),
                            ),
                        ),
                )
            })
            .when(self.mode != AuthModeChoice::Inherit && self.resolved_preview.is_some(), |this| {
                this.child(self.render_inheritance_chain(cx))
            })
    }
}

fn mask_secret(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "<empty>".to_string();
    }
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= 6 {
        "••••••".to_string()
    } else {
        let head: String = chars.iter().take(3).collect();
        let tail: String = chars.iter().skip(chars.len() - 3).collect();
        format!("{head}••••{tail}")
    }
}
