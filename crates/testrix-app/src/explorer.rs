use std::collections::{HashMap, HashSet};

use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, TestSupportExt as _,
    Window,
    assets::IconName as LucideIcon,
    component::{
        ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState},
        list::ListItem,
        menu::{DropdownMenu as _, PopupMenuItem},
        tree::{TreeEvent, TreeItem, TreeState, tree},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};
use testrix_core::{
    ExplorerNode, FilteredFolder, FilteredProject, HttpMethod, RequestNode, WorkspaceTree,
    filter_workspace,
};

use crate::theme_helpers::method_badge;

#[derive(Debug, Clone)]
pub(crate) enum ExplorerEvent {
    OpenNode(ExplorerNode),
    NewRequestIn {
        project_id: String,
        folder_id: Option<String>,
    },
    NewFolderIn {
        project_id: String,
        parent_folder_id: Option<String>,
    },
    NewProject,
    DuplicateRequest {
        project_id: String,
        request_id: String,
    },
    DeleteRequest {
        project_id: String,
        request_id: String,
    },
    DeleteFolder {
        project_id: String,
        folder_id: String,
    },
    DeleteProject {
        project_id: String,
    },
    ImportOpenApi,
    ImportPostmanZip,
    ImportApifox,
    ReloadWorkspace,
}

#[derive(Debug, Clone)]
enum NodeMeta {
    Project {
        project_id: String,
        name: String,
        has_auth: bool,
        request_count: usize,
        has_children: bool,
    },
    Folder {
        project_id: String,
        folder_id: String,
        name: String,
        has_auth: bool,
        has_children: bool,
    },
    Request {
        project_id: String,
        request_id: String,
        name: String,
        method: HttpMethod,
    },
}

/// Right-hand Project and HTTP Request Explorer with live quick search.
pub(crate) struct ExplorerPanel {
    search_input: Entity<InputState>,
    tree_state: Entity<TreeState>,
    search_query: String,
    expanded_ids: HashSet<String>,
    meta: HashMap<String, NodeMeta>,
    dirty_keys: HashSet<String>,
    workspace_snapshot: WorkspaceTree,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ExplorerEvent> for ExplorerPanel {}

impl ExplorerPanel {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Quick search requests, URLs, methods…")
        });
        let tree_state = cx.new(|cx| TreeState::new(cx));

        let search_sub = cx.subscribe(&search_input, |this, state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.search_query = state.read(cx).value().to_string();
                this.rebuild_tree(cx);
            }
        });

        let tree_sub = cx.subscribe(&tree_state, |this, _, event: &TreeEvent, cx| {
            match event {
                TreeEvent::Expanded(id) => {
                    this.expanded_ids.insert(id.to_string());
                }
                TreeEvent::Collapsed(id) => {
                    this.expanded_ids.remove(id.as_ref());
                }
            }
            cx.notify();
        });

        Self {
            search_input,
            tree_state,
            search_query: String::new(),
            expanded_ids: HashSet::new(),
            meta: HashMap::new(),
            dirty_keys: HashSet::new(),
            workspace_snapshot: WorkspaceTree::default(),
            _subscriptions: vec![search_sub, tree_sub],
        }
    }

    pub(crate) fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_input
            .update(cx, |input, cx| input.focus(window, cx));
    }

    pub(crate) fn set_dirty_keys(&mut self, dirty_keys: HashSet<String>, cx: &mut Context<Self>) {
        self.dirty_keys = dirty_keys;
        cx.notify();
    }

    pub(crate) fn set_workspace(&mut self, workspace: WorkspaceTree, cx: &mut Context<Self>) {
        // Auto-expand projects and top-level folders on initial population.
        if self.expanded_ids.is_empty() {
            for proj in &workspace.projects {
                self.expanded_ids.insert(format!("proj:{}", proj.doc.id));
                for folder in &proj.folders {
                    self.expanded_ids.insert(format!("fld:{}", folder.doc.id));
                }
            }
        }
        self.workspace_snapshot = workspace;
        self.rebuild_tree(cx);
    }

    pub(crate) fn reveal_and_select(&mut self, tree_id: &str, cx: &mut Context<Self>) {
        let id: SharedString = tree_id.to_string().into();
        self.tree_state.update(cx, |state, cx| {
            state.reveal_item(&id, gpui_kit::ScrollStrategy::Nearest, cx);
            if let Some(ix) = state.index_of(&id) {
                state.set_selected_index(Some(ix), cx);
            }
        });
        cx.notify();
    }

    fn toggle_expand(&mut self, tree_id: &str, cx: &mut Context<Self>) {
        if self.expanded_ids.contains(tree_id) {
            self.expanded_ids.remove(tree_id);
        } else {
            self.expanded_ids.insert(tree_id.to_string());
        }
        self.rebuild_tree(cx);
    }

    fn rebuild_tree(&mut self, cx: &mut Context<Self>) {
        self.meta.clear();
        let force_expand = !self.search_query.trim().is_empty();
        let filtered = filter_workspace(&self.workspace_snapshot, &self.search_query);

        let items: Vec<TreeItem> = filtered
            .into_iter()
            .map(|fp| build_project_item(&mut self.meta, &self.expanded_ids, fp, force_expand))
            .collect();

        self.tree_state.update(cx, |state, cx| {
            state.set_items(items, cx);
        });
        cx.notify();
    }

    fn selected_creation_target(&self, cx: &App) -> Option<(String, Option<String>)> {
        if let Some(entry) = self.tree_state.read(cx).selected_entry()
            && let Some(meta) = self.meta.get(entry.item().id.as_ref())
        {
            match meta {
                NodeMeta::Project { project_id, .. } => return Some((project_id.clone(), None)),
                NodeMeta::Folder {
                    project_id,
                    folder_id,
                    ..
                } => return Some((project_id.clone(), Some(folder_id.clone()))),
                NodeMeta::Request {
                    project_id,
                    request_id,
                    ..
                } => {
                    if let Some((proj, ancestors, _)) =
                        self.workspace_snapshot.find_request(request_id)
                    {
                        let folder_id = ancestors.last().map(|f| f.doc.id.clone());
                        return Some((proj.doc.id.clone(), folder_id));
                    }
                    return Some((project_id.clone(), None));
                }
            }
        }
        self.workspace_snapshot
            .active_project()
            .map(|p| (p.doc.id.clone(), None))
    }

    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.search_query.clear();
        self.rebuild_tree(cx);
    }
}

