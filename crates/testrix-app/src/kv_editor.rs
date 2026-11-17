use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Subscription, Window,
    assets::IconName as LucideIcon,
    component::{
        ActiveTheme as _, IconName, Sizable as _, StyledExt as _,
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        h_flex,
        input::{Input, InputEvent, InputState},
        scroll::ScrollableElement as _,
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};
use testrix_core::{KeyValueRow, MultipartField, MultipartFieldType};

use crate::editable_combobox::{ComboboxOption, EditableCombobox, EditableComboboxEvent};

/// Standard HTTP request header suggestions for editable header comboboxes.
pub(crate) fn common_http_header_options() -> Vec<ComboboxOption> {
    [
        ("Accept", "Media types acceptable for response"),
        ("Accept-Encoding", "Supported compression (gzip, br)"),
        ("Accept-Language", "Preferred response language"),
        ("Authorization", "Credentials (Bearer, Basic, etc.)"),
        ("Cache-Control", "Caching directives"),
        ("Connection", "Connection management (keep-alive)"),
        ("Content-Type", "MIME type of request body"),
        ("Cookie", "HTTP cookies sent to server"),
        ("Host", "Target host and port"),
        ("If-Match", "Conditional request ETag match"),
        ("If-None-Match", "Conditional request cache ETag"),
        ("Origin", "CORS origin URI"),
        ("Pragma", "Implementation-specific header"),
        ("Referer", "Referring URI"),
        ("User-Agent", "Client user agent string"),
        ("X-Api-Key", "API key header"),
        ("X-Correlation-ID", "Distributed trace correlation ID"),
        ("X-Forwarded-For", "Originating client IP address"),
        ("X-Idempotency-Key", "Idempotency key for safe retries"),
        ("X-Request-ID", "Unique per-request identifier"),
        ("X-Requested-With", "Requested-with marker"),
    ]
    .into_iter()
    .map(|(name, desc)| ComboboxOption::new(name).with_detail(desc))
    .collect()
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum TableChangeEvent {
    Changed,
}

enum NameField {
    Input(Entity<InputState>),
    Combobox(Entity<EditableCombobox>),
}

impl NameField {
    fn value(&self, cx: &App) -> String {
        match self {
            Self::Input(input) => input.read(cx).value().to_string(),
            Self::Combobox(combo) => combo.read(cx).value(cx),
        }
    }
}

struct KeyValueRowState {
    id: u64,
    enabled: bool,
    name: NameField,
    value: Entity<InputState>,
    description: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

/// Stateful, retained key-value table editor for Params, Headers, and Form URL-Encoded fields.
pub(crate) struct KeyValueTable {
    id_prefix: SharedString,
    name_placeholder: SharedString,
    value_placeholder: SharedString,
    add_label: SharedString,
    name_suggestions: Vec<ComboboxOption>,
    rows: Vec<KeyValueRowState>,
    next_row_id: u64,
}

impl EventEmitter<TableChangeEvent> for KeyValueTable {}

impl KeyValueTable {
    pub(crate) fn new(
        id_prefix: impl Into<SharedString>,
        name_placeholder: impl Into<SharedString>,
        value_placeholder: impl Into<SharedString>,
        add_label: impl Into<SharedString>,
        initial: &[KeyValueRow],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut table = Self {
            id_prefix: id_prefix.into(),
            name_placeholder: name_placeholder.into(),
            value_placeholder: value_placeholder.into(),
            add_label: add_label.into(),
            name_suggestions: Vec::new(),
            rows: Vec::new(),
            next_row_id: 1,
        };
        table.set_rows(initial, window, cx);
        table
    }

    pub(crate) fn with_name_suggestions(
        mut self,
        suggestions: Vec<ComboboxOption>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let existing = self.snapshot(cx);
        self.name_suggestions = suggestions;
        self.next_row_id = 1;
        self.set_rows(&existing, window, cx);
        self
    }

    pub(crate) fn set_rows(
        &mut self,
        items: &[KeyValueRow],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rows.clear();
        for item in items {
            let row = self.build_row(item, window, cx);
            self.rows.push(row);
        }
        cx.notify();
    }

    pub(crate) fn add_empty_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let row = self.build_row(&KeyValueRow::default(), window, cx);
        self.rows.push(row);
        cx.emit(TableChangeEvent::Changed);
        cx.notify();
    }

    pub(crate) fn snapshot(&self, cx: &App) -> Vec<KeyValueRow> {
        self.rows
            .iter()
            .map(|row| {
                KeyValueRow::new(row.name.value(cx), row.value.read(cx).value().to_string())
                    .with_enabled(row.enabled)
                    .with_description(row.description.read(cx).value().to_string())
            })
            .collect()
    }

    fn build_row(
        &mut self,
        item: &KeyValueRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> KeyValueRowState {
        let id = self.next_row_id;
        self.next_row_id += 1;

        let (name, sub_name) = if self.name_suggestions.is_empty() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(self.name_placeholder.clone())
                    .default_value(item.name.clone())
            });
            let sub = cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.emit(TableChangeEvent::Changed);
                    cx.notify();
                }
            });
            (NameField::Input(input), sub)
        } else {
            let combo_id = format!("{}-name-{id}", self.id_prefix);
            let suggestions = self.name_suggestions.clone();
            let placeholder = self.name_placeholder.clone();
            let default_val = item.name.clone();
            let combo = cx.new(|cx| {
                EditableCombobox::new(combo_id, placeholder, default_val, suggestions, window, cx)
                    .small(true)
                    .menu_width(px(300.))
            });
            let sub = cx.subscribe(&combo, |_, _, _: &EditableComboboxEvent, cx| {
                cx.emit(TableChangeEvent::Changed);
                cx.notify();
            });
            (NameField::Combobox(combo), sub)
        };
        let value = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(self.value_placeholder.clone())
                .default_value(item.value.clone())
        });
        let description = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Description")
                .default_value(item.description.clone())
        });

        let sub_val = cx.subscribe(&value, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.emit(TableChangeEvent::Changed);
                cx.notify();
            }
        });
        let sub_desc = cx.subscribe(&description, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.emit(TableChangeEvent::Changed);
                cx.notify();
            }
        });

        KeyValueRowState {
            id,
            enabled: item.enabled,
            name,
            value,
            description,
            _subscriptions: vec![sub_name, sub_val, sub_desc],
        }
    }
}

