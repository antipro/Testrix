use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window,
    assets::IconName as LucideIcon,
    component::{
        ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
        WindowExt as _,
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        h_flex,
        input::{Input, InputEvent, InputState},
        scroll::ScrollableElement as _,
        select::{Select, SelectItem, SelectState},
        tab::{Tab, TabBar},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};
use testrix_core::{EnvVariable, EnvironmentDoc, EnvironmentNode};

struct EnvVarRowState {
    id: u64,
    enabled: bool,
    secret: bool,
    original_key: String,
    key: Entity<InputState>,
    value: Entity<InputState>,
    description: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Debug, Clone)]
pub(crate) enum EnvEditorEvent {
    Save {
        env_id: String,
        updated: EnvironmentDoc,
        renamed_keys: Vec<(String, String)>,
    },
    Create {
        name: String,
    },
    Delete {
        env_id: String,
    },
    SelectActive {
        env_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MergeProfileOption {
    id: String,
    name: String,
}

impl SelectItem for MergeProfileOption {
    type Value = String;

    fn title(&self) -> SharedString {
        self.name.clone().into()
    }

    fn value(&self) -> &Self::Value {
        &self.id
    }
}

/// Persistent editor view rendered inside the Environment Profiles sheet.
pub(crate) struct EnvEditorView {
    focus_handle: FocusHandle,
    profiles: Vec<EnvironmentNode>,
    editing_env_id: Option<String>,
    active_env_id: Option<String>,
    profile_name_input: Entity<InputState>,
    new_profile_input: Entity<InputState>,
    merge_panel_open: bool,
    merge_overwrite: bool,
    merge_source_select: Entity<SelectState<Vec<MergeProfileOption>>>,
    rows: Vec<EnvVarRowState>,
    next_row_id: u64,
    dirty: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<EnvEditorEvent> for EnvEditorView {}

impl Focusable for EnvEditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EnvEditorView {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let profile_name_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Profile name"));
        let new_profile_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("New profile name…"));
        let merge_source_select =
            cx.new(|cx| SelectState::new(Vec::<MergeProfileOption>::new(), None, window, cx));

        let sub = cx.subscribe(&profile_name_input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.dirty = true;
                cx.notify();
            }
        });

