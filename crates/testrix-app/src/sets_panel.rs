use std::collections::{HashMap, HashSet};

use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window,
    assets::IconName as LucideIcon,
    component::{
        ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState},
        list::ListItem,
        menu::{ContextMenuExt as _, PopupMenuItem},
        scroll::ScrollableElement as _,
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};
use testrix_core::{
    FolderNode, HttpMethod, RequestSet, RequestSetFolder, RequestSetItem, WorkspaceTree,
};

use crate::{
    explorer::{DragPreview, DragSourceKind, DraggedRequest},
    theme_helpers::method_badge,
};

#[derive(Clone, Debug)]
pub(crate) enum SetsPanelEvent {
    CreateSet,
    RenameSet {
        set_id: String,
    },
    DeleteSet {
        set_id: String,
    },
    OpenSetProperties {
        set_id: String,
    },
    CreateFolder {
        set_id: String,
        parent_folder_id: Option<String>,
    },
    RenameFolder {
        set_id: String,
        folder_id: String,
    },
    DeleteFolder {
        set_id: String,
        folder_id: String,
    },
    OpenRequestRef {
        project_id: String,
        request_id: String,
    },
    RemoveItem {
        set_id: String,
        item_id: String,
    },
    AddRequestRef {
        target_set_id: String,
        target_folder_id: Option<String>,
        project_id: String,
        request_id: String,
        name: String,
        method: HttpMethod,
        url: String,
    },
    MoveItem {
        source_set_id: String,
        item_id: String,
        target_set_id: String,
        target_folder_id: Option<String>,
    },
}

