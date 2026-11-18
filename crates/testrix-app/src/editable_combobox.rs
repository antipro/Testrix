use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    MouseButton, ParentElement as _, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, TestSupportExt as _, Window,
    base::input::{Escape, IndentInline, MoveDown, MoveUp},
    component::{
        ActiveTheme as _, IconName, Sizable as _, ThemeStyled as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState},
        list::ListItem,
        v_flex,
    },
    deferred, div,
    prelude::FluentBuilder as _,
    px, relative,
};
use testrix_core::EnvironmentDoc;

use crate::theme_helpers::apply_unresolved_var_border;

/// A selectable suggestion displayed in an [`EditableCombobox`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ComboboxOption {
    pub value: String,
    pub detail: Option<String>,
}

impl ComboboxOption {
    pub(crate) fn new(value: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            detail: None,
        }
    }

    pub(crate) fn with_detail(mut self, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        if !detail.is_empty() {
            self.detail = Some(detail);
        }
        self
    }
}

#[derive(Debug, Clone)]
pub(crate) enum EditableComboboxEvent {
    Change,
}

/// Editable text input paired with a filterable suggestion dropdown.
///
/// Users can either type an arbitrary string or pick a suggested entry from the dropdown.
pub(crate) struct EditableCombobox {
    id: SharedString,
    input: Entity<InputState>,
    options: Vec<ComboboxOption>,
    open: bool,
    show_all: bool,
    selected_ix: usize,
    small: bool,
    cleanable: bool,
    validate_variables: bool,
    active_env: Option<EnvironmentDoc>,
    menu_width: Pixels,
    _subscription: Subscription,
}

impl EventEmitter<EditableComboboxEvent> for EditableCombobox {}

impl EditableCombobox {
    pub(crate) fn new(
        id: impl Into<SharedString>,
        placeholder: impl Into<SharedString>,
        default_value: impl Into<String>,
        options: Vec<ComboboxOption>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let placeholder: SharedString = placeholder.into();
        let default_value: String = default_value.into();
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(default_value)
        });

        let subscription = cx.subscribe_in(&input, window, Self::on_input_event);

        Self {
            id: id.into(),
            input,
            options,
            open: false,
            show_all: false,
            selected_ix: 0,
            small: false,
            cleanable: false,
            validate_variables: false,
            active_env: None,
            menu_width: px(280.),
            _subscription: subscription,
        }
    }

    pub(crate) fn small(mut self, small: bool) -> Self {
        self.small = small;
        self
    }

    pub(crate) fn cleanable(mut self, cleanable: bool) -> Self {
        self.cleanable = cleanable;
        self
    }

    pub(crate) fn menu_width(mut self, width: Pixels) -> Self {
        self.menu_width = width;
        self
    }

    pub(crate) fn validate_variables(mut self, validate: bool) -> Self {
        self.validate_variables = validate;
        self
    }

    pub(crate) fn set_active_env(
        &mut self,
        env: Option<EnvironmentDoc>,
        cx: &mut Context<Self>,
    ) {
        self.active_env = env;
        cx.notify();
    }

    pub(crate) fn rename_variables(
        &mut self,
        renames: &[(String, String)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let current = self.value(cx);
        if let Some(updated) = testrix_core::rename_template_variables(&current, renames) {
            self.input.update(cx, |state, cx| {
                state.set_value(updated, window, cx);
            });
            cx.notify();
            return true;
        }
        false
    }

    pub(crate) fn value(&self, cx: &App) -> String {
        self.input.read(cx).value().to_string()
    }

    pub(crate) fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = placeholder.into();
        self.input.update(cx, |state, cx| {
            state.set_placeholder(placeholder, window, cx)
        });
    }

    pub(crate) fn set_options(&mut self, options: Vec<ComboboxOption>, cx: &mut Context<Self>) {
        self.options = options;
        let filtered_len = self.filtered_options(cx).len();
        if filtered_len == 0 {
            self.selected_ix = 0;
        } else {
            self.selected_ix = self.selected_ix.min(filtered_len - 1);
        }
        cx.notify();
    }

    fn on_input_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                self.show_all = false;
                let has_matches = !self.filtered_options(cx).is_empty();
                self.open = has_matches && !self.options.is_empty();
                self.selected_ix = 0;
                cx.emit(EditableComboboxEvent::Change);
                cx.notify();
            }
            InputEvent::PressEnter {
                secondary: false, ..
            } => {
                if self.open {
                    self.accept_selected(window, cx);
                }
            }
            InputEvent::Blur if self.open => {
                self.open = false;
                cx.notify();
            }
            _ => {}
        }
    }

    fn filtered_options(&self, cx: &App) -> Vec<ComboboxOption> {
        let query = self.input.read(cx).value().trim().to_ascii_lowercase();
        if self.show_all || query.is_empty() {
            return self.options.clone();
        }

        let (mut starts, mut contains): (Vec<ComboboxOption>, Vec<ComboboxOption>) = self
            .options
            .iter()
            .filter(|opt| {
                opt.value.to_ascii_lowercase().contains(&query)
                    || opt
                        .detail
                        .as_deref()
                        .is_some_and(|d| d.to_ascii_lowercase().contains(&query))
            })
            .cloned()
            .partition(|opt| opt.value.to_ascii_lowercase().starts_with(&query));

        starts.append(&mut contains);
        starts
    }

    fn toggle_dropdown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.options.is_empty() {
            self.open = false;
            return;
        }
        self.open = !self.open;
        if self.open {
            self.show_all = true;
            self.selected_ix = 0;
            self.input.update(cx, |state, cx| state.focus(window, cx));
        }
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.filtered_options(cx).len();
        if !self.open || len == 0 {
            return;
        }
        self.selected_ix = (self.selected_ix as isize + delta).rem_euclid(len as isize) as usize;
        cx.notify();
    }

    fn accept_value(&mut self, value: String, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |state, cx| {
            state.set_value(value, window, cx);
            state.focus(window, cx);
        });
        self.open = false;
        self.show_all = false;
        cx.emit(EditableComboboxEvent::Change);
        cx.notify();
    }

    fn accept_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let matches = self.filtered_options(cx);
        if let Some(opt) = matches.get(self.selected_ix) {
            self.accept_value(opt.value.clone(), window, cx);
        } else {
            self.open = false;
            cx.notify();
        }
    }
}