impl Render for KeyValueTable {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prefix = self.id_prefix.clone();

        v_flex()
            .size_full()
            .min_h_0()
            .gap_2()
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
                    .child(div().flex_1().child(self.name_placeholder.clone()))
                    .child(div().flex_1().child(self.value_placeholder.clone()))
                    .child(div().flex_1().child("Description"))
                    .child(div().w_8()),
            )
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
                                .child("No rows configured yet."),
                        )
                    })
                    .children(self.rows.iter().map(|row| {
                        let row_id = row.id;
                        h_flex()
                            .id(SharedString::from(format!("{prefix}-row-{row_id}")))
                            .w_full()
                            .px_2()
                            .gap_2()
                            .items_center()
                            .child(
                                Checkbox::new(SharedString::from(format!("{prefix}-en-{row_id}")))
                                    .checked(row.enabled)
                                    .on_change(cx.listener(move |this, next, _, cx| {
                                        if let Some(target) =
                                            this.rows.iter_mut().find(|r| r.id == row_id)
                                        {
                                            target.enabled = *next;
                                            cx.emit(TableChangeEvent::Changed);
                                            cx.notify();
                                        }
                                    })),
                            )
                            .child(div().flex_1().min_w_0().child(match &row.name {
                                NameField::Input(input) => {
                                    Input::new(input).small().into_any_element()
                                }
                                NameField::Combobox(combo) => combo.clone().into_any_element(),
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&row.value).small()),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(Input::new(&row.description).small()),
                            )
                            .child(
                                Button::new(SharedString::from(format!("{prefix}-del-{row_id}")))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Delete)
                                    .tooltip("Remove row")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.rows.retain(|r| r.id != row_id);
                                        cx.emit(TableChangeEvent::Changed);
                                        cx.notify();
                                    })),
                            )
                    })),
            )
            .child(
                h_flex().pt_1().child(
                    Button::new(format!("{prefix}-add"))
                        .outline()
                        .small()
                        .icon(IconName::Plus)
                        .label(self.add_label.clone())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.add_empty_row(window, cx);
                        })),
                ),
            )
    }
}