pub(crate) struct SetsPanel {
    search_input: Entity<InputState>,
    sets: Vec<RequestSet>,
    /// Live lookup `(project_id, request_id) -> (name, method, url)` from WorkspaceTree.
    live_requests: HashMap<(String, String), (String, HttpMethod, String)>,
    expanded_ids: HashSet<String>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SetsPanelEvent> for SetsPanel {}

impl SetsPanel {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter packages…"));
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
            sets: Vec::new(),
            live_requests: HashMap::new(),
            expanded_ids: HashSet::new(),
            _subscriptions: vec![sub],
        }
    }

    pub(crate) fn render_header_actions(
        &self,
        id_suffix: &str,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        Button::new(SharedString::from(format!("packages-new-btn-{id_suffix}")))
            .ghost()
            .xsmall()
            .icon(Icon::new(IconName::Plus))
            .tooltip("New Package")
            .on_click(cx.listener(|_, _, _, cx| {
                cx.emit(SetsPanelEvent::CreateSet);
            }))
            .into_any_element()
    }

    pub(crate) fn set_data(
        &mut self,
        sets: Vec<RequestSet>,
        workspace: &WorkspaceTree,
        cx: &mut Context<Self>,
    ) {
        self.sets = sets;
        self.live_requests.clear();
        for proj in &workspace.projects {
            for req in &proj.requests {
                self.live_requests.insert(
                    (proj.doc.id.clone(), req.doc.id.clone()),
                    (
                        req.doc.name.clone(),
                        req.doc.request.method,
                        req.doc.request.url.clone(),
                    ),
                );
            }
            collect_live_folder_requests(&proj.doc.id, &proj.folders, &mut self.live_requests);
        }
        cx.notify();
    }

    fn toggle_expand(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.expanded_ids.contains(id) {
            self.expanded_ids.remove(id);
        } else {
            self.expanded_ids.insert(id.to_string());
        }
        cx.notify();
    }

    fn collect_set_rows(
        &self,
        set: &RequestSet,
        query: &str,
        out: &mut Vec<gpui_kit::AnyElement>,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity();
        let is_expanded = !query.is_empty() || self.expanded_ids.contains(&set.id);
        let set_id = set.id.clone();
        let set_id_drop = set.id.clone();
        let set_id_menu = set.id.clone();
        let set_id_toggle = set.id.clone();
        let set_id_props = set.id.clone();

        let has_children = !set.folders.is_empty() || !set.requests.is_empty();
        let matches_self = query.is_empty() || set.name.to_lowercase().contains(query);
        let has_matching_descendant = query.is_empty()
            || set.requests.iter().any(|r| self.item_matches(r, query))
            || set.folders.iter().any(|f| self.folder_matches(f, query));
        if !matches_self && !has_matching_descendant {
            return;
        }

        let row_ix = out.len();
        let view_t = view.clone();
        let view_m = view.clone();
        let view_d = view.clone();
        let view_p = view.clone();

        let set_row = ListItem::new(SharedString::from(format!("set-row-{row_ix}-{}", set.id)))
            .px_1p5()
            .py_1()
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap_1p5()
                    .child(
                        div()
                            .w(px(14.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(has_children, |d| {
                                d.child(
                                    Icon::new(if is_expanded {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    })
                                    .size(px(12.))
                                    .text_color(cx.theme().muted_foreground),
                                )
                            }),
                    )
                    .child(
                        Icon::new(LucideIcon::Layers)
                            .size(px(14.))
                            .text_color(cx.theme().primary),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .font_semibold()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(set.name.clone()),
                    )
                    .child(
                        Button::new(SharedString::from(format!("set-props-btn-{row_ix}-{}", set.id)))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(LucideIcon::Workflow))
                            .tooltip("Package Properties & DAG")
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                let sid = set_id_props.clone();
                                view_p.update(cx, |_, cx| {
                                    cx.emit(SetsPanelEvent::OpenSetProperties { set_id: sid });
                                });
                            }),
                    ),
            )
            .on_click(move |_, _, cx| {
                view_t.update(cx, |this, cx| {
                    this.toggle_expand(&set_id_toggle, cx);
                });
            })
            .drag_over::<DraggedRequest>(|style, _, _, cx| {
                style
                    .bg(cx.theme().accent)
                    .border_1()
                    .border_color(cx.theme().primary)
            })
            .on_drop(move |drag: &DraggedRequest, _, cx| {
                let target_sid = set_id_drop.clone();
                let drag = drag.clone();
                view_d.update(cx, |this, cx| {
                    this.expanded_ids.insert(target_sid.clone());
                    emit_drop_into_set(&drag, target_sid, None, cx);
                });
            })
            .context_menu(move |menu, _, _| {
                let s0 = set_id_menu.clone();
                let s1 = set_id_menu.clone();
                let s2 = set_id_menu.clone();
                let s3 = set_id_menu.clone();
                let v0 = view_m.clone();
                let v1 = view_m.clone();
                let v2 = view_m.clone();
                let v3 = view_m.clone();
                menu.item(
                    PopupMenuItem::new("Properties")
                        .icon(LucideIcon::Workflow)
                        .on_click(move |_, _, cx| {
                            v0.update(cx, |_, cx| {
                                cx.emit(SetsPanelEvent::OpenSetProperties { set_id: s0.clone() });
                            });
                        }),
                )
                .separator()
                .item(PopupMenuItem::new("New Folder").on_click(move |_, _, cx| {
                    v1.update(cx, |_, cx| {
                        cx.emit(SetsPanelEvent::CreateFolder {
                            set_id: s1.clone(),
                            parent_folder_id: None,
                        });
                    });
                }))
                .item(PopupMenuItem::new("Rename Package").on_click(move |_, _, cx| {
                    v2.update(cx, |_, cx| {
                        cx.emit(SetsPanelEvent::RenameSet { set_id: s2.clone() });
                    });
                }))
                .separator()
                .item(PopupMenuItem::new("Delete Package").on_click(move |_, _, cx| {
                    v3.update(cx, |_, cx| {
                        cx.emit(SetsPanelEvent::DeleteSet { set_id: s3.clone() });
                    });
                }))
            })
            .into_any_element();

        out.push(set_row);

        if is_expanded {
            for folder in &set.folders {
                self.collect_folder_rows(&set_id, folder, 1, query, out, cx);
            }
            for item in &set.requests {
                self.collect_item_row(&set_id, item, 1, query, out, cx);
            }
        }
    }

    fn collect_folder_rows(
        &self,
        set_id: &str,
        folder: &RequestSetFolder,
        depth: usize,
        query: &str,
        out: &mut Vec<gpui_kit::AnyElement>,
        cx: &mut Context<Self>,
    ) {
        if !query.is_empty() && !self.folder_matches(folder, query) {
            return;
        }
        let view = cx.entity();
        let is_expanded = !query.is_empty() || self.expanded_ids.contains(&folder.id);
        let has_children = !folder.folders.is_empty() || !folder.requests.is_empty();
        let row_ix = out.len();
        let indent = px((depth as f32) * 14.0 + 6.0);

        let fid_toggle = folder.id.clone();
        let sid_menu = set_id.to_string();
        let fid_menu = folder.id.clone();
        let sid_drop = set_id.to_string();
        let fid_drop = folder.id.clone();
        let view_t = view.clone();
        let view_m = view.clone();
        let view_d = view.clone();

        let folder_row = ListItem::new(SharedString::from(format!(
            "setf-row-{row_ix}-{}",
            folder.id
        )))
        .px_1p5()
        .py_0p5()
        .pl(indent)
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .items_center()
                .gap_1p5()
                .child(
                    div()
                        .w(px(14.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(has_children, |d| {
                            d.child(
                                Icon::new(if is_expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(px(12.))
                                .text_color(cx.theme().muted_foreground),
                            )
                        }),
                )
                .child(
                    Icon::new(if is_expanded {
                        IconName::FolderOpen
                    } else {
                        IconName::Folder
                    })
                    .size(px(14.))
                    .text_color(cx.theme().muted_foreground),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .font_medium()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(folder.name.clone()),
                ),
        )
        .on_click(move |_, _, cx| {
            view_t.update(cx, |this, cx| {
                this.toggle_expand(&fid_toggle, cx);
            });
        })
        .drag_over::<DraggedRequest>(|style, _, _, cx| {
            style
                .bg(cx.theme().accent)
                .border_1()
                .border_color(cx.theme().primary)
        })
        .on_drop(move |drag: &DraggedRequest, _, cx| {
            let target_sid = sid_drop.clone();
            let target_fid = fid_drop.clone();
            let drag = drag.clone();
            view_d.update(cx, |this, cx| {
                this.expanded_ids.insert(target_sid.clone());
                this.expanded_ids.insert(target_fid.clone());
                emit_drop_into_set(&drag, target_sid, Some(target_fid), cx);
            });
        })
        .context_menu(move |menu, _, _| {
            let s1 = sid_menu.clone();
            let f1 = fid_menu.clone();
            let s2 = sid_menu.clone();
            let f2 = fid_menu.clone();
            let s3 = sid_menu.clone();
            let f3 = fid_menu.clone();
            let v1 = view_m.clone();
            let v2 = view_m.clone();
            let v3 = view_m.clone();
            menu.item(PopupMenuItem::new("New Subfolder").on_click(move |_, _, cx| {
                v1.update(cx, |_, cx| {
                    cx.emit(SetsPanelEvent::CreateFolder {
                        set_id: s1.clone(),
                        parent_folder_id: Some(f1.clone()),
                    });
                });
            }))
            .item(PopupMenuItem::new("Rename Folder").on_click(move |_, _, cx| {
                v2.update(cx, |_, cx| {
                    cx.emit(SetsPanelEvent::RenameFolder {
                        set_id: s2.clone(),
                        folder_id: f2.clone(),
                    });
                });
            }))
            .separator()
            .item(PopupMenuItem::new("Delete Folder").on_click(move |_, _, cx| {
                v3.update(cx, |_, cx| {
                    cx.emit(SetsPanelEvent::DeleteFolder {
                        set_id: s3.clone(),
                        folder_id: f3.clone(),
                    });
                });
            }))
        })
        .into_any_element();

        out.push(folder_row);

        if is_expanded {
            for sub in &folder.folders {
                self.collect_folder_rows(set_id, sub, depth + 1, query, out, cx);
            }
            for item in &folder.requests {
                self.collect_item_row(set_id, item, depth + 1, query, out, cx);
            }
        }
    }

    fn collect_item_row(
        &self,
        set_id: &str,
        item: &RequestSetItem,
        depth: usize,
        query: &str,
        out: &mut Vec<gpui_kit::AnyElement>,
        cx: &mut Context<Self>,
    ) {
        if !query.is_empty() && !self.item_matches(item, query) {
            return;
        }
        let live = self
            .live_requests
            .get(&(item.project_id.clone(), item.request_id.clone()));
        let is_missing = live.is_none();
        let (name, method, url) = match live {
            Some((n, m, u)) => (n.clone(), *m, u.clone()),
            None => (item.name.clone(), item.method, item.url.clone()),
        };

        let row_ix = out.len();
        let indent = px((depth as f32) * 14.0 + 20.0);
        let view = cx.entity();
        let view_c = view.clone();
        let view_m = view.clone();

        let pid_open = item.project_id.clone();
        let rid_open = item.request_id.clone();
        let sid_rm = set_id.to_string();
        let iid_rm = item.id.clone();

        let drag_payload = DraggedRequest {
            source: DragSourceKind::Set {
                set_id: set_id.to_string(),
                item_id: item.id.clone(),
            },
            project_id: Some(item.project_id.clone()),
            request_id: Some(item.request_id.clone()),
            name: name.clone(),
            method,
            url,
            snapshot_doc: None,
        };

        let pid_menu = item.project_id.clone();
        let rid_menu = item.request_id.clone();

        let row = ListItem::new(SharedString::from(format!("seti-row-{row_ix}-{}", item.id)))
            .px_1p5()
            .py_0p5()
            .pl(indent)
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap_1p5()
                    .child(method_badge(method, true))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .when(is_missing, |d| d.text_color(cx.theme().muted_foreground))
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(if is_missing {
                                format!("{name} (missing)")
                            } else {
                                name
                            }),
                    ),
            )
            .on_click(move |_, _, cx| {
                let p = pid_open.clone();
                let r = rid_open.clone();
                view_c.update(cx, |_, cx| {
                    cx.emit(SetsPanelEvent::OpenRequestRef {
                        project_id: p,
                        request_id: r,
                    });
                });
            })
            .on_drag(drag_payload, |drag, _, _, cx| {
                cx.new(|_| DragPreview(drag.name.clone()))
            })
            .context_menu(move |menu, _, _| {
                let p = pid_menu.clone();
                let r = rid_menu.clone();
                let s = sid_rm.clone();
                let i = iid_rm.clone();
                let v1 = view_m.clone();
                let v2 = view_m.clone();
                menu.item(PopupMenuItem::new("Open Request").on_click(move |_, _, cx| {
                    v1.update(cx, |_, cx| {
                        cx.emit(SetsPanelEvent::OpenRequestRef {
                            project_id: p.clone(),
                            request_id: r.clone(),
                        });
                    });
                }))
                .separator()
                .item(PopupMenuItem::new("Remove from Package").on_click(move |_, _, cx| {
                    v2.update(cx, |_, cx| {
                        cx.emit(SetsPanelEvent::RemoveItem {
                            set_id: s.clone(),
                            item_id: i.clone(),
                        });
                    });
                }))
            })
            .into_any_element();

        out.push(row);
    }

    fn item_matches(&self, item: &RequestSetItem, query: &str) -> bool {
        let (name, method, url) = self
            .live_requests
            .get(&(item.project_id.clone(), item.request_id.clone()))
            .cloned()
            .unwrap_or_else(|| (item.name.clone(), item.method, item.url.clone()));
        name.to_lowercase().contains(query)
            || method.as_str().to_lowercase().contains(query)
            || url.to_lowercase().contains(query)
    }

    fn folder_matches(&self, folder: &RequestSetFolder, query: &str) -> bool {
        folder.name.to_lowercase().contains(query)
            || folder.requests.iter().any(|r| self.item_matches(r, query))
            || folder.folders.iter().any(|f| self.folder_matches(f, query))
    }
}

