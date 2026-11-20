use std::path::PathBuf;

use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window,
    assets::IconName as LucideIcon,
    component::{
        ActiveTheme as _, Disableable as _, Icon, IndexPath, Sizable as _, StyledExt as _,
        WindowExt as _,
        button::{Button, ButtonVariants as _},
        form::{Field, Form},
        h_flex,
        input::{Editor, EditorState, Input, InputEvent, InputState, Textarea, TextareaState},
        scroll::ScrollableElement as _,
        select::{Select, SelectItem, SelectState},
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
    rename_template_variables, replace_template_variables, resolve_folder_chain_auth,
    resolve_project_auth,
};
use testrix_http::{
    LocalShareHandle, NetworkAdapterIp, ShareChannel, ShareUploadResult,
    collect_project_yaml_files, dispatch_share_future, list_outbound_ipv4_addresses,
    start_local_share_server, upload_to_cloudflare_r2, upload_to_testrix_server,
};
use testrix_storage::{ShareSettings, slugify, to_yaml_pretty};

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
    Share,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AdapterIpOption {
    ip: String,
    label: SharedString,
}

impl AdapterIpOption {
    fn from_adapter(adapter: &NetworkAdapterIp) -> Self {
        Self {
            ip: adapter.ip.clone(),
            label: adapter.display_label().into(),
        }
    }
}