        Self {
            focus_handle: cx.focus_handle(),
            profiles: Vec::new(),
            editing_env_id: None,
            active_env_id: None,
            profile_name_input,
            new_profile_input,
            merge_panel_open: false,
            merge_overwrite: false,
            merge_source_select,
            rows: Vec::new(),
            next_row_id: 1,
            dirty: false,
            _subscriptions: vec![sub],
        }
    }

    pub(crate) fn sync_profiles(
        &mut self,
        profiles: Vec<EnvironmentNode>,
        active_env_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.profiles = profiles;
        self.active_env_id = active_env_id.clone();

        let target_id = self
            .editing_env_id
            .clone()
            .filter(|id| self.profiles.iter().any(|p| &p.doc.id == id))
            .or(active_env_id)
            .or_else(|| self.profiles.first().map(|p| p.doc.id.clone()));

        self.load_profile_for_editing(target_id.as_deref(), window, cx);
    }

    pub(crate) fn focus_profile(
        &mut self,
        env_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.load_profile_for_editing(Some(env_id), window, cx);
    }

    fn load_profile_for_editing(
        &mut self,
        env_id: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editing_env_id = env_id.map(str::to_string);
        self.rows.clear();
        self.dirty = false;
        self.refresh_merge_source_options(window, cx);

        if let Some(profile) = env_id.and_then(|id| self.profiles.iter().find(|p| p.doc.id == id)) {
            let name = profile.doc.name.clone();
            let vars = profile.doc.variables.clone();
            self.profile_name_input
                .update(cx, |state, cx| state.set_value(name, window, cx));
            for var in &vars {
                let row = self.build_row(var, window, cx);
                self.rows.push(row);
            }
        } else {
            self.profile_name_input
                .update(cx, |state, cx| state.set_value("", window, cx));
        }

        cx.notify();
    }

    fn build_row(
        &mut self,
        var: &EnvVariable,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> EnvVarRowState {
        let id = self.next_row_id;
        self.next_row_id += 1;

        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("KEY")
                .default_value(var.key.clone())
        });
        let value = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("value")
                .default_value(var.value.clone())
                .masked(var.secret)
        });
        let description = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Description")
                .default_value(var.description.clone())
        });

        let mut subs = Vec::with_capacity(3);
        for input in [&key, &value, &description] {
            subs.push(cx.subscribe(input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.dirty = true;
                    cx.notify();
                }
            }));
        }

        EnvVarRowState {
            id,
            enabled: var.enabled,
            secret: var.secret,
            original_key: var.key.trim().to_string(),
            key,
            value,
            description,
            _subscriptions: subs,
        }
    }

    fn refresh_merge_source_options(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editing_id = self.editing_env_id.as_deref();
        let options: Vec<MergeProfileOption> = self
            .profiles
            .iter()
            .filter(|p| Some(p.doc.id.as_str()) != editing_id)
            .map(|p| MergeProfileOption {
                id: p.doc.id.clone(),
                name: p.doc.name.clone(),
            })
            .collect();
        let has_options = !options.is_empty();
        if !has_options {
            self.merge_panel_open = false;
        }
        self.merge_source_select.update(cx, |sel, cx| {
            sel.set_items(options, window, cx);
            if has_options {
                sel.set_selected_index(Some(gpui_kit::component::IndexPath::default().row(0)), window, cx);
            }
        });
    }

    fn merge_from_selected_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(source_id) = self.merge_source_select.read(cx).selected_value().cloned() else {
            window.push_notification("Select a profile to merge variables from.", cx);
            return;
        };
        let Some(source_profile) = self.profiles.iter().find(|p| p.doc.id == source_id) else {
            return;
        };
        let source_name = source_profile.doc.name.clone();
        let source_vars = source_profile.doc.variables.clone();

        let mut added = 0usize;
        let mut updated = 0usize;

        for src_var in &source_vars {
            let src_key = src_var.key.trim();
            if src_key.is_empty() {
                continue;
            }
            let existing_ix = self
                .rows
                .iter()
                .position(|r| r.key.read(cx).value().trim() == src_key);

            if let Some(ix) = existing_ix {
                if self.merge_overwrite {
                    let row = &mut self.rows[ix];
                    row.enabled = src_var.enabled;
                    row.secret = src_var.secret;
                    let val = src_var.value.clone();
                    let desc = src_var.description.clone();
                    let masked = src_var.secret;
                    row.value.update(cx, |s, cx| {
                        s.set_masked(masked, window, cx);
                        s.set_value(val, window, cx);
                    });
                    row.description.update(cx, |s, cx| {
                        s.set_value(desc, window, cx);
                    });
                    updated += 1;
                }
            } else {
                let row = self.build_row(src_var, window, cx);
                self.rows.push(row);
                added += 1;
            }
        }

        if added > 0 || updated > 0 {
            self.dirty = true;
            self.merge_panel_open = false;
            window.push_notification(
                format!("Merged from '{source_name}': {added} added, {updated} updated."),
                cx,
            );
        } else {
            window.push_notification(
                format!("No new variables to merge from '{source_name}'."),
                cx,
            );
        }
        cx.notify();
    }

    fn add_variable(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let row = self.build_row(&EnvVariable::default(), window, cx);
        row.key.update(cx, |input, cx| input.focus(window, cx));
        self.rows.push(row);
        self.dirty = true;
        cx.notify();
    }

    fn commit_current_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(env_id) = self.editing_env_id.clone() else {
            return;
        };
        let Some(original) = self.profiles.iter().find(|p| p.doc.id == env_id) else {
            return;
        };

        let mut updated = original.doc.clone();
        let next_name = self.profile_name_input.read(cx).value().trim().to_string();
        if next_name.is_empty() {
            window.push_notification("Profile name must not be empty.", cx);
            return;
        }
        updated.name = next_name;
        let mut renamed_keys = Vec::new();
        updated.variables = self
            .rows
            .iter_mut()
            .map(|row| {
                let next_key_raw = row.key.read(cx).value().to_string();
                let next_key = next_key_raw.trim().to_string();
                if !row.original_key.is_empty()
                    && !next_key.is_empty()
                    && row.original_key != next_key
                {
                    renamed_keys.push((row.original_key.clone(), next_key.clone()));
                }
                row.original_key = next_key;

                let mut v = EnvVariable::new(
                    next_key_raw,
                    row.value.read(cx).value().to_string(),
                )
                .with_secret(row.secret)
                .with_description(row.description.read(cx).value().to_string());
                v.enabled = row.enabled;
                v
            })
            .collect();

        self.dirty = false;
        cx.emit(EnvEditorEvent::Save {
            env_id,
            updated,
            renamed_keys,
        });
        cx.notify();
    }
}