struct MultipartRowState {
    id: u64,
    enabled: bool,
    field_type: MultipartFieldType,
    name: Entity<InputState>,
    value: Entity<InputState>,
    content_type: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

/// Stateful table for `multipart/form-data` text and file fields.
pub(crate) struct MultipartTable {
    id_prefix: SharedString,
    rows: Vec<MultipartRowState>,
    next_row_id: u64,
}

impl EventEmitter<TableChangeEvent> for MultipartTable {}

impl MultipartTable {
    pub(crate) fn new(
        id_prefix: impl Into<SharedString>,
        initial: &[MultipartField],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut table = Self {
            id_prefix: id_prefix.into(),
            rows: Vec::new(),
            next_row_id: 1,
        };
        table.set_rows(initial, window, cx);
        table
    }

    pub(crate) fn set_rows(
        &mut self,
        items: &[MultipartField],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rows.clear();
        for item in items {
            let row = self.build_row(item, window, cx);
            self.rows.push(row);
        }
        cx.notify();
    }

    pub(crate) fn snapshot(&self, cx: &App) -> Vec<MultipartField> {
        self.rows
            .iter()
            .map(|row| {
                let name = row.name.read(cx).value().to_string();
                let value = row.value.read(cx).value().to_string();
                let ct = row.content_type.read(cx).value().to_string();
                let mut field = match row.field_type {
                    MultipartFieldType::Text => MultipartField::text(name, value),
                    MultipartFieldType::File => MultipartField::file(name, value),
                };
                field.enabled = row.enabled;
                field.content_type = if ct.trim().is_empty() { None } else { Some(ct) };
                field
            })
            .collect()
    }

    fn build_row(
        &mut self,
        item: &MultipartField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> MultipartRowState {
        let id = self.next_row_id;
        self.next_row_id += 1;

        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Field name")
                .default_value(item.name.clone())
        });
        let val_placeholder = match item.field_type {
            MultipartFieldType::Text => "Text value",
            MultipartFieldType::File => "File path",
        };
        let value = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(val_placeholder)
                .default_value(item.value.clone())
        });
        let content_type = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Content-Type (optional)")
                .default_value(item.content_type.clone().unwrap_or_default())
        });

        let sub_name = cx.subscribe(&name, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.emit(TableChangeEvent::Changed);
                cx.notify();
            }
        });
        let sub_val = cx.subscribe(&value, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.emit(TableChangeEvent::Changed);
                cx.notify();
            }
        });
        let sub_ct = cx.subscribe(&content_type, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.emit(TableChangeEvent::Changed);
                cx.notify();
            }
        });

        MultipartRowState {
            id,
            enabled: item.enabled,
            field_type: item.field_type,
            name,
            value,
            content_type,
            _subscriptions: vec![sub_name, sub_val, sub_ct],
        }
    }
}

