use std::path::PathBuf;

use gpui_kit::{
    App, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Window,
    assets::IconName as LucideIcon,
    component::{
        ActiveTheme as _, Icon, Sizable as _, StyledExt as _,
        button::{Button, ButtonVariants as _},
        form::{Field, Form},
        h_flex,
        input::{Editor, EditorState, Input, InputEvent, InputState, Textarea, TextareaState},
        scroll::ScrollableElement as _,
        tab::{Tab, TabBar},
        tag::{Tag, TagVariant},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};
use testrix_core::{
    AuthNodeRef, EnvironmentDoc, FolderDoc, FolderNode, ProjectDoc, ProjectNode,
    rename_template_variables, resolve_folder_chain_auth, resolve_project_auth,
};
use testrix_storage::to_yaml_pretty;

use crate::{
    auth_section::{AuthEditor, AuthEditorEvent},
    kv_editor::{KeyValueTable, TableChangeEvent, common_http_header_options},
    theme_helpers::apply_unresolved_var_border,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContainerSection {
    Auth,
    General,
    DefaultHeaders,
    YamlSpec,
}

#[derive(Debug, Clone)]
pub(crate) enum ContainerTabEvent {
    SaveProject {
        project_id: String,
        doc: ProjectDoc,
    },
    SaveFolder {
        project_id: String,
        folder_id: String,
        doc: FolderDoc,
    },
    OpenAuthSource(AuthNodeRef),
    DirtyChanged,
}

#[derive(Clone)]
enum ContainerKind {
    Project {
        saved: ProjectDoc,
        manifest_path: PathBuf,
    },
    Folder {
        folder_id: String,
        saved: FolderDoc,
        manifest_path: PathBuf,
    },
}

/// Workbench tab view for editing Project or Folder settings and authentication inheritance.
pub(crate) struct ContainerTabView {
    project_id: String,
    project_name: String,
    kind: ContainerKind,
    active_section: ContainerSection,
    name_input: Entity<InputState>,
    description_input: Entity<TextareaState>,
    base_url_input: Option<Entity<InputState>>,
    order_input: Option<Entity<InputState>>,
    default_headers: Option<Entity<KeyValueTable>>,
    auth_editor: Entity<AuthEditor>,
    yaml_editor: Entity<EditorState>,
    active_env: Option<EnvironmentDoc>,
    breadcrumb: SharedString,
    dirty: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ContainerTabEvent> for ContainerTabView {}

impl ContainerTabView {
    pub(crate) fn new_project(
        project: &ProjectNode,
        active_env: Option<EnvironmentDoc>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Project name")
                .default_value(project.doc.name.clone())
        });
        let description_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(3)
                .placeholder("Project description…")
                .default_value(project.doc.description.clone())
        });
        let base_url_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://api.example.com or {{base_url}}")
                .default_value(project.doc.base_url.clone())
        });
        let default_headers = cx.new(|cx| {
            KeyValueTable::new(
                format!("proj-hdr-{}", project.doc.id),
                "Header Name",
                "Header Value",
                "Add Default Header",
                &project.doc.default_headers,
                window,
                cx,
            )
            .with_name_suggestions(common_http_header_options(), window, cx)
            .with_variable_validation(active_env.clone())
        });
        let auth_editor = cx.new(|cx| {
            let mut editor = AuthEditor::new(
                format!("proj-auth-{}", project.doc.id),
                false,
                &project.doc.auth,
                window,
                cx,
            );
            editor.set_context(Some(resolve_project_auth(project)), active_env.clone(), cx);
            editor
        });
        let yaml_text = to_yaml_pretty(&project.doc).unwrap_or_default();
        let yaml_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("yaml")
                .folding(true)
                .default_value(yaml_text)
        });

        let mut view = Self {
            project_id: project.doc.id.clone(),
            project_name: project.doc.name.clone(),
            kind: ContainerKind::Project {
                saved: project.doc.clone(),
                manifest_path: project.manifest_path.clone(),
            },
            active_section: ContainerSection::Auth,
            name_input,
            description_input,
            base_url_input: Some(base_url_input),
            order_input: None,
            default_headers: Some(default_headers),
            auth_editor,
            yaml_editor,
            active_env,
            breadcrumb: project.doc.name.clone().into(),
            dirty: false,
            _subscriptions: Vec::new(),
        };
        view.wire_subscriptions(window, cx);
        view
    }

    pub(crate) fn new_folder(
        project: &ProjectNode,
        folder_chain: &[&FolderNode],
        active_env: Option<EnvironmentDoc>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let folder = *folder_chain.last().expect("folder chain non-empty");
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Folder name")
                .default_value(folder.doc.name.clone())
        });
        let description_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(3)
                .placeholder("Folder description…")
                .default_value(folder.doc.description.clone())
        });
        let order_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("0")
                .default_value(folder.doc.order.to_string())
        });
        let resolved = resolve_folder_chain_auth(project, folder_chain);
        let auth_editor = cx.new(|cx| {
            let mut editor = AuthEditor::new(
                format!("fld-auth-{}", folder.doc.id),
                true,
                &folder.doc.auth,
                window,
                cx,
            );
            editor.set_context(Some(resolved), active_env.clone(), cx);
            editor
        });
        let yaml_text = to_yaml_pretty(&folder.doc).unwrap_or_default();
        let yaml_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("yaml")
                .folding(true)
                .default_value(yaml_text)
        });

        let mut parts = vec![project.doc.name.clone()];
        for f in folder_chain {
            parts.push(f.doc.name.clone());
        }
        let breadcrumb: SharedString = parts.join(" › ").into();

        let mut view = Self {
            project_id: project.doc.id.clone(),
            project_name: project.doc.name.clone(),
            kind: ContainerKind::Folder {
                folder_id: folder.doc.id.clone(),
                saved: folder.doc.clone(),
                manifest_path: folder.manifest_path.clone(),
            },
            active_section: ContainerSection::Auth,
            name_input,
            description_input,
            base_url_input: None,
            order_input: Some(order_input),
            default_headers: None,
            auth_editor,
            yaml_editor,
            active_env,
            breadcrumb,
            dirty: false,
            _subscriptions: Vec::new(),
        };
        view.wire_subscriptions(window, cx);
        view
    }

    fn wire_subscriptions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut subs = Vec::new();
        subs.push(cx.subscribe_in(
            &self.name_input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.on_edited(window, cx);
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &self.description_input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.on_edited(window, cx);
                }
            },
        ));
        if let Some(base_url) = &self.base_url_input {
            subs.push(cx.subscribe_in(
                base_url,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.on_edited(window, cx);
                    }
                },
            ));
        }
        if let Some(order) = &self.order_input {
            subs.push(
                cx.subscribe_in(order, window, |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.on_edited(window, cx);
                    }
                }),
            );
        }
        if let Some(headers) = &self.default_headers {
            subs.push(cx.subscribe_in(
                headers,
                window,
                |this, _, _: &TableChangeEvent, window, cx| {
                    this.on_edited(window, cx);
                },
            ));
        }
        subs.push(cx.subscribe_in(
            &self.auth_editor,
            window,
            |this, _, event: &AuthEditorEvent, window, cx| match event {
                AuthEditorEvent::Changed => this.on_edited(window, cx),
                AuthEditorEvent::OpenSourceNode(node) => {
                    cx.emit(ContainerTabEvent::OpenAuthSource(node.clone()));
                }
            },
        ));
        self._subscriptions = subs;
    }

    pub(crate) fn project_id(&self) -> &str {
        &self.project_id
    }

    pub(crate) fn tab_key(&self) -> String {
        match &self.kind {
            ContainerKind::Project { .. } => format!("proj:{}", self.project_id),
            ContainerKind::Folder { folder_id, .. } => format!("fld:{folder_id}"),
        }
    }

    pub(crate) fn title(&self, cx: &App) -> String {
        let name = self.name_input.read(cx).value().trim().to_string();
        if name.is_empty() {
            match &self.kind {
                ContainerKind::Project { saved, .. } => saved.name.clone(),
                ContainerKind::Folder { saved, .. } => saved.name.clone(),
            }
        } else {
            name
        }
    }

    pub(crate) fn is_project(&self) -> bool {
        matches!(self.kind, ContainerKind::Project { .. })
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub(crate) fn sync_context(
        &mut self,
        project: &ProjectNode,
        active_env: Option<EnvironmentDoc>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.project_name = project.doc.name.clone();
        self.active_env = active_env.clone();
        if let Some(headers) = &self.default_headers {
            headers.update(cx, |table, cx| table.set_active_env(active_env.clone(), cx));
        }
        match &self.kind {
            ContainerKind::Project { .. } => {
                let mut draft = self.build_project_doc(cx);
                let mut virtual_proj = project.clone();
                virtual_proj.doc = draft.clone();
                let resolved = resolve_project_auth(&virtual_proj);
                self.auth_editor
                    .update(cx, |e, cx| e.set_context(Some(resolved), active_env, cx));
                if draft.name.trim().is_empty() {
                    draft.name = project.doc.name.clone();
                }
                self.breadcrumb = draft.name.into();
            }
            ContainerKind::Folder { folder_id, .. } => {
                if let Some(chain) = project.find_folder(folder_id) {
                    let mut parts = vec![project.doc.name.clone()];
                    for f in &chain {
                        parts.push(f.doc.name.clone());
                    }
                    self.breadcrumb = parts.join(" › ").into();

                    let draft = self.build_folder_doc(cx);
                    let mut virtual_nodes: Vec<FolderNode> =
                        chain.iter().map(|f| (*f).clone()).collect();
                    if let Some(last) = virtual_nodes.last_mut() {
                        last.doc = draft;
                    }
                    let refs: Vec<&FolderNode> = virtual_nodes.iter().collect();
                    let resolved = resolve_folder_chain_auth(project, &refs);
                    self.auth_editor
                        .update(cx, |e, cx| e.set_context(Some(resolved), active_env, cx));
                }
            }
        }
        self.refresh_yaml_preview(window, cx);
        cx.notify();
    }

    pub(crate) fn mark_saved(&mut self, cx: &mut Context<Self>) {
        match &mut self.kind {
            ContainerKind::Project { saved, .. } => {
                *saved = build_project_doc_with_fallback(
                    saved,
                    &self.name_input,
                    &self.description_input,
                    self.base_url_input.as_ref(),
                    self.default_headers.as_ref(),
                    &self.auth_editor,
                    cx,
                );
            }
            ContainerKind::Folder { saved, .. } => {
                *saved = build_folder_doc_with_fallback(
                    saved,
                    &self.name_input,
                    &self.description_input,
                    self.order_input.as_ref(),
                    &self.auth_editor,
                    cx,
                );
            }
        }
        self.dirty = false;
        cx.emit(ContainerTabEvent::DirtyChanged);
        cx.notify();
    }

    pub(crate) fn rename_variables(
        &mut self,
        renames: &[(String, String)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut changed = false;
        if let Some(base_url) = &self.base_url_input {
            let current = base_url.read(cx).value().to_string();
            if let Some(updated) = rename_template_variables(&current, renames) {
                base_url.update(cx, |state, cx| state.set_value(updated, window, cx));
                changed = true;
            }
        }
        if let Some(headers) = &self.default_headers {
            changed |= headers.update(cx, |t, cx| t.rename_variables(renames, window, cx));
        }
        changed |= self
            .auth_editor
            .update(cx, |e, cx| e.rename_variables(renames, window, cx));

        if changed {
            if !self.dirty {
                self.mark_saved(cx);
            }
            self.refresh_yaml_preview(window, cx);
            cx.notify();
        }
        changed
    }

    pub(crate) fn save(&mut self, cx: &mut Context<Self>) {
        match &self.kind {
            ContainerKind::Project { .. } => {
                let doc = self.build_project_doc(cx);
                cx.emit(ContainerTabEvent::SaveProject {
                    project_id: self.project_id.clone(),
                    doc,
                });
            }
            ContainerKind::Folder { folder_id, .. } => {
                let doc = self.build_folder_doc(cx);
                cx.emit(ContainerTabEvent::SaveFolder {
                    project_id: self.project_id.clone(),
                    folder_id: folder_id.clone(),
                    doc,
                });
            }
        }
    }

    fn on_edited(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dirty = match &self.kind {
            ContainerKind::Project { saved, .. } => &self.build_project_doc(cx) != saved,
            ContainerKind::Folder { saved, .. } => &self.build_folder_doc(cx) != saved,
        };
        self.refresh_yaml_preview(window, cx);
        cx.emit(ContainerTabEvent::DirtyChanged);
        cx.notify();
    }

    fn refresh_yaml_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let yaml = match &self.kind {
            ContainerKind::Project { .. } => to_yaml_pretty(&self.build_project_doc(cx)),
            ContainerKind::Folder { .. } => to_yaml_pretty(&self.build_folder_doc(cx)),
        }
        .unwrap_or_default();
        self.yaml_editor.update(cx, |editor, cx| {
            editor.set_value(yaml, window, cx);
        });
    }

    fn build_project_doc(&self, cx: &App) -> ProjectDoc {
        let ContainerKind::Project { saved, .. } = &self.kind else {
            unreachable!("not a project");
        };
        build_project_doc_with_fallback(
            saved,
            &self.name_input,
            &self.description_input,
            self.base_url_input.as_ref(),
            self.default_headers.as_ref(),
            &self.auth_editor,
            cx,
        )
    }

    fn build_folder_doc(&self, cx: &App) -> FolderDoc {
        let ContainerKind::Folder { saved, .. } = &self.kind else {
            unreachable!("not a folder");
        };
        build_folder_doc_with_fallback(
            saved,
            &self.name_input,
            &self.description_input,
            self.order_input.as_ref(),
            &self.auth_editor,
            cx,
        )
    }
}