impl Render for EditableCombobox {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let matches = if self.open {
            self.filtered_options(cx)
        } else {
            Vec::new()
        };
        let show_menu = self.open && !self.options.is_empty();
        let toggle_id = SharedString::from(format!("{}-toggle", self.id));
        let menu_id = SharedString::from(format!("{}-menu", self.id));
        let wrap_id = SharedString::from(format!("{}-wrap", self.id));
        let toggle_icon = if show_menu {
            IconName::ChevronUp
        } else {
            IconName::ChevronDown
        };

        div()
            .id(wrap_id)
            .relative()
            .w_full()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.open {
                    this.open = false;
                    cx.notify();
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                this.move_selection(-1, cx);
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                this.move_selection(1, cx);
            }))
            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                if this.open {
                    this.open = false;
                    cx.notify();
                }
            }))
            .capture_action(cx.listener(|this, _: &IndentInline, window, cx| {
                if this.open {
                    this.accept_selected(window, cx);
                }
            }))
            .child({
                let input = Input::new(&self.input)
                    .id(self.id.clone())
                    .w_full()
                    .when(self.small, |i| i.small())
                    .cleanable(self.cleanable)
                    .suffix(
                        Button::new(toggle_id)
                            .ghost()
                            .xsmall()
                            .icon(toggle_icon)
                            .tooltip("Select suggestion")
                            .on_click(cx.listener(|this, _, window, cx| {
                                cx.stop_propagation();
                                this.toggle_dropdown(window, cx);
                            })),
                    );
                if self.validate_variables {
                    let text = self.input.read(cx).value().to_string();
                    apply_unresolved_var_border(input, &text, self.active_env.as_ref(), cx)
                } else {
                    input
                }
            })
            .when(show_menu, |this| {
                this.child(deferred(
                    div().absolute().left_0().top(relative(1.)).pt_1().child(
                        v_flex()
                            .id(menu_id)
                            .test_support()
                            .w(self.menu_width)
                            .max_h(px(220.))
                            .overflow_y_scroll()
                            .p_1()
                            .gap_y_0p5()
                            .popover_style(cx)
                            .when(matches.is_empty(), |menu| {
                                menu.child(
                                    div()
                                        .px_2()
                                        .py_1p5()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("No matching suggestions"),
                                )
                            })
                            .children(matches.into_iter().enumerate().map(|(ix, opt)| {
                                let val = opt.value.clone();
                                let detail = opt.detail.clone();
                                let opt_id = SharedString::from(format!("{}-opt-{ix}", self.id));
                                ListItem::new(opt_id)
                                    .rounded(cx.theme().radius)
                                    .px_2()
                                    .py_1()
                                    .selected(ix == self.selected_ix)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.accept_value(val.clone(), window, cx);
                                        }),
                                    )
                                    .child(
                                        h_flex()
                                            .w_full()
                                            .justify_between()
                                            .gap_2()
                                            .items_center()
                                            .child(div().text_xs().truncate().child(opt.value))
                                            .when_some(detail, |row, d| {
                                                row.child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .truncate()
                                                        .child(d),
                                                )
                                            }),
                                    )
                            })),
                    ),
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        Bounds, Point, TestAppContext, WindowBounds, WindowOptions, px, size,
        test::TestWindowExt as _,
    };

    use super::*;

    #[gpui_kit::test]
    fn clicking_dropdown_option_changes_value_and_closes_menu(cx: &mut TestAppContext) {
        cx.update(gpui_kit::component::init);

        let options = vec![
            ComboboxOption::new("{{base_url}}").with_detail("Project baseUrl"),
            ComboboxOption::new("{{base_url_emr}}").with_detail("http://192.168.1.231:4433/emr"),
        ];

        let (window, combo) = cx.update(|cx| {
            let bounds = Bounds {
                origin: Point::default(),
                size: size(px(600.), px(400.)),
            };
            let (window, content) = gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        EditableCombobox::new(
                            "url-prefix",
                            "{{base_url}}",
                            "{{base_url}}",
                            options,
                            window,
                            cx,
                        )
                    })
                },
            )
            .expect("open test window");
            (
                window
                    .downcast::<gpui_kit::base::Root>()
                    .expect("Base Root"),
                content,
            )
        });

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);

            // Open the dropdown via the chevron toggle button.
            window.click("url-prefix-toggle", cx);
            window.render_frame(cx);

            assert!(
                window.try_find("url-prefix-menu").is_some(),
                "dropdown menu should open after clicking toggle"
            );

            // Click the second option ({{base_url_emr}}).
            window.click("url-prefix-opt-1", cx);
            window.render_frame(cx);
        })
        .unwrap();

        cx.run_until_parked();

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
        })
        .unwrap();

        cx.update(|cx| {
            let combo_ref = combo.read(cx);
            assert_eq!(combo_ref.value(cx), "{{base_url_emr}}");
            assert!(!combo_ref.open, "dropdown menu should close after selecting");
        });
    }
}
