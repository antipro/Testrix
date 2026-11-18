use std::{cell::Cell, collections::HashSet, path::PathBuf};

use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, MouseButton, ParentElement as _, PathPromptOptions, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, WindowOptions,
    assets::IconName as LucideIcon,
    base::TestSupportExt as _,
    component::{
        ActiveTheme as _, Icon, IconName, IndexPath, Placement, Selectable as _, Sizable as _,
        StyledExt as _, Theme, ThemeMode, TitleBar, WindowExt as _,
        button::{Button, ButtonVariant, ButtonVariants as _},
        dialog::{DialogClose, DialogFooter},
        empty::{
            Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant,
            EmptyTitle,
        },
        form::{Field, Form},
        h_flex,
        input::{Input, InputState},
        resizable::{h_resizable, resizable_panel},
        select::{Select, SelectEvent, SelectItem, SelectState},
        status_bar::StatusBar,
        tab::{Tab, TabBar},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px, size,
};
use testrix_core::{
    AuthConfig, AuthNodeRef, ExplorerNode, FolderNode, HttpMethod, HttpRequestDoc, KeyValueRow,
    ProjectNode, RequestBody, WorkspaceTree, rename_template_variables, resolve_request_auth,
};
use testrix_http::HttpRunner;
use testrix_storage::{
    create_environment, create_folder, create_project, create_request, delete_environment,
    delete_folder, delete_request, duplicate_request, ensure_seeded_workspace, load_workspace,
    save_config_workspace_dir, save_environment, save_folder, save_project, save_request,
    save_workspace_state,
};
use tracing::{error, info};

use crate::{
    actions::{
        APP_KEY_CONTEXT, CancelRequest, CloseActiveTab, FocusQuickSearch, NewFolder, NewProject,
        NewRequest, Quit, ReloadWorkspace, SaveActive, SendRequest, ToggleEnvSheet, ToggleTheme,
    },
    container_editor::{ContainerTabEvent, ContainerTabView},
    env_editor::{EnvEditorEvent, EnvEditorView},
    explorer::{ExplorerEvent, ExplorerPanel},
    request_editor::{RequestTabEvent, RequestTabView},
    theme_helpers::method_badge,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProfileOption {
    env_id: Option<String>,
    label: SharedString,
    var_count: usize,
}

impl SelectItem for ProfileOption {
    type Value = Option<String>;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.env_id
    }
}

#[derive(Clone)]
enum WorkbenchTab {
    Request(Entity<RequestTabView>),
    Container(Entity<ContainerTabView>),
}

impl WorkbenchTab {
    fn key(&self, cx: &App) -> String {
        match self {
            Self::Request(view) => format!("req:{}", view.read(cx).request_id()),
            Self::Container(view) => view.read(cx).tab_key(),
        }
    }

    fn title(&self, cx: &App) -> String {
        match self {
            Self::Request(view) => view.read(cx).title(cx),
            Self::Container(view) => view.read(cx).title(cx),
        }
    }

    fn is_dirty(&self, cx: &App) -> bool {
        match self {
            Self::Request(view) => view.read(cx).is_dirty(),
            Self::Container(view) => view.read(cx).is_dirty(),
        }
    }
}

/// Top-level Testrix desktop workbench window view.
pub struct TestrixApp {
    focus_handle: FocusHandle,
    workspace_dir: PathBuf,
    workspace: WorkspaceTree,
    runner: HttpRunner,
    explorer: Entity<ExplorerPanel>,
    env_editor: Entity<EnvEditorView>,
    profile_select: Entity<SelectState<Vec<ProfileOption>>>,
    tabs: Vec<WorkbenchTab>,
    active_tab_ix: Option<usize>,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for TestrixApp {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TestrixApp {
    pub fn new(workspace_dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let workspace = match ensure_seeded_workspace(&workspace_dir) {
            Ok(tree) => tree,
            Err(err) => {
                error!(error = %err, "failed to load workspace");
                WorkspaceTree {
                    root_dir: workspace_dir.clone(),
                    ..Default::default()
                }
            }
        };

        let runner = HttpRunner::new().expect("failed to initialize HTTP runner");
        let explorer = cx.new(|cx| ExplorerPanel::new(window, cx));
        let env_editor = cx.new(|cx| EnvEditorView::new(window, cx));

        let (profile_items, selected_ix) = build_profile_options(&workspace);
        let profile_select = cx.new(|cx| SelectState::new(profile_items, selected_ix, window, cx));

        let mut app = Self {
            focus_handle: cx.focus_handle(),
            workspace_dir,
            workspace,
            runner,
            explorer,
            env_editor,
            profile_select,
            tabs: Vec::new(),
            active_tab_ix: None,
            _subscriptions: Vec::new(),
        };

        app.wire_subscriptions(window, cx);
        app.explorer.update(cx, |panel, cx| {
            panel.set_workspace(app.workspace.clone(), cx);
        });
        app.sync_env_editor(window, cx);
        app.open_initial_request(window, cx);
        app
    }

    pub fn workspace_dir(&self) -> &PathBuf {
        &self.workspace_dir
    }

    pub fn workspace(&self) -> &WorkspaceTree {
        &self.workspace
    }

    pub fn open_tab_count(&self) -> usize {
        self.tabs.len()
    }

    pub fn active_tab_key(&self, cx: &App) -> Option<String> {
        self.active_tab_ix
            .and_then(|ix| self.tabs.get(ix))
            .map(|tab| tab.key(cx))
    }

    fn wire_subscriptions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut subs = Vec::new();

        subs.push(cx.subscribe_in(
            &self.profile_select,
            window,
            |this, _, event: &SelectEvent<Vec<ProfileOption>>, window, cx| {
                if let SelectEvent::Confirm(Some(env_id)) = event {
                    this.select_environment(env_id.clone(), window, cx);
                }
            },
        ));

        subs.push(cx.subscribe_in(
            &self.explorer,
            window,
            |this, _, event: &ExplorerEvent, window, cx| {
                this.handle_explorer_event(event.clone(), window, cx);
            },
        ));

        subs.push(cx.subscribe_in(
            &self.env_editor,
            window,
            |this, _, event: &EnvEditorEvent, window, cx| {
                this.handle_env_editor_event(event.clone(), window, cx);
            },
        ));

        self._subscriptions = subs;
    }

    fn open_initial_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(proj) = self.workspace.active_project() {
            let first_req = proj
                .requests
                .first()
                .or_else(|| proj.folders.iter().find_map(|f| first_request_in_folder(f)));
            if let Some(req) = first_req {
                let project_id = proj.doc.id.clone();
                let request_id = req.doc.id.clone();
                self.open_node(
                    ExplorerNode::Request {
                        project_id,
                        request_id,
                    },
                    window,
                    cx,
                );
            }
        }
    }