impl Render for SetsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.search_input.read(cx).value().trim().to_lowercase();
        let mut rows = Vec::new();
        for set in &self.sets {
            self.collect_set_rows(set, &query, &mut rows, cx);
        }

        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().sidebar)
            .child(
                div()
                    .w_full()
                    .px_2p5()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(Input::new(&self.search_input).small()),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .when(rows.is_empty(), |col| {
                        col.child(
                            div()
                                .p_4()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if self.sets.is_empty() {
                                    "No packages yet. Click + to create a Package, then drag requests here."
                                } else {
                                    "No matching items."
                                }),
                        )
                    })
                    .children(rows),
            )
    }
}

fn collect_live_folder_requests(
    project_id: &str,
    folders: &[FolderNode],
    out: &mut HashMap<(String, String), (String, HttpMethod, String)>,
) {
    for folder in folders {
        for req in &folder.requests {
            out.insert(
                (project_id.to_string(), req.doc.id.clone()),
                (
                    req.doc.name.clone(),
                    req.doc.request.method,
                    req.doc.request.url.clone(),
                ),
            );
        }
        collect_live_folder_requests(project_id, &folder.folders, out);
    }
}

fn emit_drop_into_set(
    drag: &DraggedRequest,
    target_set_id: String,
    target_folder_id: Option<String>,
    cx: &mut Context<SetsPanel>,
) {
    match &drag.source {
        DragSourceKind::Set {
            set_id: source_set_id,
            item_id,
        } => {
            cx.emit(SetsPanelEvent::MoveItem {
                source_set_id: source_set_id.clone(),
                item_id: item_id.clone(),
                target_set_id,
                target_folder_id,
            });
        }
        DragSourceKind::Explorer | DragSourceKind::History => {
            if let (Some(project_id), Some(request_id)) =
                (drag.project_id.clone(), drag.request_id.clone())
            {
                cx.emit(SetsPanelEvent::AddRequestRef {
                    target_set_id,
                    target_folder_id,
                    project_id,
                    request_id,
                    name: drag.name.clone(),
                    method: drag.method,
                    url: drag.url.clone(),
                });
            }
        }
    }
}