impl Render for ExplorerPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let total_requests = self.workspace_snapshot.total_request_count();
        let search_active = !self.search_query.trim().is_empty();
        let has_tree_items = self.tree_state.read(cx).entry(0).is_some();

        v_flex()
            .id("right-explorer-panel")
            .test_support()
            .size_full()
            .min_h_0()
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().sidebar_foreground)
            // Explorer header
            .child(
                h_flex()
                    .w_full()
                    .px_3()
                    .py_2()
                    .justify_between()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(
                        h_flex()
                            .gap_1p5()
                            .child(Icon::new(LucideIcon::FolderTree).small())
                            .child(div().text_sm().font_semibold().child(crate::i18n::t("explorer")))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("({total_requests})")),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_0p5()
                            .child(
                                Button::new("explorer-new-request")
                                    .ghost()
                                    .xsmall()
                                    .icon(LucideIcon::FilePlus)
                                    .tooltip(crate::i18n::t("new_request"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some((project_id, folder_id)) =
                                            this.selected_creation_target(cx)
                                        {
                                            cx.emit(ExplorerEvent::NewRequestIn {
                                                project_id,
                                                folder_id,
                                            });
                                        }
                                    })),
                            )
                            .child(
                                Button::new("explorer-new-folder")
                                    .ghost()
                                    .xsmall()
                                    .icon(LucideIcon::FolderPlus)
                                    .tooltip(crate::i18n::t("new_folder"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        if let Some((project_id, folder_id)) =
                                            this.selected_creation_target(cx)
                                        {
                                            cx.emit(ExplorerEvent::NewFolderIn {
                                                project_id,
                                                parent_folder_id: folder_id,
                                            });
                                        }
                                    })),
                            )
                            .child(
                                Button::new("explorer-new-project")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Plus)
                                    .tooltip(crate::i18n::t("new_project"))
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(ExplorerEvent::NewProject);
                                    })),
                            )
                            .child({
                                let view = cx.entity();
                                Button::new("explorer-import-menu")
                                    .ghost()
                                    .xsmall()
                                    .icon(LucideIcon::FileUp)
                                    .tooltip(crate::i18n::t("import_menu"))
                                    .dropdown_menu(move |menu, _, _| {
                                        let v1 = view.clone();
                                        let v2 = view.clone();
                                        let v3 = view.clone();
                                        menu.item(
                                            PopupMenuItem::new(crate::i18n::t("import_openapi"))
                                                .icon(LucideIcon::FileUp)
                                                .on_click(move |_, _, cx| {
                                                    v1.update(cx, |_, cx| {
                                                        cx.emit(ExplorerEvent::ImportOpenApi);
                                                    });
                                                }),
                                        )
                                        .item(
                                            PopupMenuItem::new(crate::i18n::t("import_postman_zip"))
                                                .icon(LucideIcon::FolderArchive)
                                                .on_click(move |_, _, cx| {
                                                    v2.update(cx, |_, cx| {
                                                        cx.emit(ExplorerEvent::ImportPostmanZip);
                                                    });
                                                }),
                                        )
                                        .item(
                                            PopupMenuItem::new(crate::i18n::t("import_apifox"))
                                                .icon(LucideIcon::Braces)
                                                .on_click(move |_, _, cx| {
                                                    v3.update(cx, |_, cx| {
                                                        cx.emit(ExplorerEvent::ImportApifox);
                                                    });
                                                }),
                                        )
                                    })
                            })
                            .child(
                                Button::new("explorer-reload")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::RefreshCw)
                                    .tooltip(crate::i18n::t("reload_disk"))
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(ExplorerEvent::ReloadWorkspace);
                                    })),
                            ),
                    ),
            )
            // Quick search field
            .child(
                div()
                    .w_full()
                    .px_2p5()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().sidebar_border)
                    .child(
                        Input::new(&self.search_input)
                            .id("explorer-quick-search")
                            .aria_label("Quick search explorer")
                            .small()
                            .cleanable(true)
                            .prefix(Icon::new(IconName::Search).small()),
                    ),
            )
            // Tree or empty search state
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .size_full()
                    .p_1p5()
                    .when(!has_tree_items && search_active, |this| {
                        this.child(
                            v_flex()
                                .w_full()
                                .items_center()
                                .justify_center()
                                .gap_2()
                                .py_8()
                                .px_3()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!("No matches for \"{}\"", self.search_query)),
                                )
                                .child(
                                    Button::new("explorer-clear-search")
                                        .outline()
                                        .xsmall()
                                        .label("Clear search")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.clear_search(window, cx);
                                        })),
                                ),
                        )
                    })
                    .when(has_tree_items || !search_active, |this| {
                        this.child(
                            tree(&self.tree_state, {
                                let view = view.clone();
                                move |_ix, entry, selected, _window, cx| {
                                    let tree_id = entry.item().id.clone();
                                    let depth = entry.depth();
                                    let is_expanded = entry.is_expanded();
                                    let meta = view.read(cx).meta.get(tree_id.as_ref()).cloned();
                                    let is_dirty =
                                        view.read(cx).dirty_keys.contains(tree_id.as_ref());

                                    render_tree_row(
                                        &view,
                                        tree_id,
                                        depth,
                                        is_expanded,
                                        selected,
                                        is_dirty,
                                        meta,
                                        cx,
                                    )
                                }
                            })
                            .size_full()
                            .context_menu({
                                let view = view.clone();
                                move |_ix, entry, menu, _window, cx| {
                                    let tree_id = entry.item().id.to_string();
                                    let meta = view.read(cx).meta.get(&tree_id).cloned();
                                    build_context_menu(&view, meta, menu)
                                }
                            }),
                        )
                    }),
            )
    }
}