    fn handle_explorer_event(
        &mut self,
        event: ExplorerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ExplorerEvent::OpenNode(node) => self.open_node(node, window, cx),
            ExplorerEvent::NewRequestIn {
                project_id,
                folder_id,
            } => self.open_new_request_dialog(project_id, folder_id, window, cx),
            ExplorerEvent::NewFolderIn {
                project_id,
                parent_folder_id,
            } => self.open_new_folder_dialog(project_id, parent_folder_id, window, cx),
            ExplorerEvent::NewProject => self.open_new_project_dialog(window, cx),
            ExplorerEvent::DuplicateRequest {
                project_id,
                request_id,
            } => self.duplicate_request_node(&project_id, &request_id, window, cx),
            ExplorerEvent::DeleteRequest {
                project_id,
                request_id,
            } => self.confirm_delete_request(project_id, request_id, window, cx),
            ExplorerEvent::DeleteFolder {
                project_id,
                folder_id,
            } => self.confirm_delete_folder(project_id, folder_id, window, cx),
            ExplorerEvent::ReloadWorkspace => self.reload_workspace(window, cx),
        }
    }

    fn handle_env_editor_event(
        &mut self,
        event: EnvEditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            EnvEditorEvent::Save {
                env_id,
                updated,
                renamed_keys,
            } => {
                let is_active =
                    self.workspace.active_environment_id.as_deref() == Some(&env_id);
                if let Some(proj) = self.workspace.active_project_mut()
                    && let Some(env_node) =
                        proj.environments.iter_mut().find(|e| e.doc.id == env_id)
                {
                    env_node.doc = updated;
                    if let Err(err) = save_environment(env_node) {
                        window.push_notification(format!("Failed saving profile: {err}"), cx);
                        return;
                    }
                    proj.environments.sort_by_key(|a| a.doc.name.to_lowercase());
                    if is_active && !renamed_keys.is_empty() {
                        propagate_variable_renames_in_project(proj, &renamed_keys);
                    }
                    self.refresh_profile_selector(window, cx);
                    self.sync_env_editor(window, cx);
                    if is_active && !renamed_keys.is_empty() {
                        self.rename_variables_in_open_tabs(&renamed_keys, window, cx);
                    }
                    self.sync_all_tabs(window, cx);
                    window.push_notification("Profile saved.", cx);
                }
            }
            EnvEditorEvent::Create { name } => {
                let Some(project_root) =
                    self.workspace.active_project().map(|p| p.root_dir.clone())
                else {
                    return;
                };
                match create_environment(&project_root, &name) {
                    Ok(env_node) => {
                        let new_id = env_node.doc.id.clone();
                        if let Some(proj) = self.workspace.active_project_mut() {
                            proj.environments.push(env_node);
                            proj.environments.sort_by_key(|a| a.doc.name.to_lowercase());
                        }
                        self.select_environment(Some(new_id.clone()), window, cx);
                        self.sync_env_editor(window, cx);
                        self.env_editor.update(cx, |editor, cx| {
                            editor.focus_profile(&new_id, window, cx);
                        });
                    }
                    Err(err) => {
                        window.push_notification(format!("Failed creating profile: {err}"), cx);
                    }
                }
            }
            EnvEditorEvent::Delete { env_id } => {
                let Some(proj) = self.workspace.active_project_mut() else {
                    return;
                };
                let Some(pos) = proj.environments.iter().position(|e| e.doc.id == env_id) else {
                    return;
                };
                let removed = proj.environments.remove(pos);
                if let Err(err) = delete_environment(&removed.file_path) {
                    window.push_notification(format!("Failed deleting profile: {err}"), cx);
                    proj.environments.insert(pos, removed);
                    return;
                }
                if self.workspace.active_environment_id.as_deref() == Some(&env_id) {
                    self.workspace.active_environment_id = self
                        .workspace
                        .active_project()
                        .and_then(|p| p.environments.first())
                        .map(|e| e.doc.id.clone());
                    let _ = save_workspace_state(
                        &self.workspace_dir,
                        self.workspace.active_project_id.as_deref(),
                        self.workspace.active_environment_id.as_deref(),
                    );
                }
                self.refresh_profile_selector(window, cx);
                self.sync_env_editor(window, cx);
                self.sync_all_tabs(window, cx);
            }
            EnvEditorEvent::SelectActive { env_id } => {
                self.select_environment(env_id, window, cx);
                self.sync_env_editor(window, cx);
            }
        }
    }

    fn select_environment(
        &mut self,
        env_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace.active_environment_id = env_id;
        let _ = save_workspace_state(
            &self.workspace_dir,
            self.workspace.active_project_id.as_deref(),
            self.workspace.active_environment_id.as_deref(),
        );
        self.refresh_profile_selector(window, cx);
        self.sync_all_tabs(window, cx);
        cx.notify();
    }

    fn refresh_profile_selector(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (items, selected_ix) = build_profile_options(&self.workspace);
        self.profile_select.update(cx, |select, cx| {
            select.set_items(items, window, cx);
            select.set_selected_index(selected_ix, window, cx);
        });
    }

    fn sync_env_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let profiles = self
            .workspace
            .active_project()
            .map(|p| p.environments.clone())
            .unwrap_or_default();
        let active_id = self.workspace.active_environment_id.clone();
        self.env_editor.update(cx, |editor, cx| {
            editor.sync_profiles(profiles, active_id, window, cx);
        });
    }

    pub fn open_node(&mut self, node: ExplorerNode, window: &mut Window, cx: &mut Context<Self>) {
        let target_key = match &node {
            ExplorerNode::Project { project_id } => format!("proj:{project_id}"),
            ExplorerNode::Folder { folder_id, .. } => format!("fld:{folder_id}"),
            ExplorerNode::Request { request_id, .. } => format!("req:{request_id}"),
        };

        if let Some(existing_ix) = self.tabs.iter().position(|tab| tab.key(cx) == target_key) {
            self.active_tab_ix = Some(existing_ix);
            self.explorer
                .update(cx, |panel, cx| panel.reveal_and_select(&target_key, cx));
            cx.notify();
            return;
        }

        let active_env = self.workspace.active_environment().map(|e| e.doc.clone());

        let new_tab = match node {
            ExplorerNode::Request {
                project_id,
                request_id,
            } => {
                let Some(project) = self.workspace.find_project(&project_id) else {
                    return;
                };
                let Some((ancestors, req_node)) = project.find_request(&request_id) else {
                    return;
                };
                let runner = self.runner.clone();
                let view = cx.new(|cx| {
                    RequestTabView::new(
                        project, &ancestors, req_node, active_env, runner, window, cx,
                    )
                });
                let sub = cx.subscribe_in(
                    &view,
                    window,
                    |this, _, event: &RequestTabEvent, window, cx| {
                        this.handle_request_tab_event(event.clone(), window, cx);
                    },
                );
                self._subscriptions.push(sub);
                WorkbenchTab::Request(view)
            }
            ExplorerNode::Project { project_id } => {
                let Some(project) = self.workspace.find_project(&project_id) else {
                    return;
                };
                let view =
                    cx.new(|cx| ContainerTabView::new_project(project, active_env, window, cx));
                let sub = cx.subscribe_in(
                    &view,
                    window,
                    |this, _, event: &ContainerTabEvent, window, cx| {
                        this.handle_container_tab_event(event.clone(), window, cx);
                    },
                );
                self._subscriptions.push(sub);
                WorkbenchTab::Container(view)
            }
            ExplorerNode::Folder {
                project_id,
                folder_id,
            } => {
                let Some(project) = self.workspace.find_project(&project_id) else {
                    return;
                };
                let Some(chain) = project.find_folder(&folder_id) else {
                    return;
                };
                let view = cx.new(|cx| {
                    ContainerTabView::new_folder(project, &chain, active_env, window, cx)
                });
                let sub = cx.subscribe_in(
                    &view,
                    window,
                    |this, _, event: &ContainerTabEvent, window, cx| {
                        this.handle_container_tab_event(event.clone(), window, cx);
                    },
                );
                self._subscriptions.push(sub);
                WorkbenchTab::Container(view)
            }
        };

        self.tabs.push(new_tab);
        self.active_tab_ix = Some(self.tabs.len() - 1);
        self.explorer
            .update(cx, |panel, cx| panel.reveal_and_select(&target_key, cx));
        self.refresh_dirty_indicators(cx);
        cx.notify();
    }

    fn handle_request_tab_event(
        &mut self,
        event: RequestTabEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            RequestTabEvent::SaveRequest {
                project_id,
                request_id,
                doc,
            } => {
                if let Some(proj) = self.workspace.find_project_mut(&project_id)
                    && let Some(req_node) = proj.find_request_mut(&request_id)
                {
                    req_node.doc = *doc;
                    if let Err(err) = save_request(req_node) {
                        window.push_notification(format!("Failed saving YAML request: {err}"), cx);
                        return;
                    }
                    let target_key = format!("req:{request_id}");
                    if let Some(WorkbenchTab::Request(tab)) =
                        self.tabs.iter().find(|t| t.key(cx) == target_key)
                    {
                        tab.update(cx, |view, cx| view.mark_saved(cx));
                    }
                    self.explorer.update(cx, |panel, cx| {
                        panel.set_workspace(self.workspace.clone(), cx);
                        panel.reveal_and_select(&target_key, cx);
                    });
                    self.refresh_dirty_indicators(cx);
                }
            }
            RequestTabEvent::OpenAuthSource { project_id, node } => {
                self.open_auth_source(&project_id, node, window, cx);
            }
            RequestTabEvent::DirtyChanged => {
                self.refresh_dirty_indicators(cx);
            }
        }
    }

    fn handle_container_tab_event(
        &mut self,
        event: ContainerTabEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ContainerTabEvent::SaveProject { project_id, doc } => {
                if let Some(proj) = self.workspace.find_project_mut(&project_id) {
                    proj.doc = doc;
                    if let Err(err) = save_project(proj) {
                        window.push_notification(format!("Failed saving project: {err}"), cx);
                        return;
                    }
                    let target_key = format!("proj:{project_id}");
                    if let Some(WorkbenchTab::Container(tab)) =
                        self.tabs.iter().find(|t| t.key(cx) == target_key)
                    {
                        tab.update(cx, |view, cx| view.mark_saved(cx));
                    }
                    self.explorer.update(cx, |panel, cx| {
                        panel.set_workspace(self.workspace.clone(), cx);
                        panel.reveal_and_select(&target_key, cx);
                    });
                    self.sync_all_tabs(window, cx);
                    self.refresh_dirty_indicators(cx);
                }
            }
            ContainerTabEvent::SaveFolder {
                project_id,
                folder_id,
                doc,
            } => {
                if let Some(proj) = self.workspace.find_project_mut(&project_id)
                    && let Some(folder) = proj.find_folder_mut(&folder_id)
                {
                    folder.doc = doc;
                    if let Err(err) = save_folder(folder) {
                        window.push_notification(format!("Failed saving folder: {err}"), cx);
                        return;
                    }
                    folder.has_manifest = true;
                    let target_key = format!("fld:{folder_id}");
                    if let Some(WorkbenchTab::Container(tab)) =
                        self.tabs.iter().find(|t| t.key(cx) == target_key)
                    {
                        tab.update(cx, |view, cx| view.mark_saved(cx));
                    }
                    self.explorer.update(cx, |panel, cx| {
                        panel.set_workspace(self.workspace.clone(), cx);
                        panel.reveal_and_select(&target_key, cx);
                    });
                    self.sync_all_tabs(window, cx);
                    self.refresh_dirty_indicators(cx);
                }
            }
            ContainerTabEvent::OpenAuthSource(node) => {
                if let Some(project_id) = self.workspace.active_project().map(|p| p.doc.id.clone())
                {
                    self.open_auth_source(&project_id, node, window, cx);
                }
            }
            ContainerTabEvent::DirtyChanged => {
                self.refresh_dirty_indicators(cx);
            }
        }
    }

    fn open_auth_source(
        &mut self,
        project_id: &str,
        node: AuthNodeRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match node {
            AuthNodeRef::Project { id, .. } => {
                self.open_node(ExplorerNode::Project { project_id: id }, window, cx);
            }
            AuthNodeRef::Folder { id, .. } => {
                self.open_node(
                    ExplorerNode::Folder {
                        project_id: project_id.to_string(),
                        folder_id: id,
                    },
                    window,
                    cx,
                );
            }
            AuthNodeRef::Request { id, .. } => {
                self.open_node(
                    ExplorerNode::Request {
                        project_id: project_id.to_string(),
                        request_id: id,
                    },
                    window,
                    cx,
                );
            }
        }
    }

    fn rename_variables_in_open_tabs(
        &mut self,
        renames: &[(String, String)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active_project_id = self
            .workspace
            .active_project()
            .map(|p| p.doc.id.clone());
        for tab in &self.tabs {
            match tab {
                WorkbenchTab::Request(view) => {
                    let belongs_to_active = active_project_id
                        .as_deref()
                        .is_some_and(|pid| pid == view.read(cx).project_id());
                    if belongs_to_active {
                        view.update(cx, |v, cx| {
                            v.rename_variables(renames, window, cx);
                        });
                    }
                }
                WorkbenchTab::Container(view) => {
                    view.update(cx, |v, cx| {
                        v.rename_variables(renames, window, cx);
                    });
                }
            }
        }
    }

    fn sync_all_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active_env = self.workspace.active_environment().map(|e| e.doc.clone());
        for tab in &self.tabs {
            match tab {
                WorkbenchTab::Request(view) => {
                    let project_id = view.read(cx).project_id().to_string();
                    if let Some(project) = self.workspace.find_project(&project_id) {
                        let env = active_env.clone();
                        view.update(cx, |v, cx| v.sync_context(project, env, window, cx));
                    }
                }
                WorkbenchTab::Container(view) => {
                    let key = view.read(cx).tab_key();
                    let project = if let Some(pid) = key.strip_prefix("proj:") {
                        self.workspace.find_project(pid)
                    } else if let Some(fid) = key.strip_prefix("fld:") {
                        self.workspace.find_folder(fid).map(|(p, _)| p)
                    } else {
                        None
                    };
                    if let Some(project) = project {
                        let env = active_env.clone();
                        view.update(cx, |v, cx| v.sync_context(project, env, window, cx));
                    }
                }
            }
        }
    }

    fn refresh_dirty_indicators(&mut self, cx: &mut Context<Self>) {
        let dirty: HashSet<String> = self
            .tabs
            .iter()
            .filter(|tab| tab.is_dirty(cx))
            .map(|tab| tab.key(cx))
            .collect();
        self.explorer
            .update(cx, |panel, cx| panel.set_dirty_keys(dirty, cx));
    }

    fn close_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        self.tabs.remove(ix);
        self.active_tab_ix = if self.tabs.is_empty() {
            None
        } else if let Some(active) = self.active_tab_ix {
            if active == ix {
                Some(ix.min(self.tabs.len() - 1))
            } else if active > ix {
                Some(active - 1)
            } else {
                Some(active)
            }
        } else {
            None
        };
        self.refresh_dirty_indicators(cx);
        cx.notify();
    }

    fn reload_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match load_workspace(&self.workspace_dir) {
            Ok(tree) => {
                self.workspace = tree;
                self.explorer.update(cx, |panel, cx| {
                    panel.set_workspace(self.workspace.clone(), cx)
                });
                self.refresh_profile_selector(window, cx);
                self.sync_env_editor(window, cx);
                self.sync_all_tabs(window, cx);
                window.push_notification("Reloaded YAML workspace from disk.", cx);
            }
            Err(err) => {
                window.push_notification(format!("Reload failed: {err}"), cx);
            }
        }
    }

    fn choose_workspace_dir(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current_dir = self.workspace_dir.to_string_lossy().to_string();
        let path_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("/path/to/testrix-workspace")
                .default_value(current_dir)
        });
        let app_entity = cx.entity();

        window.open_dialog(cx, move |dialog, _, _| {
            let path_browse = path_input.clone();
            let path_ok = path_input.clone();
            let app_ok = app_entity.clone();
            dialog
                .title("Workspace Directory")
                .w(px(520.))
                .child(
                    v_flex().gap_2().child(
                        Form::new().child(
                            Field::new()
                                .label("Workspace Path")
                                .description(
                                    "Saved in ~/.testrix/config.yaml; empty folders are seeded automatically.",
                                )
                                .child(
                                    h_flex()
                                        .w_full()
                                        .gap_2()
                                        .child(
                                            div().flex_1().min_w_0().child(
                                                Input::new(&path_input)
                                                    .id("workspace-dir-input")
                                                    .w_full(),
                                            ),
                                        )
                                        .child(
                                            Button::new("workspace-browse-btn")
                                                .outline()
                                                .icon(LucideIcon::FolderOpen)
                                                .label("Browse…")
                                                .on_click(move |_, window, cx| {
                                                    let prompt = cx.prompt_for_paths(PathPromptOptions {
                                                        files: false,
                                                        directories: true,
                                                        multiple: false,
                                                        prompt: Some(
                                                            "Select Testrix Workspace Folder"
                                                                .into(),
                                                        ),
                                                    });
                                                    let path_browse = path_browse.clone();
                                                    window
                                                        .spawn(cx, async move |window| {
                                                            match prompt.await {
                                                            Ok(Ok(Some(paths))) => {
                                                                if let Some(dir) =
                                                                    paths.into_iter().next()
                                                                {
                                                                    let _ = window.update(
                                                                        |window, cx| {
                                                                            path_browse.update(
                                                                                cx,
                                                                                |state, cx| {
                                                                                    state.set_value(
                                                                                        dir.to_string_lossy()
                                                                                            .to_string(),
                                                                                        window,
                                                                                        cx,
                                                                                    );
                                                                                },
                                                                            );
                                                                        },
                                                                    );
                                                                }
                                                            }
                                                            Ok(Err(err)) => {
                                                                tracing::warn!(error = %err, "folder picker failed");
                                                            }
                                                            _ => {}
                                                        }
                                                    })
                                                    .detach();
                                                }),
                                        ),
                                ),
                        ),
                    ),
                )
                .footer(
                    DialogFooter::new()
                        .gap_2()
                        .child(
                            DialogClose::new().child(
                                Button::new("workspace-cancel-btn")
                                    .label("Cancel")
                                    .outline(),
                            ),
                        )
                        .child(
                            Button::new("workspace-apply-btn")
                                .primary()
                                .label("Switch Workspace")
                                .on_click(move |_, window, cx| {
                                    let raw = path_ok.read(cx).value().trim().to_string();
                                    if raw.is_empty() {
                                        window.push_notification(
                                            "Workspace path must not be empty.",
                                            cx,
                                        );
                                        return;
                                    }
                                    app_ok.update(cx, |this, cx| {
                                        this.switch_workspace_dir(PathBuf::from(raw), window, cx);
                                    });
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
    }

    fn switch_workspace_dir(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match ensure_seeded_workspace(&dir) {
            Ok(tree) => {
                info!(dir = %dir.display(), "switched workspace directory");
                if let Err(err) = save_config_workspace_dir(&dir) {
                    tracing::warn!(error = %err, "failed updating ~/.testrix/config.yaml");
                }
                self.workspace_dir = dir;
                self.workspace = tree;
                self.tabs.clear();
                self.active_tab_ix = None;
                self.explorer.update(cx, |panel, cx| {
                    panel.set_workspace(self.workspace.clone(), cx)
                });
                self.refresh_profile_selector(window, cx);
                self.sync_env_editor(window, cx);
                self.open_initial_request(window, cx);
                cx.notify();
            }
            Err(err) => {
                window.push_notification(format!("Failed opening workspace: {err}"), cx);
            }
        }
    }

    fn open_env_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_env_editor(window, cx);
        let env_editor = self.env_editor.clone();
        window.open_sheet_at(Placement::Right, cx, move |sheet, _, _| {
            sheet
                .title("Profiles & Environment Variables")
                .size(px(580.))
                .child(div().size_full().p_4().child(env_editor.clone()))
        });
    }

    fn open_new_request_dialog(
        &mut self,
        project_id: String,
        folder_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Get Orders"));
        let url_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("{{base_url}}/v1/orders")
                .default_value("{{base_url}}/get")
        });
        let app_entity = cx.entity();
        let initial_focus = Cell::new(true);

        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                name_input.focus_handle(cx).focus(window, cx);
            }
            let app_ok = app_entity.clone();
            let name_ok = name_input.clone();
            let url_ok = url_input.clone();
            let pid = project_id.clone();
            let fid = folder_id.clone();

            dialog
                .title("New HTTP Request")
                .child(
                    Form::new()
                        .child(Field::new().label("Name").child(Input::new(&name_input)))
                        .child(Field::new().label("URL").child(Input::new(&url_input))),
                )
                .footer(
                    DialogFooter::new()
                        .gap_2()
                        .child(
                            DialogClose::new()
                                .child(Button::new("new-req-cancel").label("Cancel").outline()),
                        )
                        .child(
                            Button::new("new-req-create")
                                .label("Create")
                                .primary()
                                .on_click(move |_, window, cx| {
                                    let name = name_ok.read(cx).value().trim().to_string();
                                    let url = url_ok.read(cx).value().trim().to_string();
                                    app_ok.update(cx, |this, cx| {
                                        this.create_request_node(
                                            &pid,
                                            fid.as_deref(),
                                            &name,
                                            &url,
                                            window,
                                            cx,
                                        );
                                    });
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
    }

    fn create_request_node(
        &mut self,
        project_id: &str,
        folder_id: Option<&str>,
        name: &str,
        url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(proj) = self.workspace.find_project(project_id) else {
            return;
        };
        let parent_dir = folder_id
            .and_then(|fid| proj.find_folder(fid))
            .and_then(|chain| chain.last().map(|f| f.dir_path.clone()))
            .unwrap_or_else(|| proj.root_dir.clone());

        match create_request(&parent_dir, name, HttpMethod::Get, url) {
            Ok(req_node) => {
                let req_id = req_node.doc.id.clone();
                if let Some(proj_mut) = self.workspace.find_project_mut(project_id) {
                    if let Some(fid) = folder_id
                        && let Some(folder_mut) = proj_mut.find_folder_mut(fid)
                    {
                        folder_mut.requests.push(req_node);
                    } else {
                        proj_mut.requests.push(req_node);
                    }
                }
                self.explorer.update(cx, |panel, cx| {
                    panel.set_workspace(self.workspace.clone(), cx);
                });
                self.open_node(
                    ExplorerNode::Request {
                        project_id: project_id.to_string(),
                        request_id: req_id,
                    },
                    window,
                    cx,
                );
            }
            Err(err) => {
                window.push_notification(format!("Failed creating request: {err}"), cx);
            }
        }
    }

    fn open_new_folder_dialog(
        &mut self,
        project_id: String,
        parent_folder_id: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Orders"));
        let app_entity = cx.entity();
        let initial_focus = Cell::new(true);

        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                name_input.focus_handle(cx).focus(window, cx);
            }
            let app_ok = app_entity.clone();
            let name_ok = name_input.clone();
            let pid = project_id.clone();
            let fid = parent_folder_id.clone();

            dialog
                .title("New Folder")
                .child(
                    Form::new().child(
                        Field::new()
                            .label("Folder Name")
                            .child(Input::new(&name_input)),
                    ),
                )
                .footer(
                    DialogFooter::new()
                        .gap_2()
                        .child(
                            DialogClose::new()
                                .child(Button::new("new-fld-cancel").label("Cancel").outline()),
                        )
                        .child(
                            Button::new("new-fld-create")
                                .label("Create")
                                .primary()
                                .on_click(move |_, window, cx| {
                                    let name = name_ok.read(cx).value().trim().to_string();
                                    app_ok.update(cx, |this, cx| {
                                        this.create_folder_node(
                                            &pid,
                                            fid.as_deref(),
                                            &name,
                                            window,
                                            cx,
                                        );
                                    });
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
    }

    fn create_folder_node(
        &mut self,
        project_id: &str,
        parent_folder_id: Option<&str>,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(proj) = self.workspace.find_project(project_id) else {
            return;
        };
        let parent_dir = parent_folder_id
            .and_then(|fid| proj.find_folder(fid))
            .and_then(|chain| chain.last().map(|f| f.dir_path.clone()))
            .unwrap_or_else(|| proj.root_dir.clone());

        match create_folder(&parent_dir, name, 0) {
            Ok(folder_node) => {
                let fid = folder_node.doc.id.clone();
                if let Some(proj_mut) = self.workspace.find_project_mut(project_id) {
                    if let Some(parent_id) = parent_folder_id
                        && let Some(parent_mut) = proj_mut.find_folder_mut(parent_id)
                    {
                        parent_mut.folders.push(folder_node);
                    } else {
                        proj_mut.folders.push(folder_node);
                    }
                }
                self.explorer.update(cx, |panel, cx| {
                    panel.set_workspace(self.workspace.clone(), cx);
                });
                self.open_node(
                    ExplorerNode::Folder {
                        project_id: project_id.to_string(),
                        folder_id: fid,
                    },
                    window,
                    cx,
                );
            }
            Err(err) => {
                window.push_notification(format!("Failed creating folder: {err}"), cx);
            }
        }
    }

    fn open_new_project_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("Payments API"));
        let base_url_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://api.example.com or {{base_url}}")
                .default_value("{{base_url}}")
        });
        let app_entity = cx.entity();
        let initial_focus = Cell::new(true);

        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                name_input.focus_handle(cx).focus(window, cx);
            }
            let app_ok = app_entity.clone();
            let name_ok = name_input.clone();
            let url_ok = base_url_input.clone();

            dialog
                .title("New Project")
                .child(
                    Form::new()
                        .child(
                            Field::new()
                                .label("Project Name")
                                .child(Input::new(&name_input)),
                        )
                        .child(
                            Field::new()
                                .label("Base URL")
                                .child(Input::new(&base_url_input)),
                        ),
                )
                .footer(
                    DialogFooter::new()
                        .gap_2()
                        .child(
                            DialogClose::new()
                                .child(Button::new("new-proj-cancel").label("Cancel").outline()),
                        )
                        .child(
                            Button::new("new-proj-create")
                                .label("Create")
                                .primary()
                                .on_click(move |_, window, cx| {
                                    let name = name_ok.read(cx).value().trim().to_string();
                                    let base_url = url_ok.read(cx).value().trim().to_string();
                                    app_ok.update(cx, |this, cx| {
                                        this.create_project_node(&name, &base_url, window, cx);
                                    });
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
    }

    fn create_project_node(
        &mut self,
        name: &str,
        base_url: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match create_project(&self.workspace_dir, name, base_url) {
            Ok(proj_node) => {
                let pid = proj_node.doc.id.clone();
                let first_env = proj_node.environments.first().map(|e| e.doc.id.clone());
                self.workspace.projects.push(proj_node);
                self.workspace.active_project_id = Some(pid.clone());
                self.workspace.active_environment_id = first_env;
                let _ = save_workspace_state(
                    &self.workspace_dir,
                    self.workspace.active_project_id.as_deref(),
                    self.workspace.active_environment_id.as_deref(),
                );
                self.explorer.update(cx, |panel, cx| {
                    panel.set_workspace(self.workspace.clone(), cx);
                });
                self.refresh_profile_selector(window, cx);
                self.sync_env_editor(window, cx);
                self.open_node(ExplorerNode::Project { project_id: pid }, window, cx);
            }
            Err(err) => {
                window.push_notification(format!("Failed creating project: {err}"), cx);
            }
        }
    }

    fn duplicate_request_node(
        &mut self,
        project_id: &str,
        request_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((project, ancestors, req_node)) = self.workspace.find_request(request_id) else {
            return;
        };
        if project.doc.id != project_id {
            return;
        }
        let parent_folder_id = ancestors.last().map(|f| f.doc.id.clone());

        match duplicate_request(req_node) {
            Ok(copy_node) => {
                let copy_id = copy_node.doc.id.clone();
                if let Some(proj_mut) = self.workspace.find_project_mut(project_id) {
                    if let Some(fid) = parent_folder_id
                        && let Some(folder_mut) = proj_mut.find_folder_mut(&fid)
                    {
                        folder_mut.requests.push(copy_node);
                    } else {
                        proj_mut.requests.push(copy_node);
                    }
                }
                self.explorer.update(cx, |panel, cx| {
                    panel.set_workspace(self.workspace.clone(), cx);
                });
                self.open_node(
                    ExplorerNode::Request {
                        project_id: project_id.to_string(),
                        request_id: copy_id,
                    },
                    window,
                    cx,
                );
            }
            Err(err) => {
                window.push_notification(format!("Failed duplicating request: {err}"), cx);
            }
        }
    }

    fn confirm_delete_request(
        &mut self,
        project_id: String,
        request_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, _, req)) = self.workspace.find_request(&request_id) else {
            return;
        };
        let req_name = req.doc.name.clone();
        let file_path = req.file_path.clone();
        let app_entity = cx.entity();

        window.open_alert_dialog(cx, move |alert, _, _| {
            let app_ok = app_entity.clone();
            let pid = project_id.clone();
            let rid = request_id.clone();
            let path = file_path.clone();
            alert
                .title(format!("Delete \"{req_name}\"?"))
                .description("This permanently removes the YAML request file from disk.")
                .confirm()
                .ok_text("Delete")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    app_ok.update(cx, |this, cx| {
                        if let Err(err) = delete_request(&path) {
                            window.push_notification(format!("Delete failed: {err}"), cx);
                            return;
                        }
                        if let Some(ix) = this
                            .tabs
                            .iter()
                            .position(|t| t.key(cx) == format!("req:{rid}"))
                        {
                            this.close_tab(ix, cx);
                        }
                        if let Some(proj) = this.workspace.find_project_mut(&pid) {
                            remove_request_from_project(proj, &rid);
                        }
                        this.explorer.update(cx, |panel, cx| {
                            panel.set_workspace(this.workspace.clone(), cx);
                        });
                    });
                    true
                })
        });
    }

    fn confirm_delete_folder(
        &mut self,
        _project_id: String,
        folder_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, chain)) = self.workspace.find_folder(&folder_id) else {
            return;
        };
        let folder = *chain.last().expect("folder chain non-empty");
        let folder_name = folder.doc.name.clone();
        let dir_path = folder.dir_path.clone();
        let app_entity = cx.entity();

        window.open_alert_dialog(cx, move |alert, _, _| {
            let app_ok = app_entity.clone();
            let path = dir_path.clone();
            let fid = folder_id.clone();
            alert
                .title(format!("Delete folder \"{folder_name}\"?"))
                .description("All nested folders and YAML request files inside it will be deleted.")
                .confirm()
                .ok_text("Delete")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    app_ok.update(cx, |this, cx| {
                        if let Err(err) = delete_folder(&path) {
                            window.push_notification(format!("Delete failed: {err}"), cx);
                            return;
                        }
                        this.tabs.retain(|t| {
                            let key = t.key(cx);
                            if let Some(rid) = key.strip_prefix("req:") {
                                this.workspace.find_request(rid).is_some()
                            } else {
                                key != format!("fld:{fid}")
                            }
                        });
                        this.reload_workspace(window, cx);
                    });
                    true
                })
        });
    }

    fn save_active_tab(&mut self, cx: &mut Context<Self>) {
        if let Some(ix) = self.active_tab_ix
            && let Some(tab) = self.tabs.get(ix)
        {
            match tab {
                WorkbenchTab::Request(view) => view.update(cx, |v, cx| v.save(cx)),
                WorkbenchTab::Container(view) => view.update(cx, |v, cx| v.save(cx)),
            }
        }
    }

    fn send_active_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.active_tab_ix
            && let Some(WorkbenchTab::Request(view)) = self.tabs.get(ix)
        {
            view.update(cx, |v, cx| v.send_request(window, cx));
        }
    }

    fn cancel_active_request(&mut self, cx: &mut Context<Self>) {
        if let Some(ix) = self.active_tab_ix
            && let Some(WorkbenchTab::Request(view)) = self.tabs.get(ix)
        {
            view.update(cx, |v, cx| v.cancel_request(cx));
        }
    }
}