fn build_project_doc_with_fallback(
    saved: &ProjectDoc,
    name_input: &Entity<InputState>,
    desc_input: &Entity<TextareaState>,
    base_url_input: Option<&Entity<InputState>>,
    headers_table: Option<&Entity<KeyValueTable>>,
    auth_editor: &Entity<AuthEditor>,
    cx: &App,
) -> ProjectDoc {
    let mut doc = saved.clone();
    let next_name = name_input.read(cx).value().trim().to_string();
    if !next_name.is_empty() {
        doc.name = next_name;
    }
    doc.description = desc_input.read(cx).value().to_string();
    if let Some(base) = base_url_input {
        doc.base_url = base.read(cx).value().trim().to_string();
    }
    if let Some(headers) = headers_table {
        doc.default_headers = headers.read(cx).snapshot(cx);
    }
    doc.auth = auth_editor.read(cx).snapshot(cx);
    doc
}

fn build_folder_doc_with_fallback(
    saved: &FolderDoc,
    name_input: &Entity<InputState>,
    desc_input: &Entity<TextareaState>,
    order_input: Option<&Entity<InputState>>,
    auth_editor: &Entity<AuthEditor>,
    cx: &App,
) -> FolderDoc {
    let mut doc = saved.clone();
    let next_name = name_input.read(cx).value().trim().to_string();
    if !next_name.is_empty() {
        doc.name = next_name;
    }
    doc.description = desc_input.read(cx).value().to_string();
    if let Some(order) = order_input
        && let Ok(parsed) = order.read(cx).value().trim().parse::<i32>()
    {
        doc.order = parsed;
    }
    doc.auth = auth_editor.read(cx).snapshot(cx);
    doc
}

