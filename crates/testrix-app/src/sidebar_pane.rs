use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window,
    assets::IconName as LucideIcon,
    component::{
        ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        menu::{DropdownMenu as _, PopupMenuItem},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};
use testrix_core::{RequestHistoryEntry, RequestSet, WorkspaceTree};

use crate::{
    explorer::{ExplorerEvent, ExplorerPanel},
    history_panel::{HistoryPanel, HistoryPanelEvent},
    sets_panel::{SetsPanel, SetsPanelEvent},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SidebarViewMode {
    Explorer,
    History,
    Set,
}

impl SidebarViewMode {
    pub(crate) const ALL: [Self; 3] = [Self::Explorer, Self::History, Self::Set];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Explorer => "Explorer",
            Self::History => "History",
            Self::Set => "Package",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) enum SidebarPaneEvent {
    Explorer(ExplorerEvent),
    History(HistoryPanelEvent),
    Sets(SetsPanelEvent),
    SplitRequested(String),
    CloseRequested(String),
}

pub(crate) struct SidebarPane {
    pub(crate) id: String,
    pub(crate) mode: SidebarViewMode,
    pub(crate) explorer: Entity<ExplorerPanel>,
    pub(crate) history: Entity<HistoryPanel>,
    pub(crate) sets: Entity<SetsPanel>,
    can_close: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SidebarPaneEvent> for SidebarPane {}

impl SidebarPane {
    pub(crate) fn new(
        id: String,
        initial_mode: SidebarViewMode,
        can_close: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let explorer = cx.new(|cx| ExplorerPanel::new(window, cx));
        let history = cx.new(|cx| HistoryPanel::new(window, cx));
        let sets = cx.new(|cx| SetsPanel::new(window, cx));

        let mut subs = Vec::new();
        subs.push(cx.subscribe(&explorer, |_, _, event: &ExplorerEvent, cx| {
            cx.emit(SidebarPaneEvent::Explorer(event.clone()));
        }));
        subs.push(cx.subscribe(&history, |_, _, event: &HistoryPanelEvent, cx| {
            cx.emit(SidebarPaneEvent::History(event.clone()));
        }));
        subs.push(cx.subscribe(&sets, |_, _, event: &SetsPanelEvent, cx| {
            cx.emit(SidebarPaneEvent::Sets(event.clone()));
        }));

        Self {
            id,
            mode: initial_mode,
            explorer,
            history,
            sets,
            can_close,
            _subscriptions: subs,
        }
    }

    pub(crate) fn set_can_close(&mut self, can_close: bool, cx: &mut Context<Self>) {
        if self.can_close != can_close {
            self.can_close = can_close;
            cx.notify();
        }
    }

    pub(crate) fn sync_data(
        &mut self,
        workspace: &WorkspaceTree,
        history: &[RequestHistoryEntry],
        sets: &[RequestSet],
        expanded_nodes: Option<&[String]>,
        cx: &mut Context<Self>,
    ) {
        self.explorer.update(cx, |panel, cx| {
            if let Some(ids) = expanded_nodes {
                panel.set_expanded_ids(ids.to_vec());
            }
            panel.set_workspace(workspace.clone(), cx);
        });
        self.history.update(cx, |panel, cx| {
            panel.set_entries(history.to_vec(), cx);
        });
        self.sets.update(cx, |panel, cx| {
            panel.set_data(sets.to_vec(), workspace, cx);
        });
    }
}

impl Render for SidebarPane {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane_id_split = self.id.clone();
        let pane_id_close = self.id.clone();
        let current_mode = self.mode;
        let view = cx.entity();
        let pane_id = self.id.clone();
        let explorer_count = self.explorer.read(cx).total_requests();
        let header_actions = match current_mode {
            SidebarViewMode::Explorer => Some(
                self.explorer
                    .update(cx, |p, cx| p.render_header_actions(&pane_id, cx)),
            ),
            SidebarViewMode::Set => Some(
                self.sets
                    .update(cx, |p, cx| p.render_header_actions(&pane_id, cx)),
            ),
            SidebarViewMode::History => None,
        };

        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().sidebar)
            .border_l_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .w_full()
                    .px_2p5()
                    .py_1p5()
                    .gap_1p5()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Icon::new(match current_mode {
                                    SidebarViewMode::Explorer => LucideIcon::FolderTree,
                                    SidebarViewMode::History => LucideIcon::Clock,
                                    SidebarViewMode::Set => LucideIcon::Layers,
                                })
                                .small(),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(current_mode.label()),
                            )
                            .when(current_mode == SidebarViewMode::Explorer, |row| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!("({explorer_count})")),
                                )
                            })
                            .child(
                                Button::new(SharedString::from(format!(
                                    "sidebar-view-menu-{}",
                                    self.id
                                )))
                                .ghost()
                                .xsmall()
                                .icon(LucideIcon::CircleChevronDown)
                                .tooltip("Switch view")
                                .dropdown_menu(move |mut menu, _, _| {
                                    for mode in SidebarViewMode::ALL {
                                        let v = view.clone();
                                        menu = menu.item(
                                            PopupMenuItem::new(mode.label())
                                                .checked(mode == current_mode)
                                                .on_click(move |_, _, cx| {
                                                    v.update(cx, |this, cx| {
                                                        this.mode = mode;
                                                        cx.notify();
                                                    });
                                                }),
                                        );
                                    }
                                    menu
                                }),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_0p5()
                            .items_center()
                            .when_some(header_actions, |row, actions| {
                                row.child(actions)
                                    .child(div().w(px(1.)).h(px(14.)).mx_1().bg(cx.theme().border))
                            })
                            .child(
                                Button::new(SharedString::from(format!(
                                    "sidebar-split-{}",
                                    self.id
                                )))
                                .ghost()
                                .xsmall()
                                .icon(LucideIcon::SeparatorHorizontal)
                                .tooltip("Split pane vertically")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(SidebarPaneEvent::SplitRequested(
                                        pane_id_split.clone(),
                                    ));
                                })),
                            )
                            .when(self.can_close, |row| {
                                row.child(
                                    Button::new(SharedString::from(format!(
                                        "sidebar-close-{}",
                                        self.id
                                    )))
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(IconName::Close))
                                    .tooltip("Close pane")
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(SidebarPaneEvent::CloseRequested(
                                            pane_id_close.clone(),
                                        ));
                                    })),
                                )
                            }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(match self.mode {
                        SidebarViewMode::Explorer => self.explorer.clone().into_any_element(),
                        SidebarViewMode::History => self.history.clone().into_any_element(),
                        SidebarViewMode::Set => self.sets.clone().into_any_element(),
                    }),
            )
    }
}