impl Render for TestrixApp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_dark = cx.theme().is_dark();
        let active_project = self.workspace.active_project();
        let active_env = self.workspace.active_environment();
        let enabled_var_count = active_env
            .map(|e| e.doc.variables.iter().filter(|v| v.enabled).count())
            .unwrap_or(0);
        let active_env_name = active_env
            .map(|e| e.doc.name.as_str())
            .unwrap_or("No Environment");

        let active_auth_summary = self
            .active_tab_ix
            .and_then(|ix| self.tabs.get(ix))
            .and_then(|tab| match tab {
                WorkbenchTab::Request(view) => {
                    let req_id = view.read(cx).request_id().to_string();
                    let pid = view.read(cx).project_id().to_string();
                    self.workspace
                        .find_project(&pid)
                        .and_then(|p| resolve_request_auth(p, &req_id).ok())
                        .map(|r| format!("Auth: {}", r.summary()))
                }
                WorkbenchTab::Container(_) => None,
            })
            .unwrap_or_else(|| "YAML Spec: testrix.io/v1".to_string());

        let total_requests = self.workspace.total_request_count();
        let project_count = self.workspace.projects.len();

        v_flex()
            .id("testrix-root")
            .role(gpui_kit::Role::Application)
            .key_context(APP_KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(|_, _: &Quit, _, cx| cx.quit()))
            .on_action(cx.listener(|this, _: &SaveActive, _, cx| this.save_active_tab(cx)))
            .on_action(cx.listener(|this, _: &SendRequest, window, cx| {
                this.send_active_request(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CancelRequest, _, cx| {
                this.cancel_active_request(cx);
            }))
            .on_action(cx.listener(|this, _: &NewRequest, window, cx| {
                if let Some(proj) = this.workspace.active_project() {
                    this.open_new_request_dialog(proj.doc.id.clone(), None, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NewFolder, window, cx| {
                if let Some(proj) = this.workspace.active_project() {
                    this.open_new_folder_dialog(proj.doc.id.clone(), None, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NewProject, window, cx| {
                this.open_new_project_dialog(window, cx);
            }))
            .on_action(cx.listener(|this, _: &FocusQuickSearch, window, cx| {
                this.explorer
                    .update(cx, |panel, cx| panel.focus_search(window, cx));
            }))
            .on_action(cx.listener(|this, _: &CloseActiveTab, _, cx| {
                if let Some(ix) = this.active_tab_ix {
                    this.close_tab(ix, cx);
                }
            }))
            .on_action(cx.listener(|_, _: &ToggleTheme, _, cx| {
                let next = if cx.theme().is_dark() {
                    ThemeMode::Light
                } else {
                    ThemeMode::Dark
                };
                Theme::change(next, None, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleEnvSheet, window, cx| {
                this.open_env_sheet(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ReloadWorkspace, window, cx| {
                this.reload_workspace(window, cx);
            }))
            // Top TitleBar with workspace actions on left and Profile + Env Vars on right-top corner
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .items_center()
                        .gap_2()
                        .pr_2()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .child(
                                    h_flex()
                                        .gap_1p5()
                                        .items_center()
                                        .child(
                                            Icon::new(LucideIcon::Zap)
                                                .small()
                                                .text_color(cx.theme().primary),
                                        )
                                        .child(div().text_sm().font_bold().child("Testrix")),
                                )
                                .child(
                                    Button::new("open-workspace-btn")
                                        .ghost()
                                        .xsmall()
                                        .icon(LucideIcon::FolderOpen)
                                        .label("Workspace")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.choose_workspace_dir(window, cx);
                                        })),
                                )
                                .when_some(active_project, |row, proj| {
                                    row.child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(proj.doc.name.clone()),
                                    )
                                })
                                .child(
                                    Button::new("toggle-theme-btn")
                                        .ghost()
                                        .xsmall()
                                        .icon(if is_dark {
                                            IconName::Sun
                                        } else {
                                            IconName::Moon
                                        })
                                        .tooltip("Switch Light / Dark theme")
                                        .on_click(|_, _, cx| {
                                            let next = if cx.theme().is_dark() {
                                                ThemeMode::Light
                                            } else {
                                                ThemeMode::Dark
                                            };
                                            Theme::change(next, None, cx);
                                        }),
                                ),
                        )
                        // Right-top corner: Profile + Environment Variables
                        .child(
                            h_flex()
                                .id("top-right-profile-bar")
                                .test_support()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .gap_1p5()
                                .items_center()
                                .child(
                                    div().w(px(172.)).flex_none().child(
                                        Select::new(&self.profile_select)
                                            .id("profile-selector")
                                            .xsmall()
                                            .icon(LucideIcon::Layers)
                                            .placeholder("No Environment"),
                                    ),
                                )
                                .child(
                                    Button::new("env-vars-button")
                                        .outline()
                                        .xsmall()
                                        .icon(LucideIcon::SlidersHorizontal)
                                        .label(format!("Env Vars ({enabled_var_count})"))
                                        .tooltip("Manage profiles and environment variables")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_env_sheet(window, cx);
                                        })),
                                ),
                        ),
                ),
            )
            // Main Body: Center Workbench (left/center) + Project & Request Explorer (right)
            .child(
                div().flex_1().min_h_0().w_full().child(
                    h_resizable("main-workspace-split")
                        .child(
                            resizable_panel().child(
                                v_flex()
                                    .id("center-workbench-pane")
                                    .test_support()
                                    .size_full()
                                    .min_w_0()
                                    .min_h_0()
                                    .child(self.render_workbench_tabs(cx))
                                    .child(self.render_workbench_body(cx)),
                            ),
                        )
                        .child(
                            resizable_panel()
                                .size(px(310.))
                                .size_range(px(230.)..px(520.))
                                .flex_none()
                                .child(
                                    v_flex()
                                        .id("right-explorer-pane")
                                        .test_support()
                                        .size_full()
                                        .min_h_0()
                                        .child(self.explorer.clone()),
                                ),
                        ),
                ),
            )
            // Bottom StatusBar
            .child(
                StatusBar::new()
                    .left(format!(
                        "{} · {} project(s), {} request(s)",
                        self.workspace_dir.display(),
                        project_count,
                        total_requests
                    ))
                    .right(active_auth_summary)
                    .right(format!("Profile: {active_env_name}")),
            )
    }
}