impl Render for MultipartTable {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prefix = self.id_prefix.clone();
        v_flex()
            .size_full()
            .min_h_0()
            .gap_2()
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
                    .child(div().w(px(72.)).child("Type"))
                    .child(div().flex_1().child("Field"))
                    .child(div().flex_1().child("Value / File Path"))
                    .child(div().w(px(140.)).child("Content-Type"))
                    .child(div().w_8()),
            )
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
                                .child("No multipart fields configured yet."),
                        )
                    })
                    .children(self.rows.iter().map(|row| {
                        let row_id = row.id;
                        let is_file = row.field_type == MultipartFieldType::File;
                        h_flex()
                            .id(SharedString::from(format!("{prefix}-row-{row_id}")))
                            .w_full()
                            .px_2()
                            .gap_2()
                            .items_center()
                            .child(
                                Checkbox::new(SharedString::from(format!("{prefix}-en-{row_id}")))
                                    .checked(row.enabled)
                                    .on_change(cx.listener(move |this, next, _, cx| {
                                        if let Some(target) =
                                            this.rows.iter_mut().find(|r| r.id == row_id)
                                        {
                                            target.enabled = *next;
                                            cx.emit(TableChangeEvent::Changed);
                                            cx.notify();
                                        }
                                    })),
                            )
                            .child(
                                Button::new(SharedString::from(format!("{prefix}-type-{row_id}")))
                                    .outline()
                                    .xsmall()
                                    .w(px(72.))
                                    .label(if is_file { "File" } else { "Text" })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if let Some(target) =
                                            this.rows.iter_mut().find(|r| r.id == row_id)
                                        {
                                            target.field_type = match target.field_type {
                                                MultipartFieldType::Text => MultipartFieldType::File,
                                                MultipartFieldType::File => MultipartFieldType::Text,
                                            };
                                            let next_ph = match target.field_type {
                                                MultipartFieldType::Text => "Text value",
                                                MultipartFieldType::File => "File path",
                                            };
                                            target.value.update(cx, |state, cx| {
                                                state.set_placeholder(next_ph, window, cx);
                                            });
                                            cx.emit(TableChangeEvent::Changed);
                                            cx.notify();
                                        }
                                    })),
                            )
                            .child(div().flex_1().min_w_0().child(Input::new(&row.name).small()))
                            .child(
                                h_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(div().flex_1().min_w_0().child(Input::new(&row.value).small()))
                                    .when(is_file, |this| {
                                        this.child(
                                            Button::new(SharedString::from(format!(
                                                "{prefix}-browse-{row_id}"
                                            )))
                                                .outline()
                                                .xsmall()
                                                .icon(LucideIcon::FolderOpen)
                                                .tooltip("Choose file…")
                                                .on_click(cx.listener(move |_, _, window, cx| {
                                                    let prompt = cx.prompt_for_paths(PathPromptOptions {
                                                        files: true,
                                                        directories: false,
                                                        multiple: false,
                                                        prompt: Some("Select multipart file".into()),
                                                    });
                                                    let entity = cx.entity();
                                                    cx.spawn_in(window, async move |_, window| {
                                                        let path = prompt.await.ok()?.ok()??.into_iter().next()?;
                                                        window
                                                            .update(|window, cx| {
                                                                entity.update(cx, |this, cx| {
                                                                    if let Some(target) = this.rows.iter().find(|r| r.id == row_id) {
                                                                        target.value.update(cx, |input, cx| {
                                                                            input.set_value(path.to_string_lossy().to_string(), window, cx);
                                                                        });
                                                                        cx.emit(TableChangeEvent::Changed);
                                                                        cx.notify();
                                                                    }
                                                                });
                                                            })
                                                            .ok()
                                                    })
                                                    .detach();
                                                })),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .w(px(140.))
                                    .child(Input::new(&row.content_type).small()),
                            )
                            .child(
                                Button::new(SharedString::from(format!("{prefix}-del-{row_id}")))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Delete)
                                    .tooltip("Remove field")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.rows.retain(|r| r.id != row_id);
                                        cx.emit(TableChangeEvent::Changed);
                                        cx.notify();
                                    })),
                            )
                    })),
            )
            .child(
                h_flex().pt_1().gap_2().child(
                    Button::new(format!("{prefix}-add-text"))
                        .outline()
                        .small()
                        .icon(IconName::Plus)
                        .label("Add Field")
                        .on_click(cx.listener(|this, _, window, cx| {
                            let row = this.build_row(&MultipartField::default(), window, cx);
                            this.rows.push(row);
                            cx.emit(TableChangeEvent::Changed);
                            cx.notify();
                        })),
                ),
            )
    }
}