#[allow(clippy::too_many_arguments)]
fn render_tree_row(
    view: &Entity<ExplorerPanel>,
    tree_id: SharedString,
    depth: usize,
    is_expanded: bool,
    selected: bool,
    is_dirty: bool,
    meta: Option<NodeMeta>,
    cx: &mut App,
) -> ListItem {
    let indent = px(12.) * depth as f32 + px(6.);
    let row_id = SharedString::from(format!("tree-row-{tree_id}"));

    let (a11y_label, content, click_action) = match meta {
        Some(NodeMeta::Project {
            project_id,
            name,
            has_auth,
            request_count,
            has_children,
        }) => {
            let chevron = if has_children {
                if is_expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                }
            } else {
                IconName::Dash
            };
            let tid_for_toggle = tree_id.clone();
            let view_toggle = view.clone();

            let row = h_flex()
                .w_full()
                .gap_1p5()
                .items_center()
                .child(
                    Button::new(SharedString::from(format!("chev-{tree_id}")))
                        .ghost()
                        .xsmall()
                        .icon(chevron)
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            view_toggle.update(cx, |panel, cx| {
                                panel.toggle_expand(&tid_for_toggle, cx);
                            });
                        }),
                )
                .child(
                    Icon::new(LucideIcon::Boxes)
                        .small()
                        .text_color(cx.theme().primary),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .font_semibold()
                        .child(name.clone()),
                )
                .when(has_auth, |r| {
                    r.child(
                        Icon::new(LucideIcon::ShieldCheck)
                            .xsmall()
                            .text_color(cx.theme().info),
                    )
                })
                .when(is_dirty, |r| {
                    r.child(
                        div()
                            .size(px(6.))
                            .rounded(cx.theme().radius_full())
                            .bg(cx.theme().warning),
                    )
                })
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(request_count.to_string()),
                );

            (
                SharedString::from(name),
                row.into_any_element(),
                Some((tree_id, ExplorerNode::Project { project_id }, true)),
            )
        }
        Some(NodeMeta::Folder {
            project_id,
            folder_id,
            name,
            has_auth,
            has_children,
        }) => {
            let chevron = if has_children {
                if is_expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                }
            } else {
                IconName::ChevronRight
            };
            let folder_icon = if has_auth {
                LucideIcon::FolderKey
            } else if is_expanded {
                LucideIcon::FolderOpen
            } else {
                LucideIcon::Folder
            };
            let tid_for_toggle = tree_id.clone();
            let view_toggle = view.clone();

            let row = h_flex()
                .w_full()
                .gap_1p5()
                .items_center()
                .child(
                    Button::new(SharedString::from(format!("chev-{tree_id}")))
                        .ghost()
                        .xsmall()
                        .icon(chevron)
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            view_toggle.update(cx, |panel, cx| {
                                panel.toggle_expand(&tid_for_toggle, cx);
                            });
                        }),
                )
                .child(Icon::new(folder_icon).small().text_color(if has_auth {
                    cx.theme().info
                } else {
                    cx.theme().muted_foreground
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .font_medium()
                        .child(name.clone()),
                )
                .when(is_dirty, |r| {
                    r.child(
                        div()
                            .size(px(6.))
                            .rounded(cx.theme().radius_full())
                            .bg(cx.theme().warning),
                    )
                });

            (
                SharedString::from(name),
                row.into_any_element(),
                Some((
                    tree_id,
                    ExplorerNode::Folder {
                        project_id,
                        folder_id,
                    },
                    true,
                )),
            )
        }
        Some(NodeMeta::Request {
            project_id,
            request_id,
            name,
            method,
        }) => {
            let row = h_flex()
                .w_full()
                .gap_1p5()
                .items_center()
                .pl_4()
                .child(method_badge(method, true))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .when(selected, |d| d.font_medium())
                        .child(name.clone()),
                )
                .when(is_dirty, |r| {
                    r.child(
                        div()
                            .size(px(6.))
                            .rounded(cx.theme().radius_full())
                            .bg(cx.theme().warning),
                    )
                });

            (
                SharedString::from(format!("{} {}", method.as_str(), name)),
                row.into_any_element(),
                Some((
                    tree_id,
                    ExplorerNode::Request {
                        project_id,
                        request_id,
                    },
                    false,
                )),
            )
        }
        None => (
            tree_id.clone(),
            div().child(tree_id).into_any_element(),
            None,
        ),
    };

    let view_click = view.clone();
    ListItem::new(row_id)
        .w_full()
        .rounded(cx.theme().radius)
        .px_1p5()
        .py_0p5()
        .pl(indent)
        .accessibility_label(a11y_label)
        .selected(selected)
        .child(content)
        .when_some(click_action, |item, (tid, node, expand_on_click)| {
            item.on_click(move |_, _, cx| {
                let _ = (&tid, expand_on_click);
                view_click.update(cx, |_, cx| {
                    cx.emit(ExplorerEvent::OpenNode(node.clone()));
                });
            })
        })
}