impl TestrixApp {
    fn render_workbench_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_ix = self.active_tab_ix.unwrap_or(0);

        TabBar::new("workbench-tabs")
            .w_full()
            .folder()
            .small()
            .menu(true)
            .max_width(px(220.))
            .selected_index(selected_ix)
            .on_click(cx.listener(|this, ix: &usize, _, cx| {
                if *ix < this.tabs.len() {
                    this.active_tab_ix = Some(*ix);
                    let key = this.tabs[*ix].key(cx);
                    this.explorer
                        .update(cx, |panel, cx| panel.reveal_and_select(&key, cx));
                    cx.notify();
                }
            }))
            .children(self.tabs.iter().enumerate().map(|(ix, tab)| {
                let key = tab.key(cx);
                let title = tab.title(cx);
                let dirty = tab.is_dirty(cx);
                let label = if dirty { format!("{title} •") } else { title };

                let prefix = match tab {
                    WorkbenchTab::Request(view) => {
                        method_badge(view.read(cx).method(), true).into_any_element()
                    }
                    WorkbenchTab::Container(view) => {
                        let icon = if view.read(cx).is_project() {
                            LucideIcon::Boxes
                        } else {
                            LucideIcon::FolderKey
                        };
                        Icon::new(icon)
                            .xsmall()
                            .text_color(cx.theme().muted_foreground)
                            .into_any_element()
                    }
                };

                Tab::new()
                    .px_2()
                    .prefix(prefix)
                    .label(label)
                    .suffix(
                        Button::new(SharedString::from(format!("close-tab-{key}")))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.close_tab(ix, cx);
                            })),
                    )
                    .selected(self.active_tab_ix == Some(ix))
            }))
            .suffix(
                Button::new("workbench-new-request-tab")
                    .ghost()
                    .xsmall()
                    .mx_1()
                    .icon(IconName::Plus)
                    .tooltip("New HTTP Request")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Some(proj) = this.workspace.active_project() {
                            this.open_new_request_dialog(proj.doc.id.clone(), None, window, cx);
                        } else {
                            this.open_new_project_dialog(window, cx);
                        }
                    })),
            )
    }

    fn render_workbench_body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(ix) = self.active_tab_ix
            && let Some(tab) = self.tabs.get(ix)
        {
            let body = match tab {
                WorkbenchTab::Request(view) => view.clone().into_any_element(),
                WorkbenchTab::Container(view) => view.clone().into_any_element(),
            };
            return div()
                .id("center-workbench-content")
                .test_support()
                .flex_1()
                .min_h_0()
                .size_full()
                .child(body)
                .into_any_element();
        }

        div()
            .id("center-workbench-content")
            .test_support()
            .flex_1()
            .min_h_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                Empty::new()
                    .header(
                        EmptyHeader::new()
                            .media(
                                EmptyMedia::new()
                                    .with_variant(EmptyMediaVariant::Icon)
                                    .child(Icon::new(LucideIcon::SendHorizontal)),
                            )
                            .title(EmptyTitle::new().child("No request selected"))
                            .description(
                                EmptyDescription::new().child(
                                    "Pick a request from the explorer on the right or create a new YAML HTTP request.",
                                ),
                            ),
                    )
                    .content(
                        EmptyContent::new().child(
                            Button::new("empty-new-request")
                                .primary()
                                .small()
                                .icon(IconName::Plus)
                                .label("New Request…")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    if let Some(proj) = this.workspace.active_project() {
                                        this.open_new_request_dialog(
                                            proj.doc.id.clone(),
                                            None,
                                            window,
                                            cx,
                                        );
                                    } else {
                                        this.open_new_project_dialog(window, cx);
                                    }
                                })),
                        ),
                    ),
            )
            .into_any_element()
    }
}

