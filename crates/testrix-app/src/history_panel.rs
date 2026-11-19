use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    component::{
        ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState},
        list::ListItem,
        scroll::ScrollableElement as _,
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};
use testrix_core::RequestHistoryEntry;

use crate::{
    explorer::{DragPreview, DragSourceKind, DraggedRequest},
    theme_helpers::method_badge,
};

#[derive(Clone, Debug)]
pub(crate) enum HistoryPanelEvent {
    OpenEntry(RequestHistoryEntry),
    ClearHistory,
}

pub(crate) struct HistoryPanel {
    search_input: Entity<InputState>,
    entries: Vec<RequestHistoryEntry>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<HistoryPanelEvent> for HistoryPanel {}

impl HistoryPanel {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter history…"));
        let sub = cx.subscribe_in(
            &search_input,
            window,
            |_, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            },
        );
        Self {
            search_input,
            entries: Vec::new(),
            _subscriptions: vec![sub],
        }
    }

    pub(crate) fn set_entries(&mut self, entries: Vec<RequestHistoryEntry>, cx: &mut Context<Self>) {
        self.entries = entries;
        cx.notify();
    }
}

impl Render for HistoryPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.search_input.read(cx).value().trim().to_lowercase();
        let filtered: Vec<RequestHistoryEntry> = self
            .entries
            .iter()
            .filter(|entry| {
                if query.is_empty() {
                    return true;
                }
                let method = entry.doc.request.method.as_str().to_lowercase();
                let name = entry.doc.name.to_lowercase();
                let url = entry.doc.request.url.to_lowercase();
                method.contains(&query) || name.contains(&query) || url.contains(&query)
            })
            .cloned()
            .collect();

        let view_click = cx.entity();

        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .w_full()
                    .px_2()
                    .py_1p5()
                    .gap_1p5()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.search_input).small()),
                    )
                    .when(!self.entries.is_empty(), |row| {
                        row.child(
                            Button::new("history-clear-btn")
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(IconName::Delete))
                                .tooltip("Clear History")
                                .on_click(cx.listener(|_, _, _, cx| {
                                    cx.emit(HistoryPanelEvent::ClearHistory);
                                })),
                        )
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .when(filtered.is_empty(), |col| {
                        col.child(
                            div()
                                .p_4()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if self.entries.is_empty() {
                                    "No executed requests yet."
                                } else {
                                    "No matching history entries."
                                }),
                        )
                    })
                    .children(filtered.into_iter().enumerate().map(|(ix, entry)| {
                        let entry_click = entry.clone();
                        let view_c = view_click.clone();
                        let drag_payload = DraggedRequest {
                            source: DragSourceKind::History,
                            project_id: entry.project_id.clone(),
                            request_id: entry.request_id.clone(),
                            name: entry.doc.name.clone(),
                            method: entry.doc.request.method,
                            url: entry.doc.request.url.clone(),
                            snapshot_doc: Some(entry.doc.clone()),
                        };
                        let status_text = entry
                            .status_code
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| "ERR".to_string());
                        let time_short = short_timestamp(&entry.executed_at);
                        let subtitle = if entry.doc.request.url.trim().is_empty() {
                            format!("{status_text} · {time_short}")
                        } else {
                            format!("{} · {status_text} · {time_short}", entry.doc.request.url)
                        };

                        ListItem::new(SharedString::from(format!("hist-row-{ix}-{}", entry.id)))
                            .px_2()
                            .py_1()
                            .child(
                                v_flex()
                                    .w_full()
                                    .min_w_0()
                                    .gap_0p5()
                                    .child(
                                        h_flex()
                                            .w_full()
                                            .min_w_0()
                                            .gap_1p5()
                                            .items_center()
                                            .child(method_badge(entry.doc.request.method, true))
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .text_xs()
                                                    .font_medium()
                                                    .overflow_hidden()
                                                    .text_ellipsis()
                                                    .whitespace_nowrap()
                                                    .child(entry.doc.name.clone()),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .w_full()
                                            .min_w_0()
                                            .pl(px(2.))
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .overflow_hidden()
                                            .text_ellipsis()
                                            .whitespace_nowrap()
                                            .child(subtitle),
                                    ),
                            )
                            .on_click(move |_, _, cx| {
                                view_c.update(cx, |_, cx| {
                                    cx.emit(HistoryPanelEvent::OpenEntry(entry_click.clone()));
                                });
                            })
                            .on_drag(drag_payload, |drag, _, _, cx| {
                                cx.new(|_| DragPreview(drag.name.clone()))
                            })
                    })),
            )
    }
}

fn short_timestamp(ts: &str) -> String {
    if let Some(t_pos) = ts.find('T') {
        let rest = &ts[t_pos + 1..];
        let hhmmss: String = rest.chars().take(8).collect();
        let date: String = ts[..t_pos].chars().skip(5).collect();
        return format!("{date} {hhmmss}");
    }
    ts.to_string()
}