impl SelectItem for AdapterIpOption {
    type Value = String;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.ip
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ActiveShareRecord {
    pub(crate) id: String,
    pub(crate) project_id: String,
    pub(crate) project_name: String,
    pub(crate) project_slug: String,
    pub(crate) channel: ShareChannel,
    pub(crate) share_url: String,
    pub(crate) target_info: String,
    pub(crate) file_paths: Vec<String>,
    pub(crate) local_handle: Option<LocalShareHandle>,
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
    ShareStarted(ActiveShareRecord),
}

#[derive(Clone)]
enum ContainerKind {
    Project {
        saved: ProjectDoc,
        manifest_path: PathBuf,
        project_root: PathBuf,
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
    share_settings: ShareSettings,
    share_channel: ShareChannel,
    share_in_flight: bool,
    share_status: Option<Result<ShareUploadResult, String>>,
    project_snapshot: Option<ProjectNode>,
    ip_select: Option<Entity<SelectState<Vec<AdapterIpOption>>>>,
    local_port_input: Option<Entity<InputState>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ContainerTabEvent> for ContainerTabView {}

impl ContainerTabView {
    pub(crate) fn new_project(
        project: &ProjectNode,
        active_env: Option<EnvironmentDoc>,
        share_settings: ShareSettings,
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

        let adapter_options: Vec<AdapterIpOption> = list_outbound_ipv4_addresses()
            .iter()
            .map(AdapterIpOption::from_adapter)
            .collect();
        let selected_ip_ix = if adapter_options.is_empty() {
            None
        } else {
            Some(IndexPath::default())
        };
        let ip_select = cx.new(|cx| SelectState::new(adapter_options, selected_ip_ix, window, cx));
        let local_port_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Port (0 = auto, or e.g. 8787)")
                .default_value("8787")
        });

        let mut view = Self {
            project_id: project.doc.id.clone(),
            project_name: project.doc.name.clone(),
            kind: ContainerKind::Project {
                saved: project.doc.clone(),
                manifest_path: project.manifest_path.clone(),
                project_root: project.root_dir.clone(),
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
            share_settings,
            share_channel: ShareChannel::TestrixServer,
            share_in_flight: false,
            share_status: None,
            project_snapshot: Some(project.clone()),
            ip_select: Some(ip_select),
            local_port_input: Some(local_port_input),
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
            share_settings: ShareSettings::default(),
            share_channel: ShareChannel::TestrixServer,
            share_in_flight: false,
            share_status: None,
            project_snapshot: None,
            ip_select: None,
            local_port_input: None,
            _subscriptions: Vec::new(),
        };
        view.wire_subscriptions(window, cx);
        view
    }

    pub(crate) fn set_share_settings(
        &mut self,
        settings: ShareSettings,
        cx: &mut Context<Self>,
    ) {
        self.share_settings = settings;
        cx.notify();
    }

    fn trigger_share_upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ContainerKind::Project { project_root, .. } = &self.kind else {
            return;
        };
        if self.share_in_flight {
            return;
        }
        let files = match collect_project_yaml_files(project_root) {
            Ok(f) => f,
            Err(err) => {
                self.share_status = Some(Err(err));
                cx.notify();
                return;
            }
        };

        let channel = self.share_channel;
        let project_name = self.name_input.read(cx).value().trim().to_string();
        let project_name = if project_name.is_empty() {
            self.project_name.clone()
        } else {
            project_name
        };
        let project_slug = project_root
            .file_name()
            .and_then(|n| n.to_str())
            .map(slugify)
            .unwrap_or_else(|| slugify(&project_name));
        let settings = self.share_settings.clone();

        let project_id = self.project_id.clone();
        let file_paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();

        if channel == ShareChannel::LocalNetwork {
            let Some(project_node) = self.project_snapshot.clone() else {
                self.share_status = Some(Err("Project snapshot unavailable".to_string()));
                cx.notify();
                return;
            };
            let bind_ip = self
                .ip_select
                .as_ref()
                .and_then(|sel| sel.read(cx).selected_value().cloned())
                .unwrap_or_else(|| "127.0.0.1".to_string());
            let port = self
                .local_port_input
                .as_ref()
                .and_then(|p| p.read(cx).value().trim().parse::<u16>().ok())
                .unwrap_or(0);

            match start_local_share_server(bind_ip, port, project_node, project_slug.clone()) {
                Ok(handle) => {
                    let url = handle.share_url.clone();
                    let bind_addr = handle.bind_addr.clone();
                    self.share_status = Some(Ok(ShareUploadResult {
                        channel: ShareChannel::LocalNetwork,
                        files_uploaded: file_paths.len(),
                        share_url: url.clone(),
                    }));
                    cx.emit(ContainerTabEvent::ShareStarted(ActiveShareRecord {
                        id: format!("{project_id}:local"),
                        project_id: project_id.clone(),
                        project_name,
                        project_slug,
                        channel: ShareChannel::LocalNetwork,
                        share_url: url,
                        target_info: bind_addr,
                        file_paths,
                        local_handle: Some(handle),
                    }));
                }
                Err(err) => {
                    self.share_status = Some(Err(err));
                }
            }
            cx.notify();
            return;
        }

        self.share_in_flight = true;
        self.share_status = None;
        cx.notify();

        let target_info = match channel {
            ShareChannel::TestrixServer => settings.share_server_url.clone(),
            ShareChannel::CloudflareR2 => format!("r2://{}", settings.cloudflare_r2_bucket),
            ShareChannel::LocalNetwork => String::new(),
        };

        let rx = match channel {
            ShareChannel::TestrixServer => dispatch_share_future(upload_to_testrix_server(
                settings.share_server_url,
                settings.share_token,
                project_slug.clone(),
                project_name.clone(),
                settings.share_access_token,
                files,
            )),
            ShareChannel::CloudflareR2 => dispatch_share_future(upload_to_cloudflare_r2(
                settings.cloudflare_account_id,
                settings.cloudflare_r2_bucket,
                settings.cloudflare_api_token,
                settings.cloudflare_pages_url,
                project_slug.clone(),
                settings.share_access_token,
                files,
            )),
            ShareChannel::LocalNetwork => unreachable!(),
        };

        cx.spawn_in(window, async move |weak_self, cx| {
            let outcome = rx.await;
            _ = weak_self.update_in(cx, |this, _, cx| {
                this.share_in_flight = false;
                let res = match outcome {
                    Ok(res) => res,
                    Err(_) => Err("Share upload task cancelled unexpectedly".to_string()),
                };
                if let Ok(ok) = &res {
                    cx.emit(ContainerTabEvent::ShareStarted(ActiveShareRecord {
                        id: format!("{project_id}:{}", ok.channel.label()),
                        project_id: project_id.clone(),
                        project_name: project_name.clone(),
                        project_slug: project_slug.clone(),
                        channel: ok.channel,
                        share_url: ok.share_url.clone(),
                        target_info: target_info.clone(),
                        file_paths: file_paths.clone(),
                        local_handle: None,
                    }));
                }
                this.share_status = Some(res);
                cx.notify();
            });
        })
        .detach();
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

    pub(crate) fn rewrite_variables(
        &mut self,
        pairs: &[(String, String)],
        wrap_braces: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut changed = false;
        if let Some(base_url) = &self.base_url_input {
            let current = base_url.read(cx).value().to_string();
            let updated = if wrap_braces {
                rename_template_variables(&current, pairs)
            } else {
                replace_template_variables(&current, pairs)
            };
            if let Some(updated) = updated {
                base_url.update(cx, |state, cx| state.set_value(updated, window, cx));
                changed = true;
            }
        }
        if let Some(headers) = &self.default_headers {
            changed |= headers.update(cx, |t, cx| t.rewrite_variables(pairs, wrap_braces, window, cx));
        }
        changed |= self
            .auth_editor
            .update(cx, |e, cx| e.rewrite_variables(pairs, wrap_braces, window, cx));

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
                (ContainerSection::Share, "Share"),
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
                    })
                    .when(is_project && self.active_section == ContainerSection::Share, |this| {
                        let channels = [
                            ShareChannel::TestrixServer,
                            ShareChannel::CloudflareR2,
                            ShareChannel::LocalNetwork,
                        ];
                        let settings = &self.share_settings;
                        let is_ready = match self.share_channel {
                            ShareChannel::TestrixServer => {
                                !settings.share_server_url.trim().is_empty()
                                    && !settings.share_token.trim().is_empty()
                            }
                            ShareChannel::CloudflareR2 => {
                                !settings.cloudflare_account_id.trim().is_empty()
                                    && !settings.cloudflare_r2_bucket.trim().is_empty()
                                    && !settings.cloudflare_api_token.trim().is_empty()
                            }
                            ShareChannel::LocalNetwork => self.ip_select.is_some(),
                        };

                        let status_text = match self.share_channel {
                            ShareChannel::TestrixServer => {
                                if settings.share_token.trim().is_empty() {
                                    format!(
                                        "Not logged in (Server: {}). Open Preferences to sign in.",
                                        settings.share_server_url
                                    )
                                } else {
                                    format!(
                                        "Logged in as {} on {}",
                                        settings.share_email, settings.share_server_url
                                    )
                                }
                            }
                            ShareChannel::CloudflareR2 => {
                                if settings.cloudflare_api_token.trim().is_empty()
                                    || settings.cloudflare_account_id.trim().is_empty()
                                    || settings.cloudflare_r2_bucket.trim().is_empty()
                                {
                                    "Cloudflare R2 credentials not configured. Open Preferences to add your Account ID, Bucket, and API Token.".to_string()
                                } else {
                                    format!(
                                        "Ready: R2 bucket \"{}\" (Account {})",
                                        settings.cloudflare_r2_bucket,
                                        settings.cloudflare_account_id
                                    )
                                }
                            }
                            ShareChannel::LocalNetwork => {
                                let selected_ip = self
                                    .ip_select
                                    .as_ref()
                                    .and_then(|sel| sel.read(cx).selected_value().cloned())
                                    .unwrap_or_else(|| "127.0.0.1".to_string());
                                format!("Local HTTP server will bind to outbound IP: {selected_ip}")
                            }
                        };

                        this.child(
                            v_flex()
                                .gap_4()
                                .max_w(px(640.))
                                .child(
                                    v_flex()
                                        .gap_1()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_semibold()
                                                .child("Share Channel"),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("Upload all YAML files in this project (manifest, folders, requests, and environments) to a share destination."),
                                        )
                                        .child(
                                            h_flex().gap_2().pt_1().children(channels.into_iter().map(|ch| {
                                                let active = self.share_channel == ch;
                                                let btn = Button::new(format!("share-ch-{}", ch.label()))
                                                    .small()
                                                    .label(ch.label())
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        this.share_channel = ch;
                                                        this.share_status = None;
                                                        cx.notify();
                                                    }));
                                                if active { btn.primary() } else { btn.outline() }
                                            })),
                                        ),
                                )
                                .child(
                                    div()
                                        .p_3()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(cx.theme().border)
                                        .bg(cx.theme().secondary)
                                        .text_xs()
                                        .text_color(if is_ready {
                                            cx.theme().foreground
                                        } else {
                                            cx.theme().muted_foreground
                                        })
                                        .child(status_text),
                                )
                                .when(self.share_channel == ShareChannel::LocalNetwork, |parent| {
                                    parent.child(
                                        v_flex()
                                            .gap_2()
                                            .child(
                                                h_flex()
                                                    .justify_between()
                                                    .items_center()
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_semibold()
                                                            .child("Outbound Network Adapter IP"),
                                                    )
                                                    .child(
                                                        Button::new("refresh-ips-btn")
                                                            .xsmall()
                                                            .outline()
                                                            .label("Refresh IPs")
                                                            .on_click(cx.listener(|this, _, window, cx| {
                                                                let options: Vec<AdapterIpOption> =
                                                                    list_outbound_ipv4_addresses()
                                                                        .iter()
                                                                        .map(AdapterIpOption::from_adapter)
                                                                        .collect();
                                                                let sel_ix = if options.is_empty() {
                                                                    None
                                                                } else {
                                                                    Some(IndexPath::default())
                                                                };
                                                                if let Some(ip_sel) = &this.ip_select {
                                                                    ip_sel.update(cx, |state, cx| {
                                                                        state.set_items(options, window, cx);
                                                                        state.set_selected_index(sel_ix, window, cx);
                                                                    });
                                                                }
                                                                cx.notify();
                                                            })),
                                                    ),
                                            )
                                            .when_some(self.ip_select.as_ref(), |col, ip_sel| {
                                                col.child(
                                                    Select::new(ip_sel)
                                                        .id("local-ip-select")
                                                        .small()
                                                        .placeholder("Select outbound IP"),
                                                )
                                            })
                                            .when_some(self.local_port_input.as_ref(), |col, port_in| {
                                                col.child(
                                                    h_flex()
                                                        .gap_2()
                                                        .items_center()
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(cx.theme().muted_foreground)
                                                                .child("Port:"),
                                                        )
                                                        .child(div().w(px(140.)).child(Input::new(port_in).small())),
                                                )
                                            }),
                                    )
                                })
                                .child(
                                    h_flex().gap_2().child(
                                        Button::new("share-upload-btn")
                                            .primary()
                                            .disabled(self.share_in_flight || !is_ready)
                                            .label(if self.share_in_flight {
                                                "Uploading YAML…"
                                            } else if self.share_channel == ShareChannel::LocalNetwork {
                                                "Start Local Share"
                                            } else {
                                                "Upload Project YAML"
                                            })
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.trigger_share_upload(window, cx);
                                            })),
                                    ),
                                )
                                .when_some(self.share_status.as_ref(), |parent, result| {
                                    match result {
                                        Ok(ok) => {
                                            let url = ok.share_url.clone();
                                            let url_for_copy = url.clone();
                                            parent.child(
                                                v_flex()
                                                    .gap_2()
                                                    .p_3()
                                                    .rounded_md()
                                                    .border_1()
                                                    .border_color(cx.theme().border)
                                                    .bg(cx.theme().secondary)
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .font_semibold()
                                                            .child(format!(
                                                                "Uploaded {} YAML file(s) via {}",
                                                                ok.files_uploaded,
                                                                ok.channel.label()
                                                            )),
                                                    )
                                                    .child(
                                                        h_flex()
                                                            .gap_2()
                                                            .items_center()
                                                            .child(
                                                                div()
                                                                    .text_xs()
                                                                    .flex_1()
                                                                    .child(url),
                                                            )
                                                            .child(
                                                                Button::new("copy-share-url-btn")
                                                                    .small()
                                                                    .outline()
                                                                    .label("Copy URL")
                                                                    .on_click(move |_, window, cx| {
                                                                        cx.write_to_clipboard(
                                                                            ClipboardItem::new_string(
                                                                                url_for_copy.clone(),
                                                                            ),
                                                                        );
                                                                        window.push_notification(
                                                                            "Share URL copied to clipboard",
                                                                            cx,
                                                                        );
                                                                    }),
                                                            ),
                                                    ),
                                            )
                                        }
                                        Err(err) => parent.child(
                                            div()
                                                .p_3()
                                                .rounded_md()
                                                .border_1()
                                                .border_color(cx.theme().danger)
                                                .text_xs()
                                                .text_color(cx.theme().danger)
                                                .child(err.clone()),
                                        ),
                                    }
                                }),
                        )
                    }),
            )
    }
}