impl Render for EnvEditorView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_tab_ix = self
            .editing_env_id
            .as_deref()
            .and_then(|id| self.profiles.iter().position(|p| p.doc.id == id))
            .unwrap_or(0);

        let is_editing_active =
            self.editing_env_id.is_some() && self.editing_env_id == self.active_env_id;
        let has_profiles = !self.profiles.is_empty();
        let editing_id = self.editing_env_id.clone();

        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .min_h_0()
            .gap_3()
            // Profile selector tabs + create bar
            .child(
                v_flex()
                    .gap_2()
                    .when(has_profiles, |this| {
                        let profiles = self.profiles.clone();
                        this.child(
                            TabBar::new("env-profile-tabs")
                                .w_full()
                                .segmented()
                                .small()
                                .selected_index(selected_tab_ix)
                                .on_click(cx.listener(move |this, ix: &usize, window, cx| {
                                    if let Some(profile) = this.profiles.get(*ix) {
                                        let id = profile.doc.id.clone();
                                        this.load_profile_for_editing(Some(&id), window, cx);
                                    }
                                }))
                                .children(profiles.iter().map(|p| {
                                    let is_active =
                                        self.active_env_id.as_deref() == Some(p.doc.id.as_str());
                                    let label: SharedString = if is_active {
                                        format!("{} (Active)", p.doc.name).into()
                                    } else {
                                        p.doc.name.clone().into()
                                    };
                                    Tab::new().label(label)
                                })),
                        )
                    })
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        Input::new(&self.new_profile_input)
                                            .id("env-new-profile-input")
                                            .small(),
                                    ),
                            )
                            .child(
                                Button::new("env-create-profile")
                                    .outline()
                                    .small()
                                    .icon(IconName::Plus)
                                    .label("New Profile")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let name = this
                                            .new_profile_input
                                            .read(cx)
                                            .value()
                                            .trim()
                                            .to_string();
                                        let name = if name.is_empty() {
                                            "New Profile".to_string()
                                        } else {
                                            name
                                        };
                                        this.new_profile_input.update(cx, |s, cx| {
                                            s.set_value("", window, cx);
                                        });
                                        cx.emit(EnvEditorEvent::Create { name });
                                    })),
                            ),
                    ),
            )
            // Selected profile metadata + actions
            .when_some(editing_id, |this, env_id| {
                this.child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    Input::new(&self.profile_name_input)
                                        .id("env-profile-name-input")
                                        .small(),
                                ),
                        )
                        .child(
                            Button::new("env-set-active")
                                .small()
                                .when(is_editing_active, |b| b.primary().label("Active"))
                                .when(!is_editing_active, |b| b.outline().label("Set Active"))
                                .disabled(is_editing_active)
                                .on_click(cx.listener({
                                    let env_id = env_id.clone();
                                    move |_, _, _, cx| {
                                        cx.emit(EnvEditorEvent::SelectActive {
                                            env_id: Some(env_id.clone()),
                                        });
                                    }
                                })),
                        )
                        .child(
                            Button::new("env-delete-profile")
                                .ghost()
                                .small()
                                .icon(IconName::Delete)
                                .tooltip("Delete Profile")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(EnvEditorEvent::Delete {
                                        env_id: env_id.clone(),
                                    });
                                })),
                        ),
                )
            })
            // Variables header
            .child(
                h_flex()
                    .w_full()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .text_xs()
                    .font_medium()
                    .text_color(cx.theme().muted_foreground)
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(div().w_6())
                    .child(div().flex_1().child("Variable"))
                    .child(div().flex_1().child("Value"))
                    .child(div().w(px(52.)).child("Secret"))
                    .child(div().w_8()),
            )
            // Variables rows
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .gap_1p5()
                    .when(self.rows.is_empty(), |this| {
                        this.child(
                            div()
                                .px_3()
                                .py_4()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("No variables in this profile yet. Use {{variable}} in requests."),
                        )
                    })
                    .children(self.rows.iter().map(|row| {
                        let row_id = row.id;
                        h_flex()
                            .id(("env-var-row", row_id))
                            .w_full()
                            .px_2()
                            .gap_2()
                            .items_center()
                            .child(
                                Checkbox::new(("env-var-en", row_id))
                                    .checked(row.enabled)
                                    .on_change(cx.listener(move |this, next, _, cx| {
                                        if let Some(target) =
                                            this.rows.iter_mut().find(|r| r.id == row_id)
                                        {
                                            target.enabled = *next;
                                            this.dirty = true;
                                            cx.notify();
                                        }
                                    })),
                            )
                            .child(
                                div().flex_1().min_w_0().child(
                                    Input::new(&row.key)
                                        .id(SharedString::from(format!("env-var-key-{row_id}")))
                                        .small(),
                                ),
                            )
                            .child(
                                div().flex_1().min_w_0().child(
                                    Input::new(&row.value)
                                        .id(SharedString::from(format!("env-var-val-{row_id}")))
                                        .small()
                                        .when(row.secret, |input| input.mask_toggle()),
                                ),
                            )
                            .child(
                                div().w(px(52.)).flex().justify_center().child(
                                    Checkbox::new(("env-var-sec", row_id))
                                        .checked(row.secret)
                                        .on_change(cx.listener(move |this, next, window, cx| {
                                            if let Some(target) =
                                                this.rows.iter_mut().find(|r| r.id == row_id)
                                            {
                                                target.secret = *next;
                                                target.value.update(cx, |state, cx| {
                                                    state.set_masked(*next, window, cx);
                                                });
                                                this.dirty = true;
                                                cx.notify();
                                            }
                                        })),
                                ),
                            )
                            .child(
                                Button::new(("env-var-del", row_id))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Delete)
                                    .tooltip("Remove variable")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.rows.retain(|r| r.id != row_id);
                                        this.dirty = true;
                                        cx.notify();
                                    })),
                            )
                    })),
            )
            // Merge from other profile panel (collapsible)
            .when(self.merge_panel_open && self.profiles.len() > 1, |this| {
                this.child(
                    h_flex()
                        .w_full()
                        .p_2()
                        .gap_2()
                        .items_center()
                        .rounded(cx.theme().radius)
                        .border_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().secondary)
                        .child(
                            div()
                                .text_xs()
                                .font_medium()
                                .child("Merge from:"),
                        )
                        .child(
                            div()
                                .w(px(180.))
                                .child(Select::new(&self.merge_source_select).small()),
                        )
                        .child(
                            Checkbox::new("env-merge-overwrite")
                                .label("Overwrite existing")
                                .checked(self.merge_overwrite)
                                .on_change(cx.listener(|this, next, _, cx| {
                                    this.merge_overwrite = *next;
                                    cx.notify();
                                })),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new("env-merge-apply")
                                .primary()
                                .xsmall()
                                .label("Merge")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.merge_from_selected_profile(window, cx);
                                })),
                        )
                        .child(
                            Button::new("env-merge-cancel")
                                .ghost()
                                .xsmall()
                                .label("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.merge_panel_open = false;
                                    cx.notify();
                                })),
                        ),
                )
            })
            // Footer actions
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .pt_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("env-add-variable")
                                    .outline()
                                    .small()
                                    .icon(IconName::Plus)
                                    .label("Add Variable")
                                    .disabled(self.editing_env_id.is_none())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.add_variable(window, cx);
                                    })),
                            )
                            .when(self.profiles.len() > 1, |row| {
                                row.child(
                                    Button::new("env-merge-toggle")
                                        .outline()
                                        .small()
                                        .icon(Icon::new(LucideIcon::GitMerge))
                                        .label("Merge from…")
                                        .disabled(self.editing_env_id.is_none())
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.merge_panel_open = !this.merge_panel_open;
                                            if this.merge_panel_open {
                                                this.refresh_merge_source_options(window, cx);
                                            }
                                            cx.notify();
                                        })),
                                )
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("env-use-none")
                                    .ghost()
                                    .small()
                                    .label("No Environment")
                                    .disabled(self.active_env_id.is_none())
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(EnvEditorEvent::SelectActive { env_id: None });
                                    })),
                            )
                            .child(
                                Button::new("env-save-profile")
                                    .primary()
                                    .small()
                                    .icon(Icon::new(LucideIcon::Save))
                                    .label(if self.dirty { "Save Profile*" } else { "Save Profile" })
                                    .disabled(self.editing_env_id.is_none())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.commit_current_profile(window, cx);
                                    })),
                            ),
                    ),
            )
    }
}