fn build_profile_options(workspace: &WorkspaceTree) -> (Vec<ProfileOption>, Option<IndexPath>) {
    let mut items = vec![ProfileOption {
        env_id: None,
        label: "No Environment".into(),
        var_count: 0,
    }];

    if let Some(project) = workspace.active_project() {
        for env in &project.environments {
            let enabled = env.doc.variables.iter().filter(|v| v.enabled).count();
            items.push(ProfileOption {
                env_id: Some(env.doc.id.clone()),
                label: SharedString::from(format!("{} ({enabled})", env.doc.name)),
                var_count: enabled,
            });
        }
    }

    let selected_row = workspace
        .active_environment_id
        .as_ref()
        .and_then(|active_id| {
            items
                .iter()
                .position(|opt| opt.env_id.as_ref() == Some(active_id))
        })
        .unwrap_or(0);

    (items, Some(IndexPath::default().row(selected_row)))
}

fn first_request_in_folder(
    folder: &testrix_core::FolderNode,
) -> Option<&testrix_core::RequestNode> {
    if let Some(req) = folder.requests.first() {
        return Some(req);
    }
    for sub in &folder.folders {
        if let Some(req) = first_request_in_folder(sub) {
            return Some(req);
        }
    }
    None
}

fn remove_request_from_project(project: &mut testrix_core::ProjectNode, request_id: &str) {
    project.requests.retain(|r| r.doc.id != request_id);
    for folder in &mut project.folders {
        remove_request_from_folder(folder, request_id);
    }
}