impl Render for ContainerTabView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_project = self.is_project();
        let manifest_path = match &self.kind {
            ContainerKind::Project { manifest_path, .. }
            | ContainerKind::Folder { manifest_path, .. } => {
                manifest_path.to_string_lossy().to_string()
            }
        };

        let sections: Vec<(ContainerSection, &'static str)> = if is_project {
            vec![
                (ContainerSection::Auth, "Auth"),
                (ContainerSection::General, "Project Settings"),
                (ContainerSection::DefaultHeaders, "Default Headers"),
                (ContainerSection::YamlSpec, "YAML Document"),
            ]
        } else {
            vec![
                (ContainerSection::Auth, "Auth"),
                (ContainerSection::General, "Folder Settings"),
                (ContainerSection::YamlSpec, "YAML Document"),
            ]
        };

        let selected_ix = sections
            .iter()
            .position(|(sec, _)| *sec == self.active_section)
            .unwrap_or(0);

        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            // Header strip
            .child(
                h_flex()
                    .w_full()
                    .px_4()
                    .py_2p5()
                    .justify_between()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_2()
                            .min_w_0()
                            .child(
                                Tag::new()
                                    .with_variant(TagVariant::Secondary)
                                    .outline()
                                    .small()
                                    .child(if is_project { "PROJECT" } else { "FOLDER" }),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .truncate()
                                    .child(self.breadcrumb.clone()),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(manifest_path),
                            )
                            .child(
                                Button::new(SharedString::from(format!(
                                    "save-container-{}",
                                    self.tab_key()
                                )))
                                    .primary()
                                    .small()
                                    .icon(Icon::new(LucideIcon::Save))
                                    .label(if self.dirty { "Save*" } else { "Save" })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.save(cx);
                                    })),
                            ),
                    ),
            )
            // Section tabs
            .child(
                TabBar::new(SharedString::from(format!(
                    "container-sections-{}",
                    self.tab_key()
                )))
                    .w_full()
                    .underline()
                    .small()
                    .px_4()
                    .selected_index(selected_ix)
                    .on_click(cx.listener({
                        let sections = sections.clone();
                        move |this, ix: &usize, _, cx| {
                            if let Some((sec, _)) = sections.get(*ix) {
                                this.active_section = *sec;
                                cx.notify();
                            }
                        }
                    }))
                    .children(sections.iter().map(|(_, label)| Tab::new().label(*label))),
            )
            // Content body
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .overflow_y_scrollbar()
                    .when(self.active_section == ContainerSection::Auth, |this| {
                        this.child(
                            v_flex()
                                .gap_3()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(if is_project {
                                            "Project authentication is inherited by folders and requests set to \"Inherit from parent\"."
                                        } else {
                                            "Folder authentication overrides parent auth for child folders and requests set to \"Inherit from parent\"."
                                        }),
                                )
                                .child(self.auth_editor.clone()),
                        )
                    })
                    .when(self.active_section == ContainerSection::General, |this| {
                        this.child(
                            Form::new()
                                .child(
                                    Field::new()
                                        .label("Name")
                                        .child(Input::new(&self.name_input).small()),
                                )
                                .when_some(self.base_url_input.as_ref(), |form, base_url| {
                                    let base_text = base_url.read(cx).value().to_string();
                                    form.child(
                                        Field::new()
                                            .label("Base URL")
                                            .description("Prepended when a request URL starts with /")
                                            .child(apply_unresolved_var_border(
                                                Input::new(base_url).small(),
                                                &base_text,
                                                self.active_env.as_ref(),
                                                cx,
                                            )),
                                    )
                                })
                                .when_some(self.order_input.as_ref(), |form, order| {
                                    form.child(
                                        Field::new()
                                            .label("Sort Order")
                                            .child(Input::new(order).small().w(px(120.))),
                                    )
                                })
                                .child(
                                    Field::new()
                                        .label("Description")
                                        .child(Textarea::new(&self.description_input)),
                                ),
                        )
                    })
                    .when(
                        self.active_section == ContainerSection::DefaultHeaders,
                        |this| {
                            this.when_some(self.default_headers.as_ref(), |d, headers| {
                                d.child(headers.clone())
                            })
                        },
                    )
                    .when(self.active_section == ContainerSection::YamlSpec, |this| {
                        this.child(
                            div()
                                .size_full()
                                .min_h(px(320.))
                                .child(Editor::new(&self.yaml_editor).h(px(360.)).readonly(true)),
                        )
                    }),
            )
    }
}