fn build_context_menu(
    view: &Entity<ExplorerPanel>,
    meta: Option<NodeMeta>,
    menu: gpui_kit::component::menu::PopupMenu,
) -> gpui_kit::component::menu::PopupMenu {
    let Some(meta) = meta else {
        return menu;
    };

    match meta {
        NodeMeta::Project { project_id, .. } => {
            let v1 = view.clone();
            let v2 = view.clone();
            let v3 = view.clone();
            let v4 = view.clone();
            let v5 = view.clone();
            let p1 = project_id.clone();
            let p2 = project_id.clone();
            let p3 = project_id.clone();
            let p4 = project_id;
            menu.item(
                PopupMenuItem::new(crate::i18n::t("new_request"))
                    .icon(LucideIcon::FilePlus)
                    .on_click(move |_, _, cx| {
                        v1.update(cx, |_, cx| {
                            cx.emit(ExplorerEvent::NewRequestIn {
                                project_id: p1.clone(),
                                folder_id: None,
                            });
                        });
                    }),
            )
            .item(
                PopupMenuItem::new(crate::i18n::t("new_folder"))
                    .icon(LucideIcon::FolderPlus)
                    .on_click(move |_, _, cx| {
                        v2.update(cx, |_, cx| {
                            cx.emit(ExplorerEvent::NewFolderIn {
                                project_id: p2.clone(),
                                parent_folder_id: None,
                            });
                        });
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new(crate::i18n::t("project_settings_menu"))
                    .icon(LucideIcon::ShieldCheck)
                    .on_click(move |_, _, cx| {
                        v3.update(cx, |_, cx| {
                            cx.emit(ExplorerEvent::OpenNode(ExplorerNode::Project {
                                project_id: p3.clone(),
                            }));
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("Reload from Disk")
                    .icon(IconName::RefreshCw)
                    .on_click(move |_, _, cx| {
                        v4.update(cx, |_, cx| {
                            cx.emit(ExplorerEvent::ReloadWorkspace);
                        });
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new(crate::i18n::t("delete_project_menu"))
                    .icon(IconName::Delete)
                    .on_click(move |_, _, cx| {
                        v5.update(cx, |_, cx| {
                            cx.emit(ExplorerEvent::DeleteProject {
                                project_id: p4.clone(),
                            });
                        });
                    }),
            )
        }
        NodeMeta::Folder {
            project_id,
            folder_id,
            ..
        } => {
            let v1 = view.clone();
            let v2 = view.clone();
            let v3 = view.clone();
            let v4 = view.clone();
            let p1 = project_id.clone();
            let f1 = folder_id.clone();
            let p2 = project_id.clone();
            let f2 = folder_id.clone();
            let p3 = project_id.clone();
            let f3 = folder_id.clone();
            menu.item(
                PopupMenuItem::new(crate::i18n::t("new_request"))
                    .icon(LucideIcon::FilePlus)
                    .on_click(move |_, _, cx| {
                        v1.update(cx, |_, cx| {
                            cx.emit(ExplorerEvent::NewRequestIn {
                                project_id: p1.clone(),
                                folder_id: Some(f1.clone()),
                            });
                        });
                    }),
            )
            .item(
                PopupMenuItem::new(crate::i18n::t("new_subfolder_menu"))
                    .icon(LucideIcon::FolderPlus)
                    .on_click(move |_, _, cx| {
                        v2.update(cx, |_, cx| {
                            cx.emit(ExplorerEvent::NewFolderIn {
                                project_id: p2.clone(),
                                parent_folder_id: Some(f2.clone()),
                            });
                        });
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new(crate::i18n::t("folder_settings_menu"))
                    .icon(LucideIcon::FolderKey)
                    .on_click(move |_, _, cx| {
                        v3.update(cx, |_, cx| {
                            cx.emit(ExplorerEvent::OpenNode(ExplorerNode::Folder {
                                project_id: p3.clone(),
                                folder_id: f3.clone(),
                            }));
                        });
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new(crate::i18n::t("delete_folder_menu"))
                    .icon(IconName::Delete)
                    .on_click(move |_, _, cx| {
                        v4.update(cx, |_, cx| {
                            cx.emit(ExplorerEvent::DeleteFolder {
                                project_id: project_id.clone(),
                                folder_id: folder_id.clone(),
                            });
                        });
                    }),
            )
        }
        NodeMeta::Request {
            project_id,
            request_id,
            ..
        } => {
            let v1 = view.clone();
            let v2 = view.clone();
            let v3 = view.clone();
            let p1 = project_id.clone();
            let r1 = request_id.clone();
            let p2 = project_id.clone();
            let r2 = request_id.clone();
            menu.item(
                PopupMenuItem::new("Open")
                    .icon(LucideIcon::FileCode)
                    .on_click(move |_, _, cx| {
                        v1.update(cx, |_, cx| {
                            cx.emit(ExplorerEvent::OpenNode(ExplorerNode::Request {
                                project_id: p1.clone(),
                                request_id: r1.clone(),
                            }));
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("Duplicate")
                    .icon(IconName::Copy)
                    .on_click(move |_, _, cx| {
                        v2.update(cx, |_, cx| {
                            cx.emit(ExplorerEvent::DuplicateRequest {
                                project_id: p2.clone(),
                                request_id: r2.clone(),
                            });
                        });
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new("Delete Request…")
                    .icon(IconName::Delete)
                    .on_click(move |_, _, cx| {
                        v3.update(cx, |_, cx| {
                            cx.emit(ExplorerEvent::DeleteRequest {
                                project_id: project_id.clone(),
                                request_id: request_id.clone(),
                            });
                        });
                    }),
            )
        }
    }
}

fn build_project_item(
    meta: &mut HashMap<String, NodeMeta>,
    expanded_ids: &HashSet<String>,
    fp: FilteredProject<'_>,
    force_expand: bool,
) -> TreeItem {
    let proj = fp.project;
    let tree_id = format!("proj:{}", proj.doc.id);
    let has_children = !fp.folders.is_empty() || !fp.requests.is_empty();
    let expanded = force_expand || expanded_ids.contains(&tree_id);

    meta.insert(
        tree_id.clone(),
        NodeMeta::Project {
            project_id: proj.doc.id.clone(),
            name: proj.doc.name.clone(),
            has_auth: !matches!(
                proj.doc.auth,
                testrix_core::AuthConfig::None | testrix_core::AuthConfig::Inherit
            ),
            request_count: proj.request_count(),
            has_children,
        },
    );

    let mut children = Vec::with_capacity(fp.folders.len() + fp.requests.len());
    for folder in fp.folders {
        children.push(build_folder_item(
            meta,
            expanded_ids,
            &proj.doc.id,
            folder,
            force_expand,
        ));
    }
    for req in fp.requests {
        children.push(build_request_item(meta, &proj.doc.id, req));
    }

    TreeItem::new(tree_id, proj.doc.name.clone())
        .expanded(expanded)
        .children(children)
}

fn build_folder_item(
    meta: &mut HashMap<String, NodeMeta>,
    expanded_ids: &HashSet<String>,
    project_id: &str,
    ff: FilteredFolder<'_>,
    force_expand: bool,
) -> TreeItem {
    let folder = ff.folder;
    let tree_id = format!("fld:{}", folder.doc.id);
    let has_children = !ff.folders.is_empty() || !ff.requests.is_empty();
    let expanded = force_expand || expanded_ids.contains(&tree_id);

    meta.insert(
        tree_id.clone(),
        NodeMeta::Folder {
            project_id: project_id.to_string(),
            folder_id: folder.doc.id.clone(),
            name: folder.doc.name.clone(),
            has_auth: folder.doc.auth.is_explicit(),
            has_children,
        },
    );

    let mut children = Vec::with_capacity(ff.folders.len() + ff.requests.len());
    for sub in ff.folders {
        children.push(build_folder_item(
            meta,
            expanded_ids,
            project_id,
            sub,
            force_expand,
        ));
    }
    for req in ff.requests {
        children.push(build_request_item(meta, project_id, req));
    }

    TreeItem::new(tree_id, folder.doc.name.clone())
        .expanded(expanded)
        .children(children)
}

fn build_request_item(
    meta: &mut HashMap<String, NodeMeta>,
    project_id: &str,
    req: &RequestNode,
) -> TreeItem {
    let tree_id = format!("req:{}", req.doc.id);
    meta.insert(
        tree_id.clone(),
        NodeMeta::Request {
            project_id: project_id.to_string(),
            request_id: req.doc.id.clone(),
            name: req.doc.name.clone(),
            method: req.doc.request.method,
        },
    );
    TreeItem::new(tree_id, req.doc.name.clone())
}