fn remove_request_from_folder(folder: &mut testrix_core::FolderNode, request_id: &str) {
    folder.requests.retain(|r| r.doc.id != request_id);
    for sub in &mut folder.folders {
        remove_request_from_folder(sub, request_id);
    }
}

/// Initializes Testrix keybindings and GPUI Kit components.
pub fn init_app(cx: &mut App) {
    gpui_kit::init(cx);
    crate::actions::init_keybindings(cx);
}

/// Opens the main Testrix desktop window with native titlebar integration.
pub fn open_main_window(
    workspace_dir: PathBuf,
    cx: &mut App,
) -> gpui_kit::Result<(gpui_kit::AnyWindowHandle, Entity<TestrixApp>)> {
    let window_bounds = gpui_kit::WindowBounds::centered(size(px(1360.), px(860.)), cx);
    let options = WindowOptions {
        window_bounds: Some(window_bounds),
        window_min_size: Some(size(px(960.), px(620.))),
        kind: gpui_kit::WindowKind::Normal,
        #[cfg(target_os = "linux")]
        window_decorations: Some(gpui_kit::WindowDecorations::Client),
        ..TitleBar::window_options()
    };

    let (handle, entity) = gpui_kit::open_window(options, cx, move |window, cx| {
        tracing::info!(actual_bounds = ?window.bounds(), viewport = ?window.viewport_size(), "created Testrix window");
        cx.new(|cx| TestrixApp::new(workspace_dir, window, cx))
    })?;
    let _ = handle.update(cx, |_, window, _| {
        window.set_window_title("Testrix — YAML API Workbench");
        window.activate_window();
    });
    tracing::info!(?window_bounds, "opened main Testrix window");
    Ok((handle, entity))
}

fn rewrite_string_in_place(target: &mut String, renames: &[(String, String)]) -> bool {
    if let Some(updated) = rename_template_variables(target, renames) {
        *target = updated;
        true
    } else {
        false
    }
}

fn rewrite_kv_rows(rows: &mut [KeyValueRow], renames: &[(String, String)]) -> bool {
    let mut changed = false;
    for row in rows {
        changed |= rewrite_string_in_place(&mut row.name, renames);
        changed |= rewrite_string_in_place(&mut row.value, renames);
    }
    changed
}

fn rewrite_auth_config(auth: &mut AuthConfig, renames: &[(String, String)]) -> bool {
    match auth {
        AuthConfig::Inherit | AuthConfig::None => false,
        AuthConfig::Bearer { token, prefix } => {
            let mut changed = rewrite_string_in_place(token, renames);
            changed |= rewrite_string_in_place(prefix, renames);
            changed
        }
        AuthConfig::Basic { username, password } => {
            let mut changed = rewrite_string_in_place(username, renames);
            changed |= rewrite_string_in_place(password, renames);
            changed
        }
        AuthConfig::ApiKey { key, value, .. } => {
            let mut changed = rewrite_string_in_place(key, renames);
            changed |= rewrite_string_in_place(value, renames);
            changed
        }
    }
}

fn rewrite_request_doc(doc: &mut HttpRequestDoc, renames: &[(String, String)]) -> bool {
    let mut changed = rewrite_string_in_place(&mut doc.request.url, renames);
    changed |= rewrite_kv_rows(&mut doc.request.params, renames);
    changed |= rewrite_kv_rows(&mut doc.request.headers, renames);
    changed |= rewrite_auth_config(&mut doc.request.auth, renames);
    match &mut doc.request.body {
        RequestBody::None | RequestBody::Binary { .. } => {}
        RequestBody::Json { content } | RequestBody::Raw { content, .. } => {
            changed |= rewrite_string_in_place(content, renames);
        }
        RequestBody::FormUrlEncoded { fields } => {
            changed |= rewrite_kv_rows(fields, renames);
        }
        RequestBody::Multipart { fields } => {
            for field in fields {
                changed |= rewrite_string_in_place(&mut field.name, renames);
                changed |= rewrite_string_in_place(&mut field.value, renames);
            }
        }
    }
    changed
}

fn propagate_variable_renames_in_folder(folder: &mut FolderNode, renames: &[(String, String)]) {
    if rewrite_auth_config(&mut folder.doc.auth, renames) {
        let _ = save_folder(folder);
    }
    for req in &mut folder.requests {
        if rewrite_request_doc(&mut req.doc, renames) {
            let _ = save_request(req);
        }
    }
    for sub in &mut folder.folders {
        propagate_variable_renames_in_folder(sub, renames);
    }
}

fn propagate_variable_renames_in_project(proj: &mut ProjectNode, renames: &[(String, String)]) {
    let mut proj_changed = rewrite_string_in_place(&mut proj.doc.base_url, renames);
    proj_changed |= rewrite_kv_rows(&mut proj.doc.default_headers, renames);
    proj_changed |= rewrite_auth_config(&mut proj.doc.auth, renames);
    if proj_changed {
        let _ = save_project(proj);
    }
    for req in &mut proj.requests {
        if rewrite_request_doc(&mut req.doc, renames) {
            let _ = save_request(req);
        }
    }
    for folder in &mut proj.folders {
        propagate_variable_renames_in_folder(folder, renames);
    }
}
