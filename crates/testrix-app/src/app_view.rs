use std::{
    cell::Cell,
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, OnceLock},
};

use gpui_kit::{
    component::scroll::ScrollableElement as _,
    App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable, Image, ImageFormat,
    ImageSource, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _,
    PathPromptOptions, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, WindowOptions, img,
    assets::IconName as LucideIcon,
    base::TestSupportExt as _,
    component::{
        ActiveTheme as _, Disableable as _, Icon, IconName, IndexPath, Placement, Selectable as _,
        Sizable as _,
        StyledExt as _, Theme, ThemeMode, TitleBar, WindowExt as _,
        button::{Button, ButtonVariant, ButtonVariants as _},
        checkbox::Checkbox,
        dialog::{DialogClose, DialogFooter},
        empty::{
            Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant,
            EmptyTitle,
        },
        form::{Field, Form},
        h_flex,
        input::{Input, InputEvent, InputState, Replace, Search, Textarea, TextareaState},
        menu::{ContextMenuExt as _, PopupMenuItem},
        resizable::{h_resizable, resizable_panel, v_resizable},
        select::{Select, SelectEvent, SelectItem, SelectState},
        spinner::Spinner,
        status_bar::StatusBar,
        switch::Switch,
        tab::{Tab, TabBar},
        tag::{Tag, TagVariant},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px, size,
};
use testrix_core::{
    AuthConfig, AuthNodeRef, ExplorerNode, FolderNode, GLOBAL_ENV_ID, HttpMethod, HttpRequestDoc,
    HttpRequestSpec, ImportedProjectBundle, KeyValueRow, ProjectNode, RequestBody,
    RequestHistoryEntry, RequestSet, RequestSetFolder, RequestSetItem, WorkspaceTree,
    export_openapi_3_0, generate_id, parse_apifox, parse_openapi_3_0, parse_postman_collection,
    parse_postman_zip, parse_request_script, rename_template_variables, replace_template_variables,
    resolve_request_auth,
};
use testrix_http::{
    DownloadProgressState, HttpRunner, OneshotTryRecvError, ReleaseInfo, ShareChannel, ShareFile,
    check_for_upgrade, collect_project_yaml_files, dispatch_share_future, download_release_asset,
    delete_remote_sync_repo, fetch_remote_sync_state, launch_installer, login_sync_server,
    login_testrix_server, push_merged_sync_state, revoke_cloudflare_r2_share,
    revoke_testrix_server_share, upload_to_cloudflare_r2, upload_to_testrix_server,
};
use testrix_storage::{
    GitCommitInfo, PersistedShareRecord, ShareSettings, SyncSettings,
    apply_synced_files_and_commit, clear_saved_responses, collect_workspace_tracked_files,
    commit_workspace_change, create_environment, create_folder, create_project, create_request,
    delete_environment, delete_folder, delete_project, delete_request, duplicate_request,
    ensure_seeded_workspace, ensure_workspace_git_repo, import_project_bundle, init_or_load_config,
    list_file_commits, load_request_response, load_sync_base, load_workspace,
    load_workspace_environments, load_workspace_explorer_state, load_workspace_history,
    load_workspace_sets, load_workspace_tabs, move_request, read_file_at_commit,
    restore_file_from_commit, save_config_appearance, save_config_key_bindings,
    save_config_preferences, save_config_share_settings, save_config_shared_projects,
    save_config_sync_settings, save_config_workspace_dir, save_environment, save_folder,
    save_global_environment, save_project, save_request, save_request_response, save_sync_base,
    save_workspace_explorer_state, save_workspace_history, save_workspace_sets,
    save_workspace_state, save_workspace_tabs, three_way_merge_trees,
};
use tracing::{error, info};

use crate::{
    actions::{
        APP_KEY_CONTEXT, CUSTOMIZABLE_SHORTCUTS, CancelRequest, CloseActiveTab, FocusQuickSearch,
        NewFolder, NewProject, NewRequest, Quit, ReloadWorkspace, SaveActive, SendRequest,
        ToggleEnvSheet, ToggleTheme, apply_keybindings, effective_shortcut, validate_shortcut,
    },
    container_editor::{ActiveShareRecord, ContainerTabEvent, ContainerTabView},
    env_editor::{EnvEditorEvent, EnvEditorView},
    explorer::ExplorerEvent,
    history_panel::HistoryPanelEvent,
    request_editor::{RequestTabEvent, RequestTabView},
    sets_panel::SetsPanelEvent,
    sidebar_pane::{SidebarPane, SidebarPaneEvent, SidebarViewMode},
    theme_helpers::method_badge,
    tray::hide_window_to_tray,
};

fn app_titlebar_icon() -> Arc<Image> {
    static ICON: OnceLock<Arc<Image>> = OnceLock::new();
    ICON.get_or_init(|| {
        Arc::new(Image::from_bytes(
            ImageFormat::Png,
            include_bytes!("../assets/icon.png").to_vec(),
        ))
    })
    .clone()
}

#[derive(Debug, Clone, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RawBuildInfo {
    #[serde(default)]
    version: String,
    #[serde(default)]
    build_time: String,
    #[serde(default)]
    branch: String,
    #[serde(default)]
    commit: String,
}

#[derive(Debug, Clone)]
struct BuildInfo {
    version: String,
    build_time: String,
    branch: String,
    commit: String,
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

fn current_build_info() -> BuildInfo {
    let raw: RawBuildInfo =
        serde_json::from_str(include_str!("../assets/build_info.json")).unwrap_or_default();
    let pick = |val: String, fallback: Option<&str>, default: &str| -> String {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            trimmed.to_string()
        } else if let Some(fb) = fallback.map(str::trim).filter(|s| !s.is_empty()) {
            fb.to_string()
        } else {
            default.to_string()
        }
    };
    BuildInfo {
        version: pick(
            raw.version,
            Some(concat!("v", env!("CARGO_PKG_VERSION"))),
            concat!("v", env!("CARGO_PKG_VERSION")),
        ),
        build_time: pick(
            raw.build_time,
            option_env!("TESTRIX_LOCAL_BUILD_TIME"),
            "dev",
        ),
        branch: pick(raw.branch, option_env!("TESTRIX_LOCAL_BRANCH"), "unknown"),
        commit: pick(raw.commit, option_env!("TESTRIX_LOCAL_COMMIT"), "unknown"),
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct CloseBehaviorOption(Option<&'static str>);

impl SelectItem for CloseBehaviorOption {
    type Value = Option<&'static str>;

    fn title(&self) -> SharedString {
        match self.0 {
            Some("tray") => crate::i18n::t("close_behavior_tray").into(),
            Some("quit") => crate::i18n::t("close_behavior_quit").into(),
            _ => crate::i18n::t("close_behavior_ask").into(),
        }
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LanguageOption(crate::i18n::AppLanguage);

impl SelectItem for LanguageOption {
    type Value = crate::i18n::AppLanguage;

    fn title(&self) -> SharedString {
        self.0.native_name().into()
    }

    fn value(&self) -> &Self::Value {
        &self.0
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

#[derive(Debug, Clone, Default)]
enum UpgradeStatus {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(ReleaseInfo),
    Downloading {
        release: ReleaseInfo,
        downloaded: u64,
        total: Option<u64>,
    },
    ReadyToInstall {
        release: ReleaseInfo,
        installer_path: PathBuf,
    },
    Error {
        release: Option<ReleaseInfo>,
        message: String,
    },
}

impl UpgradeStatus {
    fn active_release(&self) -> Option<&ReleaseInfo> {
        match self {
            Self::Available(release)
            | Self::Downloading { release, .. }
            | Self::ReadyToInstall { release, .. } => Some(release),
            Self::Error { release, .. } => release.as_ref(),
            Self::Idle | Self::Checking | Self::UpToDate => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreferencesSection {
    General,
    Appearance,
    Keybindings,
    ShareSync,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UiFontSize {
    Small,
    Large,
}

impl UiFontSize {
    fn from_code(code: &str) -> Self {
        if code.eq_ignore_ascii_case("large") {
            Self::Large
        } else {
            Self::Small
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Large => "large",
        }
    }

    fn pixels(self) -> gpui_kit::Pixels {
        match self {
            Self::Small => px(16.),
            Self::Large => px(18.),
        }
    }
}

/// Top-level Testrix desktop workbench window view.
pub struct TestrixApp {
    focus_handle: FocusHandle,
    workspace_dir: PathBuf,
    workspace: WorkspaceTree,
    runner: HttpRunner,
    sidebar_panes: Vec<Entity<SidebarPane>>,
    sidebar_pane_subscriptions: Vec<Subscription>,
    history_entries: Vec<RequestHistoryEntry>,
    request_sets: Vec<RequestSet>,
    env_editor: Entity<EnvEditorView>,
    profile_select: Entity<SelectState<Vec<ProfileOption>>>,
    language_select: Entity<SelectState<Vec<LanguageOption>>>,
    close_behavior_select: Entity<SelectState<Vec<CloseBehaviorOption>>>,
    tabs: Vec<WorkbenchTab>,
    active_tab_ix: Option<usize>,
    context_tab_ix: Option<usize>,
    remember_open_tabs: bool,
    remember_responses: bool,
    close_behavior: Option<String>,
    ui_font_size: UiFontSize,
    editor_font_size: u16,
    custom_key_bindings: std::collections::BTreeMap<String, String>,
    pref_section: PreferencesSection,
    pref_keymap_error: Option<String>,
    share_settings: ShareSettings,
    active_shares: Vec<ActiveShareRecord>,
    share_sync_count: usize,
    sync_settings: SyncSettings,
    sync_in_progress: bool,
    quit_after_sync: bool,
    force_quit: bool,
    exit_prompt_open: bool,
    upgrade_status: UpgradeStatus,
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
            Ok(tree) => {
                if let Err(err) = ensure_workspace_git_repo(&tree.root_dir) {
                    tracing::warn!(error = %err, "failed initializing workspace git repo");
                }
                tree
            }
            Err(err) => {
                error!(error = %err, "failed to load workspace");
                WorkspaceTree {
                    root_dir: workspace_dir.clone(),
                    ..Default::default()
                }
            }
        };

        let runner = HttpRunner::new().expect("failed to initialize HTTP runner");
        let initial_pane_id = generate_id("pane");
        let initial_pane = cx.new(|cx| {
            SidebarPane::new(initial_pane_id, SidebarViewMode::Explorer, false, window, cx)
        });
        let history_entries = load_workspace_history(&workspace.root_dir);
        let request_sets = load_workspace_sets(&workspace.root_dir);
        let env_editor = cx.new(|cx| EnvEditorView::new(window, cx));

        let (profile_items, selected_ix) = build_profile_options(&workspace);
        let profile_select = cx.new(|cx| SelectState::new(profile_items, selected_ix, window, cx));

        let cfg = init_or_load_config().unwrap_or_default();
        let initial_lang = crate::i18n::AppLanguage::from_code(&cfg.language);
        crate::i18n::set_language(initial_lang);
        let language_items: Vec<LanguageOption> = crate::i18n::AppLanguage::ALL
            .iter()
            .copied()
            .map(LanguageOption)
            .collect();
        let language_ix = crate::i18n::AppLanguage::ALL
            .iter()
            .position(|&l| l == initial_lang)
            .unwrap_or(0);
        let language_select = cx.new(|cx| {
            SelectState::new(
                language_items,
                Some(IndexPath::default().row(language_ix)),
                window,
                cx,
            )
        });
        let close_behavior_items = vec![
            CloseBehaviorOption(None),
            CloseBehaviorOption(Some("tray")),
            CloseBehaviorOption(Some("quit")),
        ];
        let close_behavior_ix = match cfg.close_behavior.as_deref() {
            Some("tray") => 1,
            Some("quit") => 2,
            _ => 0,
        };
        let close_behavior_select = cx.new(|cx| {
            SelectState::new(
                close_behavior_items,
                Some(IndexPath::default().row(close_behavior_ix)),
                window,
                cx,
            )
        });

        let restored_shares: Vec<ActiveShareRecord> = cfg
            .shared_projects
            .iter()
            .filter_map(|p| {
                let channel = ShareChannel::from_label(&p.channel)?;
                if channel == ShareChannel::LocalNetwork {
                    return None;
                }
                Some(ActiveShareRecord {
                    id: p.id.clone(),
                    project_id: p.project_id.clone(),
                    project_name: p.project_name.clone(),
                    project_slug: p.project_slug.clone(),
                    channel,
                    share_url: p.share_url.clone(),
                    target_info: p.target_info.clone(),
                    file_paths: Vec::new(),
                    local_handle: None,
                })
            })
            .collect();
        info!(
            restored_share_count = restored_shares.len(),
            configured_shared_projects = cfg.shared_projects.len(),
            "loaded Testrix config on startup"
        );

        let initial_ui_font_size = UiFontSize::from_code(&cfg.ui_font_size);
        let initial_editor_font_size = cfg.editor_font_size.clamp(10, 24);
        let initial_key_bindings = cfg.key_bindings.clone();
        if let Some(mode) = cfg.theme_mode.as_deref() {
            let target_mode = if mode.eq_ignore_ascii_case("dark") {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            };
            if cx.theme().mode != target_mode {
                Theme::change(target_mode, Some(window), cx);
                crate::theme_helpers::configure_syntax_highlighting(cx);
            }
        }
        Theme::global_mut(cx).font_size = initial_ui_font_size.pixels();
        apply_keybindings(&initial_key_bindings, cx);

        let mut app = Self {
            focus_handle: cx.focus_handle(),
            workspace_dir,
            workspace,
            runner,
            sidebar_panes: vec![initial_pane],
            sidebar_pane_subscriptions: Vec::new(),
            history_entries,
            request_sets,
            env_editor,
            profile_select,
            language_select,
            close_behavior_select,
            tabs: Vec::new(),
            active_tab_ix: None,
            context_tab_ix: None,
            remember_open_tabs: cfg.remember_open_tabs,
            remember_responses: cfg.remember_responses,
            close_behavior: cfg.close_behavior.clone(),
            ui_font_size: initial_ui_font_size,
            editor_font_size: initial_editor_font_size,
            custom_key_bindings: initial_key_bindings,
            pref_section: PreferencesSection::General,
            pref_keymap_error: None,
            share_settings: ShareSettings::from(&cfg),
            active_shares: restored_shares,
            share_sync_count: 0,
            sync_settings: SyncSettings::from(&cfg),
            sync_in_progress: false,
            quit_after_sync: false,
            force_quit: false,
            exit_prompt_open: false,
            upgrade_status: UpgradeStatus::Idle,
            _subscriptions: Vec::new(),
        };

        let app_weak = cx.weak_entity();
        window.on_window_should_close(cx, move |window, cx| {
            let Some(app_entity) = app_weak.upgrade() else {
                return true;
            };
            let (force_quit, syncing, prompt_open, close_behavior) = {
                let app = app_entity.read(cx);
                (
                    app.force_quit,
                    app.share_sync_count > 0,
                    app.exit_prompt_open,
                    app.close_behavior.clone(),
                )
            };
            if force_quit {
                if syncing {
                    if !prompt_open {
                        app_entity.update(cx, |this, cx| {
                            this.open_exit_sync_dialog(window, cx);
                        });
                    }
                    return false;
                }
                return true;
            }

            match close_behavior.as_deref() {
                Some("tray") => {
                    hide_window_to_tray(window, cx);
                    false
                }
                Some("quit") => {
                    if syncing {
                        if !prompt_open {
                            app_entity.update(cx, |this, cx| {
                                this.open_exit_sync_dialog(window, cx);
                            });
                        }
                        return false;
                    }
                    cx.quit();
                    true
                }
                _ => {
                    if !prompt_open {
                        app_entity.update(cx, |this, cx| {
                            this.open_close_behavior_dialog(window, cx);
                        });
                    }
                    false
                }
            }
        });

        app.wire_subscriptions(window, cx);
        let saved_expanded = load_workspace_explorer_state(&app.workspace.root_dir);
        app.sync_sidebars(Some(&saved_expanded), cx);
        app.sync_env_editor(window, cx);
        app.restore_or_open_initial_tabs(window, cx);
        app.maybe_auto_git_sync(window, cx);
        app.start_ipc_reload_listener(window, cx);
        if !cfg!(debug_assertions) {
            app.check_for_updates(false, window, cx);
        }
        app
    }

    pub fn workspace_dir(&self) -> &PathBuf {
        &self.workspace_dir
    }

    pub fn set_force_quit(&mut self) {
        self.force_quit = true;
    }

    fn start_ipc_reload_listener(&self, window: &mut Window, cx: &mut Context<Self>) {
        let mut rx = crate::ipc::start_ipc_server();
        let app_entity = cx.entity();
        window
            .spawn(cx, async move |window| {
                while let Some((req, reply_tx)) = rx.recv().await {
                    if req.action != "reload" {
                        let _ = reply_tx.send(crate::ipc::IpcResponse {
                            ok: false,
                            reloaded: false,
                            message: Some(format!("Unsupported IPC action: {}", req.action)),
                        });
                        continue;
                    }

                    let target_ws = req.workspace.clone();
                    let outcome = window.update(|window, cx| {
                        app_entity.update(cx, |this, cx| {
                            let matches_workspace = match target_ws.as_deref() {
                                Some(ws) if !ws.trim().is_empty() => {
                                    crate::ipc::normalize_workspace_path(std::path::Path::new(ws))
                                        == crate::ipc::normalize_workspace_path(
                                            &this.workspace_dir,
                                        )
                                }
                                _ => true,
                            };
                            if matches_workspace {
                                this.reload_workspace(window, cx);
                                window.push_notification(
                                    "Reloaded workspace from external YAML change.",
                                    cx,
                                );
                                true
                            } else {
                                false
                            }
                        })
                    });

                    let resp = match outcome {
                        Ok(true) => crate::ipc::IpcResponse {
                            ok: true,
                            reloaded: true,
                            message: Some("Reloaded workspace".to_string()),
                        },
                        Ok(false) => crate::ipc::IpcResponse {
                            ok: true,
                            reloaded: false,
                            message: Some(
                                "Running GUI has a different workspace open".to_string(),
                            ),
                        },
                        Err(_) => crate::ipc::IpcResponse {
                            ok: false,
                            reloaded: false,
                            message: Some("Failed updating GUI window".to_string()),
                        },
                    };
                    let _ = reply_tx.send(resp);
                }
            })
            .detach();
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

    pub fn set_share_settings_for_test(
        &mut self,
        settings: ShareSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.share_settings = settings;
        self.sync_all_tabs(window, cx);
    }

    pub fn active_share_count(&self) -> usize {
        self.active_shares.len()
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
            &self.language_select,
            window,
            |this, _, event: &SelectEvent<Vec<LanguageOption>>, window, cx| {
                if let SelectEvent::Confirm(Some(lang)) = event {
                    crate::i18n::set_language(*lang);
                    let _ = save_config_preferences(
                        this.remember_open_tabs,
                        this.remember_responses,
                        lang.code(),
                        this.close_behavior.as_deref(),
                    );
                    cx.notify();
                    window.refresh();
                }
            },
        ));

        subs.push(cx.subscribe_in(
            &self.close_behavior_select,
            window,
            |this, _, event: &SelectEvent<Vec<CloseBehaviorOption>>, window, cx| {
                if let SelectEvent::Confirm(Some(choice)) = event {
                    this.close_behavior = choice.map(str::to_string);
                    let _ = save_config_preferences(
                        this.remember_open_tabs,
                        this.remember_responses,
                        crate::i18n::current_language().code(),
                        this.close_behavior.as_deref(),
                    );
                    cx.notify();
                    window.refresh();
                }
            },
        ));

        self.wire_sidebar_pane_subscriptions(window, cx);

        subs.push(cx.subscribe_in(
            &self.env_editor,
            window,
            |this, _, event: &EnvEditorEvent, window, cx| {
                this.handle_env_editor_event(event.clone(), window, cx);
            },
        ));

        self._subscriptions = subs;
    }

    fn wire_sidebar_pane_subscriptions(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let can_close = self.sidebar_panes.len() > 1;
        let mut pane_subs = Vec::new();
        for pane in &self.sidebar_panes {
            pane.update(cx, |p, cx| p.set_can_close(can_close, cx));
            pane_subs.push(cx.subscribe_in(
                pane,
                window,
                |this, _, event: &SidebarPaneEvent, window, cx| {
                    this.handle_sidebar_pane_event(event.clone(), window, cx);
                },
            ));
        }
        self.sidebar_pane_subscriptions = pane_subs;
    }

    fn sync_sidebars(&mut self, expanded_nodes: Option<&[String]>, cx: &mut Context<Self>) {
        for pane in &self.sidebar_panes {
            pane.update(cx, |p, cx| {
                p.sync_data(
                    &self.workspace,
                    &self.history_entries,
                    &self.request_sets,
                    expanded_nodes,
                    cx,
                );
            });
        }
    }

    fn reveal_and_select_in_explorers(&mut self, key: &str, cx: &mut Context<Self>) {
        for pane in &self.sidebar_panes {
            pane.update(cx, |p, cx| {
                p.explorer.update(cx, |panel, cx| {
                    panel.reveal_and_select(key, cx);
                });
            });
        }
    }

    fn set_dirty_keys_in_explorers(&mut self, dirty: HashSet<String>, cx: &mut Context<Self>) {
        for pane in &self.sidebar_panes {
            pane.update(cx, |p, cx| {
                p.explorer.update(cx, |panel, cx| {
                    panel.set_dirty_keys(dirty.clone(), cx);
                });
            });
        }
    }

    fn restore_or_open_initial_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.remember_open_tabs {
            let tab_state = load_workspace_tabs(&self.workspace.root_dir);
            let mut opened_any = false;
            for key in &tab_state.open_tabs {
                if let Some(node) = self.resolve_node_from_key(key) {
                    self.open_node(node, window, cx);
                    opened_any = true;
                }
            }
            if opened_any {
                if let Some(active_key) = tab_state.active_tab_key
                    && let Some(ix) = self.tabs.iter().position(|t| t.key(cx) == active_key)
                {
                    self.active_tab_ix = Some(ix);
                    self.reveal_and_select_in_explorers(&active_key, cx);
                }
                self.persist_open_tabs(cx);
                return;
            }
        }
        self.open_initial_request(window, cx);
    }

    fn resolve_node_from_key(&self, key: &str) -> Option<ExplorerNode> {
        if let Some(pid) = key.strip_prefix("proj:") {
            return self
                .workspace
                .find_project(pid)
                .map(|p| ExplorerNode::Project {
                    project_id: p.doc.id.clone(),
                });
        }
        if let Some(fid) = key.strip_prefix("fld:") {
            return self
                .workspace
                .find_folder(fid)
                .map(|(p, _)| ExplorerNode::Folder {
                    project_id: p.doc.id.clone(),
                    folder_id: fid.to_string(),
                });
        }
        if let Some(rid) = key.strip_prefix("req:") {
            for proj in &self.workspace.projects {
                if proj.find_request(rid).is_some() {
                    return Some(ExplorerNode::Request {
                        project_id: proj.doc.id.clone(),
                        request_id: rid.to_string(),
                    });
                }
            }
        }
        None
    }

    fn persist_open_tabs(&self, cx: &App) {
        if !self.remember_open_tabs {
            return;
        }
        let keys: Vec<String> = self.tabs.iter().map(|t| t.key(cx)).collect();
        let active_key = self
            .active_tab_ix
            .and_then(|ix| self.tabs.get(ix))
            .map(|t| t.key(cx));
        let _ = save_workspace_tabs(&self.workspace.root_dir, &keys, active_key.as_deref());
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
            ExplorerEvent::DeleteProject { project_id } => {
                self.confirm_delete_project(project_id, window, cx)
            }
            ExplorerEvent::ImportOpenApi => self.prompt_import_openapi(window, cx),
            ExplorerEvent::ImportPostmanZip => self.prompt_import_postman_zip(window, cx),
            ExplorerEvent::ImportApifox => self.prompt_import_apifox(window, cx),
            ExplorerEvent::ExportOpenApi { project_id } => {
                self.prompt_export_openapi(project_id, window, cx)
            }
            ExplorerEvent::MoveRequest {
                source_project_id,
                request_id,
                target_project_id,
                target_folder_id,
            } => self.move_request_node(
                &source_project_id,
                &request_id,
                &target_project_id,
                target_folder_id.as_deref(),
                window,
                cx,
            ),
            ExplorerEvent::CreateRequestFromDoc {
                target_project_id,
                target_folder_id,
                doc,
            } => self.create_request_from_doc(
                &target_project_id,
                target_folder_id.as_deref(),
                doc,
                window,
                cx,
            ),
            ExplorerEvent::ExpandedStateChanged(expanded) => {
                let _ = save_workspace_explorer_state(&self.workspace.root_dir, &expanded);
            }
            ExplorerEvent::ReloadWorkspace => self.reload_workspace(window, cx),
        }
    }

    fn handle_sidebar_pane_event(
        &mut self,
        event: SidebarPaneEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SidebarPaneEvent::Explorer(e) => self.handle_explorer_event(e, window, cx),
            SidebarPaneEvent::History(e) => self.handle_history_event(e, window, cx),
            SidebarPaneEvent::Sets(e) => self.handle_sets_event(e, window, cx),
            SidebarPaneEvent::SplitRequested(pane_id) => {
                let Some(pos) = self.sidebar_panes.iter().position(|p| p.read(cx).id == pane_id)
                else {
                    return;
                };
                let next_mode = match self.sidebar_panes[pos].read(cx).mode {
                    SidebarViewMode::Explorer => SidebarViewMode::Set,
                    SidebarViewMode::Set => SidebarViewMode::History,
                    SidebarViewMode::History => SidebarViewMode::Explorer,
                };
                let new_id = generate_id("pane");
                let new_pane = cx.new(|cx| {
                    SidebarPane::new(new_id, next_mode, true, window, cx)
                });
                self.sidebar_panes.insert(pos + 1, new_pane);
                self.wire_sidebar_pane_subscriptions(window, cx);
                let saved_expanded = load_workspace_explorer_state(&self.workspace.root_dir);
                self.sync_sidebars(Some(&saved_expanded), cx);
                cx.notify();
            }
            SidebarPaneEvent::CloseRequested(pane_id) => {
                if self.sidebar_panes.len() <= 1 {
                    return;
                }
                self.sidebar_panes.retain(|p| p.read(cx).id != pane_id);
                self.wire_sidebar_pane_subscriptions(window, cx);
                cx.notify();
            }
        }
    }

    fn handle_history_event(
        &mut self,
        event: HistoryPanelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            HistoryPanelEvent::ClearHistory => {
                self.history_entries.clear();
                let _ = save_workspace_history(&self.workspace.root_dir, &self.history_entries);
                self.sync_sidebars(None, cx);
                window.push_notification("History cleared.", cx);
            }
            HistoryPanelEvent::OpenEntry(entry) => {
                let existing = entry
                    .project_id
                    .as_ref()
                    .zip(entry.request_id.as_ref())
                    .and_then(|(pid, rid)| {
                        self.workspace
                            .find_project(pid)
                            .and_then(|p| p.find_request(rid))
                            .map(|_| (pid.clone(), rid.clone()))
                    });

                if let Some((pid, rid)) = existing {
                    self.open_node(
                        ExplorerNode::Request {
                            project_id: pid,
                            request_id: rid.clone(),
                        },
                        window,
                        cx,
                    );
                    let target_key = format!("req:{rid}");
                    if let Some(WorkbenchTab::Request(tab)) =
                        self.tabs.iter().find(|t| t.key(cx) == target_key)
                    {
                        tab.update(cx, |view, cx| {
                            view.recover_from_doc(&entry.doc, window, cx);
                        });
                    }
                    window.push_notification("Recovered request from history (unsaved).", cx);
                } else {
                    let mut doc = entry.doc.clone();
                    doc.id = generate_id("req");
                    let stub_proj = ProjectNode {
                        doc: testrix_core::ProjectDoc::new("", "History"),
                        root_dir: self.workspace.root_dir.clone(),
                        manifest_path: PathBuf::new(),
                        environments: Vec::new(),
                        folders: Vec::new(),
                        requests: Vec::new(),
                    };
                    let req_node = testrix_core::RequestNode {
                        doc: doc.clone(),
                        file_path: PathBuf::new(),
                    };
                    let active_env = self.workspace.effective_environment();
                    let global_env = self
                        .workspace
                        .global_environment
                        .as_ref()
                        .map(|e| e.doc.clone());
                    let runner = self.runner.clone();
                    let editor_font_size = f32::from(self.editor_font_size);
                    let tab_view = cx.new(|cx| {
                        let mut v = RequestTabView::new(
                            &stub_proj,
                            &[],
                            &req_node,
                            active_env,
                            global_env,
                            runner,
                            window,
                            cx,
                        );
                        v.set_ephemeral(true);
                        v.set_editor_font_size(editor_font_size, cx);
                        v
                    });
                    let sub = cx.subscribe_in(
                        &tab_view,
                        window,
                        |this, _, event: &RequestTabEvent, window, cx| {
                            this.handle_request_tab_event(event.clone(), window, cx);
                        },
                    );
                    self._subscriptions.push(sub);
                    self.tabs.push(WorkbenchTab::Request(tab_view));
                    self.active_tab_ix = Some(self.tabs.len() - 1);
                    self.refresh_dirty_indicators(cx);
                    cx.notify();
                }
            }
        }
    }

    fn handle_sets_event(
        &mut self,
        event: SetsPanelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SetsPanelEvent::CreateSet => {
                self.open_set_name_dialog("New Package", "", None, window, cx, |this, name| {
                    this.request_sets.push(RequestSet {
                        id: generate_id("set"),
                        name,
                        folders: Vec::new(),
                        requests: Vec::new(),
                    });
                });
            }
            SetsPanelEvent::RenameSet { set_id } => {
                let current = self
                    .request_sets
                    .iter()
                    .find(|s| s.id == set_id)
                    .map(|s| s.name.clone())
                    .unwrap_or_default();
                self.open_set_name_dialog(
                    "Rename Package",
                    &current,
                    None,
                    window,
                    cx,
                    move |this, name| {
                        if let Some(s) = this.request_sets.iter_mut().find(|s| s.id == set_id) {
                            s.name = name;
                        }
                    },
                );
            }
            SetsPanelEvent::DeleteSet { set_id } => {
                self.request_sets.retain(|s| s.id != set_id);
                let _ = save_workspace_sets(&self.workspace.root_dir, &self.request_sets);
                self.sync_sidebars(None, cx);
            }
            SetsPanelEvent::CreateFolder {
                set_id,
                parent_folder_id,
            } => {
                self.open_set_name_dialog(
                    "New Package Folder",
                    "",
                    None,
                    window,
                    cx,
                    move |this, name| {
                        let new_folder = RequestSetFolder {
                            id: generate_id("setf"),
                            name,
                            folders: Vec::new(),
                            requests: Vec::new(),
                        };
                        if let Some(set) = this.request_sets.iter_mut().find(|s| s.id == set_id) {
                            if let Some(pid) = &parent_folder_id {
                                let _ = insert_set_folder(&mut set.folders, pid, new_folder);
                            } else {
                                set.folders.push(new_folder);
                            }
                        }
                    },
                );
            }
            SetsPanelEvent::RenameFolder { set_id, folder_id } => {
                let current = self
                    .request_sets
                    .iter()
                    .find(|s| s.id == set_id)
                    .and_then(|s| find_set_folder_name(&s.folders, &folder_id))
                    .unwrap_or_default();
                self.open_set_name_dialog(
                    "Rename Package Folder",
                    &current,
                    None,
                    window,
                    cx,
                    move |this, name| {
                        if let Some(set) = this.request_sets.iter_mut().find(|s| s.id == set_id) {
                            rename_set_folder(&mut set.folders, &folder_id, &name);
                        }
                    },
                );
            }
            SetsPanelEvent::DeleteFolder { set_id, folder_id } => {
                if let Some(set) = self.request_sets.iter_mut().find(|s| s.id == set_id) {
                    remove_set_folder(&mut set.folders, &folder_id);
                    let _ = save_workspace_sets(&self.workspace.root_dir, &self.request_sets);
                    self.sync_sidebars(None, cx);
                }
            }
            SetsPanelEvent::OpenRequestRef {
                project_id,
                request_id,
            } => {
                let exists = self
                    .workspace
                    .find_project(&project_id)
                    .and_then(|p| p.find_request(&request_id))
                    .is_some();
                if exists {
                    self.open_node(
                        ExplorerNode::Request {
                            project_id,
                            request_id,
                        },
                        window,
                        cx,
                    );
                } else {
                    window.push_notification(
                        "Referenced request no longer exists in Explorer.",
                        cx,
                    );
                }
            }
            SetsPanelEvent::RemoveItem { set_id, item_id } => {
                if let Some(set) = self.request_sets.iter_mut().find(|s| s.id == set_id) {
                    let _ = take_item_from_set(set, &item_id);
                    let _ = save_workspace_sets(&self.workspace.root_dir, &self.request_sets);
                    self.sync_sidebars(None, cx);
                }
            }
            SetsPanelEvent::AddRequestRef {
                target_set_id,
                target_folder_id,
                project_id,
                request_id,
                name,
                method,
                url,
            } => {
                let item = RequestSetItem {
                    id: generate_id("seti"),
                    project_id,
                    request_id,
                    name,
                    method,
                    url,
                };
                if let Some(set) = self.request_sets.iter_mut().find(|s| s.id == target_set_id) {
                    insert_item_into_set(set, target_folder_id.as_deref(), item);
                    let _ = save_workspace_sets(&self.workspace.root_dir, &self.request_sets);
                    self.sync_sidebars(None, cx);
                }
            }
            SetsPanelEvent::MoveItem {
                source_set_id,
                item_id,
                target_set_id,
                target_folder_id,
            } => {
                let Some(src_set) = self
                    .request_sets
                    .iter_mut()
                    .find(|s| s.id == source_set_id)
                else {
                    return;
                };
                let Some(item) = take_item_from_set(src_set, &item_id) else {
                    return;
                };
                if let Some(dst_set) = self
                    .request_sets
                    .iter_mut()
                    .find(|s| s.id == target_set_id)
                {
                    insert_item_into_set(dst_set, target_folder_id.as_deref(), item);
                }
                let _ = save_workspace_sets(&self.workspace.root_dir, &self.request_sets);
                self.sync_sidebars(None, cx);
            }
        }
    }

    fn open_set_name_dialog<F>(
        &mut self,
        title: &'static str,
        initial: &str,
        _placeholder: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
        on_confirm: F,
    ) where
        F: FnOnce(&mut Self, String) + 'static,
    {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Enter name…")
                .default_value(initial.to_string())
        });
        let app_entity = cx.entity();
        let on_confirm = std::rc::Rc::new(std::cell::RefCell::new(Some(on_confirm)));
        let initial_focus = Cell::new(true);

        let input_enter = input.clone();
        let cb_enter = on_confirm.clone();
        let sub = cx.subscribe_in(
            &input,
            window,
            move |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    let val = input_enter.read(cx).value().trim().to_string();
                    if val.is_empty() {
                        return;
                    }
                    if let Some(cb) = cb_enter.borrow_mut().take() {
                        cb(this, val);
                        let _ = save_workspace_sets(&this.workspace.root_dir, &this.request_sets);
                        this.sync_sidebars(None, cx);
                    }
                    window.close_dialog(cx);
                }
            },
        );
        self._subscriptions.push(sub);

        window.open_dialog(cx, move |dialog, window, cx| {
            if initial_focus.replace(false) {
                input.focus_handle(cx).focus(window, cx);
            }
            let input_ok = input.clone();
            let app_ok = app_entity.clone();
            let cb_ok = on_confirm.clone();
            dialog
                .title(title)
                .w(px(360.))
                .child(
                    v_flex()
                        .gap_2()
                        .child(Input::new(&input).small()),
                )
                .footer(
                    DialogFooter::new()
                        .gap_2()
                        .child(
                            DialogClose::new().child(
                                Button::new("set-dlg-cancel")
                                    .outline()
                                    .small()
                                    .label("Cancel"),
                            ),
                        )
                        .child(
                            Button::new("set-dlg-ok")
                                .primary()
                                .small()
                                .label("OK")
                                .on_click(move |_, window, cx| {
                                    let val = input_ok.read(cx).value().trim().to_string();
                                    if val.is_empty() {
                                        return;
                                    }
                                    if let Some(cb) = cb_ok.borrow_mut().take() {
                                        app_ok.update(cx, |this, cx| {
                                            cb(this, val);
                                            let _ = save_workspace_sets(
                                                &this.workspace.root_dir,
                                                &this.request_sets,
                                            );
                                            this.sync_sidebars(None, cx);
                                        });
                                    }
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
    }

    fn create_request_from_doc(
        &mut self,
        target_project_id: &str,
        target_folder_id: Option<&str>,
        mut doc: HttpRequestDoc,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(proj) = self.workspace.find_project_mut(target_project_id) else {
            return;
        };
        let target_dir = if let Some(fid) = target_folder_id {
            find_folder_dir_in_folders(&proj.folders, fid).unwrap_or_else(|| proj.root_dir.clone())
        } else {
            proj.root_dir.clone()
        };
        let mut created = match create_request(
            &target_dir,
            &doc.name,
            doc.request.method,
            &doc.request.url,
        ) {
            Ok(n) => n,
            Err(err) => {
                window.push_notification(format!("Failed creating request: {err}"), cx);
                return;
            }
        };
        doc.id = created.doc.id.clone();
        created.doc = doc;
        if let Err(err) = save_request(&created) {
            window.push_notification(format!("Failed saving request: {err}"), cx);
            return;
        }
        let req_name = created.doc.name.clone();
        let req_id = created.doc.id.clone();
        insert_request_into_project(proj, target_folder_id, created);
        self.commit_git_change(&format!("Create request: {req_name}"));
        self.sync_sidebars(None, cx);
        self.open_node(
            ExplorerNode::Request {
                project_id: target_project_id.to_string(),
                request_id: req_id,
            },
            window,
            cx,
        );
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
                if env_id == GLOBAL_ENV_ID {
                    match save_global_environment(&self.workspace.root_dir, &updated) {
                        Ok(node) => {
                            self.workspace.global_environment = Some(node);
                            if !renamed_keys.is_empty() {
                                for proj in &mut self.workspace.projects {
                                    propagate_variable_rewrites_in_project(
                                        proj,
                                        &renamed_keys,
                                        true,
                                    );
                                }
                            }
                            self.commit_git_change("Update global environment");
                            self.sync_env_editor(window, cx);
                            if !renamed_keys.is_empty() {
                                self.rewrite_variables_in_open_tabs(
                                    &renamed_keys,
                                    true,
                                    window,
                                    cx,
                                );
                            }
                            self.sync_all_tabs(window, cx);
                            self.env_editor.update(cx, |editor, cx| {
                                window.focus(&editor.focus_handle(cx), cx);
                            });
                            self.maybe_auto_git_sync(window, cx);
                            window.push_notification("Global environment saved.", cx);
                        }
                        Err(err) => {
                            window.push_notification(
                                format!("Failed saving global environment: {err}"),
                                cx,
                            );
                        }
                    }
                    return;
                }
                let is_active =
                    self.workspace.active_environment_id.as_deref() == Some(&env_id);
                if let Some(env_node) = self
                    .workspace
                    .environments
                    .iter_mut()
                    .find(|e| e.doc.id == env_id)
                {
                    env_node.doc = updated;
                    if let Err(err) = save_environment(env_node) {
                        window.push_notification(format!("Failed saving profile: {err}"), cx);
                        return;
                    }
                    let env_name = env_node.doc.name.clone();
                    self.workspace
                        .environments
                        .sort_by_key(|a| a.doc.name.to_lowercase());
                    if is_active && !renamed_keys.is_empty() {
                        for proj in &mut self.workspace.projects {
                            propagate_variable_rewrites_in_project(proj, &renamed_keys, true);
                        }
                    }
                    self.commit_git_change(&format!("Update environment: {env_name}"));
                    self.refresh_profile_selector(window, cx);
                    self.sync_env_editor(window, cx);
                    if is_active && !renamed_keys.is_empty() {
                        self.rewrite_variables_in_open_tabs(&renamed_keys, true, window, cx);
                    }
                    self.sync_all_tabs(window, cx);
                    self.env_editor.update(cx, |editor, cx| {
                        window.focus(&editor.focus_handle(cx), cx);
                    });
                    self.maybe_auto_git_sync(window, cx);
                    window.push_notification("Profile saved.", cx);
                }
            }
            EnvEditorEvent::Create { name } => {
                match create_environment(&self.workspace.root_dir, &name) {
                    Ok(env_node) => {
                        let new_id = env_node.doc.id.clone();
                        let env_name = env_node.doc.name.clone();
                        self.workspace.environments.push(env_node);
                        self.workspace
                            .environments
                            .sort_by_key(|a| a.doc.name.to_lowercase());
                        self.commit_git_change(&format!("Create environment: {env_name}"));
                        self.select_environment(Some(new_id.clone()), window, cx);
                        self.sync_env_editor(window, cx);
                        self.env_editor.update(cx, |editor, cx| {
                            editor.focus_profile(&new_id, window, cx);
                        });
                        self.maybe_auto_git_sync(window, cx);
                    }
                    Err(err) => {
                        window.push_notification(format!("Failed creating profile: {err}"), cx);
                    }
                }
            }
            EnvEditorEvent::Delete { env_id } => {
                let Some(env_name) = self
                    .workspace
                    .environments
                    .iter()
                    .find(|e| e.doc.id == env_id)
                    .map(|e| e.doc.name.clone())
                else {
                    return;
                };
                let app_entity = cx.entity();
                window.open_alert_dialog(cx, move |alert, _, _| {
                    let app_ok = app_entity.clone();
                    let eid = env_id.clone();
                    let title_name = env_name.clone();
                    alert
                        .title(format!("Delete profile \"{title_name}\"?"))
                        .description("This permanently removes the profile YAML file from disk.")
                        .confirm()
                        .ok_text("Delete")
                        .ok_variant(ButtonVariant::Danger)
                        .on_ok(move |_, window, cx| {
                            app_ok.update(cx, |this, cx| {
                                let Some(pos) = this
                                    .workspace
                                    .environments
                                    .iter()
                                    .position(|e| e.doc.id == eid)
                                else {
                                    return;
                                };
                                let removed = this.workspace.environments.remove(pos);
                                if let Err(err) = delete_environment(&removed.file_path) {
                                    window.push_notification(
                                        format!("Failed deleting profile: {err}"),
                                        cx,
                                    );
                                    this.workspace.environments.insert(pos, removed);
                                    return;
                                }
                                this.commit_git_change(&format!(
                                    "Delete environment: {}",
                                    removed.doc.name
                                ));
                                if this.workspace.active_environment_id.as_deref() == Some(&eid) {
                                    this.workspace.active_environment_id = this
                                        .workspace
                                        .environments
                                        .first()
                                        .map(|e| e.doc.id.clone());
                                    let _ = save_workspace_state(
                                        &this.workspace_dir,
                                        this.workspace.active_project_id.as_deref(),
                                        this.workspace.active_environment_id.as_deref(),
                                    );
                                }
                                this.refresh_profile_selector(window, cx);
                                this.sync_env_editor(window, cx);
                                this.sync_all_tabs(window, cx);
                                this.maybe_auto_git_sync(window, cx);
                            });
                            true
                        })
                });
            }
            EnvEditorEvent::SelectActive { env_id } => {
                self.select_environment(env_id, window, cx);
                self.sync_env_editor(window, cx);
            }
            EnvEditorEvent::ReplaceVariable { key, value } => {
                let pairs = [(key.clone(), value)];
                for proj in &mut self.workspace.projects {
                    propagate_variable_rewrites_in_project(proj, &pairs, false);
                }
                self.rewrite_variables_in_open_tabs(&pairs, false, window, cx);
                self.sync_all_tabs(window, cx);
                window.push_notification(
                    format!("Replaced \"{{{{{key}}}}}\" with its real value."),
                    cx,
                );
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
        let global_profile = self.workspace.global_environment.clone();
        let profiles = self.workspace.environments.clone();
        let active_id = self.workspace.active_environment_id.clone();
        self.env_editor.update(cx, |editor, cx| {
            editor.sync_profiles(global_profile, profiles, active_id, window, cx);
        });
    }

    fn set_active_project(&mut self, project_id: &str, _window: &mut Window, _cx: &mut Context<Self>) {
        if self.workspace.active_project_id.as_deref() == Some(project_id) {
            return;
        }
        if self.workspace.find_project(project_id).is_none() {
            return;
        }

        self.workspace.active_project_id = Some(project_id.to_string());
        let _ = save_workspace_state(
            &self.workspace.root_dir,
            self.workspace.active_project_id.as_deref(),
            self.workspace.active_environment_id.as_deref(),
        );
    }

    pub fn open_node(&mut self, node: ExplorerNode, window: &mut Window, cx: &mut Context<Self>) {
        let project_id = match &node {
            ExplorerNode::Project { project_id }
            | ExplorerNode::Folder { project_id, .. }
            | ExplorerNode::Request { project_id, .. } => project_id.clone(),
        };
        self.set_active_project(&project_id, window, cx);

        let target_key = match &node {
            ExplorerNode::Project { project_id } => format!("proj:{project_id}"),
            ExplorerNode::Folder { folder_id, .. } => format!("fld:{folder_id}"),
            ExplorerNode::Request { request_id, .. } => format!("req:{request_id}"),
        };

        if let Some(existing_ix) = self.tabs.iter().position(|tab| tab.key(cx) == target_key) {
            self.active_tab_ix = Some(existing_ix);
            self.reveal_and_select_in_explorers(&target_key, cx);
            self.persist_open_tabs(cx);
            cx.notify();
            return;
        }

        let active_env = self.workspace.effective_environment();
        let global_env = self
            .workspace
            .global_environment
            .as_ref()
            .map(|e| e.doc.clone());

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
                let cached_resp = if self.remember_responses {
                    load_request_response(&self.workspace.root_dir, &request_id)
                } else {
                    None
                };
                let editor_font_size = f32::from(self.editor_font_size);
                let view = cx.new(|cx| {
                    let mut tab = RequestTabView::new(
                        project, &ancestors, req_node, active_env, global_env, runner, window, cx,
                    );
                    tab.set_editor_font_size(editor_font_size, cx);
                    if let Some(snap) = cached_resp {
                        tab.restore_response(snap, window, cx);
                    }
                    tab
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
                let share_settings = self.share_settings.clone();
                let view = cx.new(|cx| {
                    ContainerTabView::new_project(project, active_env, share_settings, window, cx)
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
        self.reveal_and_select_in_explorers(&target_key, cx);
        self.refresh_dirty_indicators(cx);
        self.persist_open_tabs(cx);
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
                if project_id.is_empty() {
                    // Ephemeral tab (e.g. opened from a deleted History entry): save into the first project.
                    let Some(first_proj) = self.workspace.projects.first_mut() else {
                        window.push_notification(
                            "Create a project first before saving this request.",
                            cx,
                        );
                        return;
                    };
                    let target_proj_id = first_proj.doc.id.clone();
                    let mut req_node = match create_request(
                        &first_proj.root_dir,
                        &doc.name,
                        doc.request.method,
                        &doc.request.url,
                    ) {
                        Ok(n) => n,
                        Err(err) => {
                            window.push_notification(
                                format!("Failed creating YAML request: {err}"),
                                cx,
                            );
                            return;
                        }
                    };
                    let new_id = req_node.doc.id.clone();
                    req_node.doc = *doc;
                    req_node.doc.id = new_id.clone();
                    if let Err(err) = save_request(&req_node) {
                        window.push_notification(format!("Failed saving YAML request: {err}"), cx);
                        return;
                    }
                    let req_name = req_node.doc.name.clone();
                    let req_file_path = req_node.file_path.clone();
                    first_proj.requests.push(req_node);
                    self.commit_git_change(&format!("Create request: {req_name}"));
                    let old_key = format!("req:{request_id}");
                    if let Some(WorkbenchTab::Request(tab)) =
                        self.tabs.iter().find(|t| t.key(cx) == old_key)
                    {
                        tab.update(cx, |view, cx| {
                            view.set_location(target_proj_id.clone(), req_file_path);
                            view.mark_saved(cx);
                        });
                    }
                    self.sync_sidebars(None, cx);
                    self.refresh_dirty_indicators(cx);
                    window.push_notification("Saved request to first project.", cx);
                    return;
                }
                if let Some(proj) = self.workspace.find_project_mut(&project_id)
                    && let Some(req_node) = proj.find_request_mut(&request_id)
                {
                    req_node.doc = *doc;
                    if let Err(err) = save_request(req_node) {
                        window.push_notification(format!("Failed saving YAML request: {err}"), cx);
                        return;
                    }
                    let req_name = req_node.doc.name.clone();
                    let req_file_path = req_node.file_path.clone();
                    let changed_file = share_file_from_disk(&proj.root_dir, &req_file_path);
                    self.commit_git_change(&format!("Update request: {req_name}"));
                    info!(
                        project_id = %project_id,
                        request_id = %request_id,
                        changed_file = ?changed_file.as_ref().map(|f| &f.path),
                        "SaveRequest persisted to disk; checking auto-sync"
                    );
                    let target_key = format!("req:{request_id}");
                    if let Some(WorkbenchTab::Request(tab)) =
                        self.tabs.iter().find(|t| t.key(cx) == target_key)
                    {
                        tab.update(cx, |view, cx| view.mark_saved(cx));
                    }
                    self.sync_sidebars(None, cx);
                    self.reveal_and_select_in_explorers(&target_key, cx);
                    self.refresh_dirty_indicators(cx);
                    self.sync_shared_project_after_save(&project_id, changed_file, window, cx);
                    self.maybe_auto_git_sync(window, cx);
                }
            }
            RequestTabEvent::OpenAuthSource { project_id, node } => {
                self.open_auth_source(&project_id, node, window, cx);
            }
            RequestTabEvent::DirtyChanged => {
                self.refresh_dirty_indicators(cx);
            }
            RequestTabEvent::ApplyPostScriptEnv {
                env_id,
                vars,
            } => {
                if let Some(env_node) = self
                    .workspace
                    .environments
                    .iter_mut()
                    .find(|e| e.doc.id == env_id)
                {
                    let changed =
                        crate::post_script::apply_vars_to_environment(&mut env_node.doc, &vars);
                    if !changed.is_empty() {
                        if let Err(err) = save_environment(env_node) {
                            window.push_notification(
                                format!("Failed saving post-script environment: {err}"),
                                cx,
                            );
                            return;
                        }
                        let env_name = env_node.doc.name.clone();
                        self.commit_git_change(&format!("Update environment: {env_name}"));
                        self.sync_env_editor(window, cx);
                        self.sync_all_tabs(window, cx);
                    }
                }
            }
            RequestTabEvent::ApplyPostScriptGlobalEnv { vars } => {
                let mut doc = self
                    .workspace
                    .global_environment
                    .as_ref()
                    .map(|e| e.doc.clone())
                    .unwrap_or_else(|| {
                        testrix_core::EnvironmentDoc::new(GLOBAL_ENV_ID, "Global")
                    });
                let changed = crate::post_script::apply_vars_to_environment(&mut doc, &vars);
                if !changed.is_empty() {
                    match save_global_environment(&self.workspace.root_dir, &doc) {
                        Ok(node) => {
                            self.workspace.global_environment = Some(node);
                            self.commit_git_change("Update global environment");
                            self.sync_env_editor(window, cx);
                            self.sync_all_tabs(window, cx);
                            self.maybe_auto_git_sync(window, cx);
                        }
                        Err(err) => {
                            window.push_notification(
                                format!("Failed saving post-script global environment: {err}"),
                                cx,
                            );
                        }
                    }
                }
            }
            RequestTabEvent::ResponseCompleted {
                request_id,
                snapshot,
            } => {
                if self.remember_responses {
                    let _ = save_request_response(&self.workspace.root_dir, &request_id, &snapshot);
                }
            }
            RequestTabEvent::RequestExecuted {
                project_id,
                request_id,
                doc,
                status_code,
                duration_ms,
            } => {
                let entry = RequestHistoryEntry {
                    id: generate_id("hist"),
                    executed_at: utc_now_iso8601(),
                    project_id,
                    request_id,
                    env_id: self.workspace.active_environment_id.clone(),
                    doc: *doc,
                    status_code,
                    duration_ms,
                };
                self.history_entries.insert(0, entry);
                if self.history_entries.len() > 200 {
                    self.history_entries.truncate(200);
                }
                let _ = save_workspace_history(&self.workspace.root_dir, &self.history_entries);
                self.sync_sidebars(None, cx);
            }
            RequestTabEvent::OpenRequestHistory {
                project_id,
                request_id,
                file_path,
                request_name,
            } => {
                self.open_request_git_history_dialog(
                    project_id,
                    request_id,
                    file_path,
                    request_name,
                    window,
                    cx,
                );
            }
        }
    }

    fn commit_git_change(&self, message: &str) {
        if let Err(err) = commit_workspace_change(&self.workspace_dir, message) {
            tracing::warn!(error = %err, message = %message, "failed to auto-commit workspace change");
        }
    }

    fn format_unix_timestamp(ts: i64) -> String {
        if ts <= 0 {
            return "unknown".to_string();
        }
        let secs = ts as u64;
        let days = secs / 86_400;
        let rem = secs % 86_400;
        let hours = rem / 3_600;
        let mins = (rem % 3_600) / 60;

        // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm)
        let z = days as i64 + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097) as u64;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        format!("{y:04}-{m:02}-{d:02} {hours:02}:{mins:02}")
    }

    fn open_request_git_history_dialog(
        &mut self,
        project_id: String,
        request_id: String,
        file_path: PathBuf,
        request_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let workspace_root = self.workspace_dir.clone();
        let commits: Vec<GitCommitInfo> = match list_file_commits(&workspace_root, &file_path, 50) {
            Ok(list) => list,
            Err(err) => {
                window.push_notification(format!("Failed reading git history: {err}"), cx);
                return;
            }
        };

        let initial_preview = commits.first().and_then(|c| {
            let rel = c
                .file_path_at_commit
                .as_deref()
                .map(str::to_string)
                .or_else(|| {
                    testrix_storage::workspace_relative_path(&workspace_root, &file_path).ok()
                })?;
            read_file_at_commit(&workspace_root, &rel, &c.hash).ok()
        });

        let selected_ix = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let preview_cache = std::rc::Rc::new(std::cell::RefCell::new(initial_preview));
        let app_entity = cx.entity();

        window.open_dialog(cx, move |dialog, _, cx| {
            let commits_list = commits.clone();
            let sel_ix = selected_ix.get().min(commits_list.len().saturating_sub(1));
            let selected_commit = commits_list.get(sel_ix).cloned();
            let preview_text = preview_cache
                .borrow()
                .clone()
                .unwrap_or_else(|| "No preview available.".to_string());

            let app_restore = app_entity.clone();
            let pid_restore = project_id.clone();
            let rid_restore = request_id.clone();
            let fpath_restore = file_path.clone();
            let rname_restore = request_name.clone();
            let wroot_restore = workspace_root.clone();

            dialog
                .title(format!("History: {request_name}"))
                .width(px(820.))
                .child(if commits_list.is_empty() {
                    div()
                        .p_4()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("No git commit history found for this request yet. Save a change to create a commit.")
                        .into_any_element()
                } else {
                    h_flex()
                        .w_full()
                        .h(px(420.))
                        .gap_3()
                        .child(
                            v_flex()
                                .id("req-git-history-list")
                                .w(px(280.))
                                .h_full()
                                .overflow_y_scrollbar()
                                .border_1()
                                .border_color(cx.theme().border)
                                .rounded_md()
                                .p_1p5()
                                .gap_1()
                                .children(commits_list.iter().enumerate().map(|(ix, commit)| {
                                    let is_selected = ix == sel_ix;
                                    let sel_ref = selected_ix.clone();
                                    let prev_ref = preview_cache.clone();
                                    let wroot = workspace_root.clone();
                                    let fpath = file_path.clone();
                                    let commit_clone = commit.clone();
                                    let ts_label = Self::format_unix_timestamp(commit.timestamp);

                                    div()
                                        .id(SharedString::from(format!("git-hist-item-{ix}")))
                                        .w_full()
                                        .px_2p5()
                                        .py_2()
                                        .rounded_sm()
                                        .cursor_pointer()
                                        .border_1()
                                        .when(is_selected, |el| {
                                            el.bg(cx.theme().secondary)
                                                .border_color(cx.theme().primary)
                                        })
                                        .when(!is_selected, |el| {
                                            el.border_color(gpui_kit::transparent_black())
                                                .hover(|s| s.bg(cx.theme().secondary.opacity(0.5)))
                                        })
                                        .on_click(move |_, window, _| {
                                            sel_ref.set(ix);
                                            let rel = commit_clone
                                                .file_path_at_commit
                                                .clone()
                                                .or_else(|| {
                                                    testrix_storage::workspace_relative_path(
                                                        &wroot, &fpath,
                                                    )
                                                    .ok()
                                                });
                                            let next_preview = rel.and_then(|r| {
                                                read_file_at_commit(&wroot, &r, &commit_clone.hash)
                                                    .ok()
                                            });
                                            *prev_ref.borrow_mut() = next_preview;
                                            window.refresh();
                                        })
                                        .child(
                                            v_flex()
                                                .gap_1()
                                                .child(
                                                    h_flex()
                                                        .justify_between()
                                                        .gap_2()
                                                        .child(
                                                            Tag::new()
                                                                .outline()
                                                                .small()
                                                                .child(commit.short_hash.clone()),
                                                        )
                                                        .child(
                                                            div()
                                                                .text_xs()
                                                                .text_color(
                                                                    cx.theme().muted_foreground,
                                                                )
                                                                .child(ts_label),
                                                        ),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .font_medium()
                                                        .truncate()
                                                        .child(commit.message.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .truncate()
                                                        .child(commit.author.clone()),
                                                ),
                                        )
                                })),
                        )
                        .child(
                            div()
                                .id("req-git-history-preview")
                                .flex_1()
                                .h_full()
                                .overflow_y_scrollbar()
                                .border_1()
                                .border_color(cx.theme().border)
                                .rounded_md()
                                .bg(cx.theme().secondary.opacity(0.3))
                                .p_3()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_xs()
                                .child(preview_text),
                        )
                        .into_any_element()
                })
                .footer(
                    DialogFooter::new()
                        .gap_2()
                        .child(
                            DialogClose::new()
                                .child(Button::new("git-hist-close").label("Close").outline()),
                        )
                        .when_some(selected_commit, |footer, commit| {
                            footer.child(
                                Button::new("git-hist-restore")
                                    .primary()
                                    .icon(Icon::new(LucideIcon::RotateCcwClock))
                                    .label(format!("Restore ({})", commit.short_hash))
                                    .on_click(move |_, window, cx| {
                                        let restored_yaml = match restore_file_from_commit(
                                            &wroot_restore,
                                            &fpath_restore,
                                            &commit.hash,
                                            commit.file_path_at_commit.as_deref(),
                                        ) {
                                            Ok(content) => content,
                                            Err(err) => {
                                                window.push_notification(
                                                    format!("Restore failed: {err}"),
                                                    cx,
                                                );
                                                return;
                                            }
                                        };

                                        let restored_doc: HttpRequestDoc =
                                            match yaml_serde::from_str(&restored_yaml) {
                                                Ok(doc) => doc,
                                                Err(err) => {
                                                    window.push_notification(
                                                        format!("Restored YAML invalid: {err}"),
                                                        cx,
                                                    );
                                                    return;
                                                }
                                            };

                                        let pid = pid_restore.clone();
                                        let rid = rid_restore.clone();
                                        let short_hash = commit.short_hash.clone();
                                        let rname = rname_restore.clone();
                                        app_restore.update(cx, |this, cx| {
                                            if let Some(proj) = this.workspace.find_project_mut(&pid)
                                                && let Some(req_node) = proj.find_request_mut(&rid)
                                            {
                                                req_node.doc = restored_doc.clone();
                                            }
                                            let target_key = format!("req:{rid}");
                                            if let Some(WorkbenchTab::Request(tab)) =
                                                this.tabs.iter().find(|t| t.key(cx) == target_key)
                                            {
                                                tab.update(cx, |view, cx| {
                                                    view.recover_from_doc(&restored_doc, window, cx);
                                                    view.mark_saved(cx);
                                                });
                                            }
                                            this.sync_sidebars(None, cx);
                                            this.refresh_dirty_indicators(cx);
                                            this.maybe_auto_git_sync(window, cx);
                                        });
                                        window.close_dialog(cx);
                                        window.push_notification(
                                            format!("Restored \"{rname}\" to {short_hash}."),
                                            cx,
                                        );
                                    }),
                            )
                        }),
                )
        });
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
                    let old_base_url = proj.doc.base_url.trim().to_string();
                    let new_base_url = doc.base_url.trim().to_string();
                    proj.doc = doc;
                    if let Err(err) = save_project(proj) {
                        window.push_notification(format!("Failed saving project: {err}"), cx);
                        return;
                    }
                    let base_changed = !old_base_url.is_empty()
                        && !new_base_url.is_empty()
                        && old_base_url != new_base_url;
                    if base_changed {
                        propagate_base_url_in_project(proj, &old_base_url, &new_base_url);
                    }
                    let target_key = format!("proj:{project_id}");
                    if let Some(WorkbenchTab::Container(tab)) =
                        self.tabs.iter().find(|t| t.key(cx) == target_key)
                    {
                        tab.update(cx, |view, cx| view.mark_saved(cx));
                    }
                    if base_changed {
                        for tab in &self.tabs {
                            if let WorkbenchTab::Request(view) = tab
                                && view.read(cx).project_id() == project_id
                            {
                                view.update(cx, |v, cx| {
                                    v.replace_url_base(&old_base_url, &new_base_url, window, cx);
                                });
                            }
                        }
                    }
                    let proj_name = proj.doc.name.clone();
                    let changed_file = if base_changed {
                        None
                    } else {
                        share_file_from_disk(&proj.root_dir, &proj.manifest_path)
                    };
                    self.commit_git_change(&format!("Update project: {proj_name}"));
                    self.sync_sidebars(None, cx);
                    self.reveal_and_select_in_explorers(&target_key, cx);
                    self.sync_all_tabs(window, cx);
                    self.refresh_dirty_indicators(cx);
                    self.sync_shared_project_after_save(&project_id, changed_file, window, cx);
                    self.maybe_auto_git_sync(window, cx);
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
                    let folder_name = folder.doc.name.clone();
                    let folder_manifest_path = folder.manifest_path.clone();
                    let changed_file = share_file_from_disk(&proj.root_dir, &folder_manifest_path);
                    self.commit_git_change(&format!("Update folder: {folder_name}"));
                    self.sync_sidebars(None, cx);
                    self.reveal_and_select_in_explorers(&target_key, cx);
                    self.sync_all_tabs(window, cx);
                    self.refresh_dirty_indicators(cx);
                    self.sync_shared_project_after_save(&project_id, changed_file, window, cx);
                    self.maybe_auto_git_sync(window, cx);
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
            ContainerTabEvent::ShareStarted(record) => {
                info!(
                    share_id = %record.id,
                    project_id = %record.project_id,
                    project_slug = %record.project_slug,
                    channel = %record.channel.label(),
                    "ShareStarted received"
                );
                if let Some(prev) = self.active_shares.iter().find(|s| s.id == record.id)
                    && let Some(handle) = &prev.local_handle
                {
                    handle.stop();
                }
                self.active_shares.retain(|s| s.id != record.id);
                self.active_shares.push(record);
                self.persist_remote_shares();
                cx.notify();
            }
        }
    }

    fn persist_remote_shares(&self) {
        let persisted: Vec<PersistedShareRecord> = self
            .active_shares
            .iter()
            .filter(|s| s.channel != ShareChannel::LocalNetwork)
            .map(|s| PersistedShareRecord {
                id: s.id.clone(),
                project_id: s.project_id.clone(),
                project_name: s.project_name.clone(),
                project_slug: s.project_slug.clone(),
                channel: s.channel.label().to_string(),
                share_url: s.share_url.clone(),
                target_info: s.target_info.clone(),
            })
            .collect();
        match save_config_shared_projects(&persisted) {
            Ok(()) => info!(count = persisted.len(), "persisted remote share records to config.yaml"),
            Err(err) => tracing::warn!(error = %err, "failed persisting remote share records"),
        }
    }

    fn sync_shared_project_after_save(
        &mut self,
        project_id: &str,
        changed_file: Option<ShareFile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        info!(
            project_id = %project_id,
            active_shares = self.active_shares.len(),
            changed_file = ?changed_file.as_ref().map(|f| &f.path),
            "sync_shared_project_after_save called"
        );
        let Some(proj) = self.workspace.find_project(project_id) else {
            tracing::warn!(project_id = %project_id, "sync_shared_project_after_save: project not found in workspace");
            return;
        };
        for share in &self.active_shares {
            if share.project_id == project_id
                && share.channel == ShareChannel::LocalNetwork
                && let Some(handle) = &share.local_handle
            {
                handle.update_project(proj);
            }
        }
        let targets: Vec<ActiveShareRecord> = self
            .active_shares
            .iter()
            .filter(|s| s.project_id == project_id && s.channel != ShareChannel::LocalNetwork)
            .cloned()
            .collect();
        if targets.is_empty() {
            info!(project_id = %project_id, "no remote active_shares for project; skipping upload");
            return;
        }
        let project_name = proj.doc.name.clone();
        let project_root = proj.root_dir.clone();
        let settings = self.share_settings.clone();
        info!(
            project_id = %project_id,
            target_count = targets.len(),
            r2_bucket = %settings.cloudflare_r2_bucket,
            r2_account = %settings.cloudflare_account_id,
            "starting background share sync"
        );

        for share in targets {
            let files = match share.channel {
                ShareChannel::TestrixServer => match collect_project_yaml_files(&project_root) {
                    Ok(f) => f,
                    Err(err) => {
                        window.push_notification(format!("Share sync failed: {err}"), cx);
                        continue;
                    }
                },
                ShareChannel::CloudflareR2 => {
                    if let Some(file) = changed_file.clone() {
                        vec![file]
                    } else {
                        match collect_project_yaml_files(&project_root) {
                            Ok(f) => f,
                            Err(err) => {
                                window.push_notification(format!("Share sync failed: {err}"), cx);
                                continue;
                            }
                        }
                    }
                }
                ShareChannel::LocalNetwork => continue,
            };

            let rx = match share.channel {
                ShareChannel::TestrixServer => dispatch_share_future(upload_to_testrix_server(
                    settings.share_server_url.clone(),
                    settings.share_token.clone(),
                    share.project_slug.clone(),
                    project_name.clone(),
                    settings.share_access_token.clone(),
                    files,
                )),
                ShareChannel::CloudflareR2 => dispatch_share_future(upload_to_cloudflare_r2(
                    settings.cloudflare_account_id.clone(),
                    settings.cloudflare_r2_bucket.clone(),
                    settings.cloudflare_api_token.clone(),
                    settings.cloudflare_pages_url.clone(),
                    share.project_slug.clone(),
                    settings.share_access_token.clone(),
                    files,
                )),
                ShareChannel::LocalNetwork => unreachable!(),
            };

            self.share_sync_count += 1;
            cx.notify();

            cx.spawn_in(window, async move |weak_self, cx| {
                let outcome = rx.await;
                _ = weak_self.update_in(cx, |this, window, cx| {
                    this.share_sync_count = this.share_sync_count.saturating_sub(1);
                    match outcome {
                        Ok(Ok(res)) => {
                            info!(
                                project = %share.project_slug,
                                channel = %res.channel.label(),
                                files = res.files_uploaded,
                                "background share sync completed"
                            );
                        }
                        Ok(Err(err)) => {
                            tracing::warn!(error = %err, "background share sync failed");
                            window.push_notification(format!("Share sync failed: {err}"), cx);
                        }
                        Err(_) => {
                            window.push_notification(
                                "Share sync failed: upload task cancelled".to_string(),
                                cx,
                            );
                        }
                    }
                    if this.share_sync_count == 0 && this.quit_after_sync {
                        cx.quit();
                    }
                    cx.notify();
                });
            })
            .detach();
        }
    }

    fn request_app_exit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.force_quit || self.share_sync_count == 0 {
            cx.quit();
        } else if !self.exit_prompt_open {
            self.open_exit_sync_dialog(window, cx);
        }
    }

    fn open_close_behavior_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.exit_prompt_open = true;
        let app_entity = cx.entity();
        let remember_state = std::rc::Rc::new(Cell::new(true));

        window.open_dialog(cx, move |dialog, _, _cx| {
            let app_cancel = app_entity.clone();
            let app_tray = app_entity.clone();
            let app_quit = app_entity.clone();
            let rem_toggle = remember_state.clone();
            let rem_tray = remember_state.clone();
            let rem_quit = remember_state.clone();
            let checked = remember_state.get();

            dialog
                .title(crate::i18n::t("close_prompt_title"))
                .width(px(460.))
                .child(
                    v_flex()
                        .gap_3()
                        .child(div().text_sm().child(crate::i18n::t("close_prompt_desc")))
                        .child(
                            Checkbox::new("close-remember-choice")
                                .checked(checked)
                                .label(crate::i18n::t("remember_choice"))
                                .on_click(move |next, window, _| {
                                    rem_toggle.set(*next);
                                    window.refresh();
                                }),
                        ),
                )
                .footer(
                    DialogFooter::new()
                        .gap_2()
                        .child(
                            DialogClose::new().child(
                                Button::new("close-prompt-cancel")
                                    .outline()
                                    .label(crate::i18n::t("cancel"))
                                    .on_click(move |_, _, cx| {
                                        app_cancel.update(cx, |this, cx| {
                                            this.exit_prompt_open = false;
                                            cx.notify();
                                        });
                                    }),
                            ),
                        )
                        .child(
                            DialogClose::new().child(
                                Button::new("close-prompt-tray")
                                    .outline()
                                    .label(crate::i18n::t("minimize_to_tray"))
                                    .on_click(move |_, window, cx| {
                                        let remember = rem_tray.get();
                                        app_tray.update(cx, |this, cx| {
                                            this.exit_prompt_open = false;
                                            if remember {
                                                this.set_close_behavior(
                                                    Some("tray"),
                                                    window,
                                                    cx,
                                                );
                                            }
                                            cx.notify();
                                        });
                                        hide_window_to_tray(window, cx);
                                    }),
                            ),
                        )
                        .child(
                            DialogClose::new().child(
                                Button::new("close-prompt-quit")
                                    .primary()
                                    .label(crate::i18n::t("quit_app"))
                                    .on_click(move |_, window, cx| {
                                        let remember = rem_quit.get();
                                        app_quit.update(cx, |this, cx| {
                                            this.exit_prompt_open = false;
                                            if remember {
                                                this.set_close_behavior(
                                                    Some("quit"),
                                                    window,
                                                    cx,
                                                );
                                            }
                                            if this.share_sync_count > 0 {
                                                this.open_exit_sync_dialog(window, cx);
                                            } else {
                                                this.force_quit = true;
                                                cx.quit();
                                            }
                                        });
                                    }),
                            ),
                        ),
                )
        });
    }

    fn set_close_behavior(
        &mut self,
        behavior: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_behavior = behavior.map(str::to_string);
        let ix = match behavior {
            Some("tray") => 1,
            Some("quit") => 2,
            _ => 0,
        };
        self.close_behavior_select.update(cx, |sel, cx| {
            sel.set_selected_index(Some(IndexPath::default().row(ix)), window, cx);
        });
        let _ = save_config_preferences(
            self.remember_open_tabs,
            self.remember_responses,
            crate::i18n::current_language().code(),
            self.close_behavior.as_deref(),
        );
    }

    fn open_exit_sync_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.exit_prompt_open = true;
        let app_entity = cx.entity();
        let syncing_count = self.share_sync_count;
        window.open_dialog(cx, move |dialog, _, _| {
            let app_cancel = app_entity.clone();
            let app_now = app_entity.clone();
            let app_wait = app_entity.clone();
            dialog
                .title("Background Share Sync in Progress")
                .width(px(460.))
                .child(
                    div().text_sm().child(format!(
                        "{syncing_count} background share upload(s) are still running. Do you want to wait for them to finish before exiting?"
                    )),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new().child(
                                Button::new("exit-sync-cancel")
                                    .outline()
                                    .label("Cancel")
                                    .on_click(move |_, _, cx| {
                                        app_cancel.update(cx, |this, cx| {
                                            this.exit_prompt_open = false;
                                            cx.notify();
                                        });
                                    }),
                            ),
                        )
                        .child(
                            DialogClose::new().child(
                                Button::new("exit-sync-now")
                                    .danger()
                                    .label("Exit Now")
                                    .on_click(move |_, _, cx| {
                                        app_now.update(cx, |this, cx| {
                                            this.force_quit = true;
                                            this.exit_prompt_open = false;
                                            cx.quit();
                                        });
                                    }),
                            ),
                        )
                        .child(
                            DialogClose::new().child(
                                Button::new("exit-sync-wait")
                                    .primary()
                                    .label("Wait & Exit")
                                    .on_click(move |_, _, cx| {
                                        app_wait.update(cx, |this, cx| {
                                            this.exit_prompt_open = false;
                                            this.quit_after_sync = true;
                                            if this.share_sync_count == 0 {
                                                cx.quit();
                                            } else {
                                                cx.notify();
                                            }
                                        });
                                    }),
                            ),
                        ),
                )
        });
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

    fn rewrite_variables_in_open_tabs(
        &mut self,
        pairs: &[(String, String)],
        wrap_braces: bool,
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
                    let belongs_to_active = !wrap_braces
                        || active_project_id
                            .as_deref()
                            .is_some_and(|pid| pid == view.read(cx).project_id());
                    if belongs_to_active {
                        view.update(cx, |v, cx| {
                            v.rewrite_variables(pairs, wrap_braces, window, cx);
                        });
                    }
                }
                WorkbenchTab::Container(view) => {
                    view.update(cx, |v, cx| {
                        v.rewrite_variables(pairs, wrap_braces, window, cx);
                    });
                }
            }
        }
    }

    fn sync_all_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active_env = self.workspace.effective_environment();
        let global_env = self
            .workspace
            .global_environment
            .as_ref()
            .map(|e| e.doc.clone());
        for tab in &self.tabs {
            match tab {
                WorkbenchTab::Request(view) => {
                    let project_id = view.read(cx).project_id().to_string();
                    if let Some(project) = self.workspace.find_project(&project_id) {
                        let env = active_env.clone();
                        let genv = global_env.clone();
                        view.update(cx, |v, cx| v.sync_context(project, env, genv, window, cx));
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
        self.set_dirty_keys_in_explorers(dirty, cx);
    }

    fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(ix) {
            let key = tab.key(cx);
            self.request_close_tabs(vec![key], window, cx);
        }
    }

    fn close_other_tabs(&mut self, keep_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let keys: Vec<String> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(ix, _)| *ix != keep_ix)
            .map(|(_, t)| t.key(cx))
            .collect();
        self.request_close_tabs(keys, window, cx);
    }

    fn close_all_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let keys: Vec<String> = self.tabs.iter().map(|t| t.key(cx)).collect();
        self.request_close_tabs(keys, window, cx);
    }

    fn close_tabs_by_keys(&mut self, keys: &[String], cx: &mut Context<Self>) {
        if keys.is_empty() {
            return;
        }
        let active_key = self
            .active_tab_ix
            .and_then(|ix| self.tabs.get(ix))
            .map(|t| t.key(cx));
        self.tabs.retain(|t| !keys.contains(&t.key(cx)));
        self.active_tab_ix = if self.tabs.is_empty() {
            None
        } else if let Some(ref ak) = active_key
            && let Some(pos) = self.tabs.iter().position(|t| &t.key(cx) == ak)
        {
            Some(pos)
        } else {
            Some(0)
        };
        self.context_tab_ix = None;
        self.refresh_dirty_indicators(cx);
        self.persist_open_tabs(cx);
        cx.notify();
    }

    fn save_tabs_by_keys(&mut self, keys: &[String], cx: &mut Context<Self>) {
        for tab in &self.tabs {
            if keys.contains(&tab.key(cx)) && tab.is_dirty(cx) {
                match tab {
                    WorkbenchTab::Request(view) => view.update(cx, |v, cx| v.save(cx)),
                    WorkbenchTab::Container(view) => view.update(cx, |v, cx| v.save(cx)),
                }
            }
        }
    }

    fn request_close_tabs(
        &mut self,
        keys: Vec<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if keys.is_empty() {
            return;
        }
        let dirty_titles: Vec<String> = self
            .tabs
            .iter()
            .filter(|t| keys.contains(&t.key(cx)) && t.is_dirty(cx))
            .map(|t| t.title(cx))
            .collect();

        if dirty_titles.is_empty() {
            self.close_tabs_by_keys(&keys, cx);
            return;
        }

        let count = dirty_titles.len();
        let title = if count == 1 {
            format!("Save changes to \"{}\"?", dirty_titles[0])
        } else {
            format!("Save changes to {count} unsaved tabs?")
        };
        let description = if count == 1 {
            "This tab has unsaved changes. Do you want to save before closing?".to_string()
        } else {
            format!(
                "Unsaved tabs: {}. Do you want to save before closing?",
                dirty_titles.join(", ")
            )
        };

        let app_entity = cx.entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let app_discard = app_entity.clone();
            let app_save = app_entity.clone();
            let keys_discard = keys.clone();
            let keys_save = keys.clone();
            alert
                .title(title.clone())
                .description(description.clone())
                .footer(
                    DialogFooter::new()
                        .gap_2()
                        .child(
                            DialogClose::new().child(
                                Button::new("tab-close-cancel-btn")
                                    .label(crate::i18n::t("cancel"))
                                    .outline(),
                            ),
                        )
                        .child(
                            Button::new("tab-close-discard-btn")
                                .danger()
                                .label(crate::i18n::t("dont_save"))
                                .on_click(move |_, window, cx| {
                                    app_discard.update(cx, |this, cx| {
                                        this.close_tabs_by_keys(&keys_discard, cx);
                                    });
                                    window.close_dialog(cx);
                                }),
                        )
                        .child(
                            Button::new("tab-close-save-btn")
                                .primary()
                                .label(crate::i18n::t("save_and_close"))
                                .on_click(move |_, window, cx| {
                                    app_save.update(cx, |this, cx| {
                                        this.save_tabs_by_keys(&keys_save, cx);
                                        this.close_tabs_by_keys(&keys_save, cx);
                                    });
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
    }

    fn prompt_import_openapi(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt_import_json_project(
            "Select OpenAPI 3.0 JSON file",
            "OpenAPI 3.0",
            |json| parse_openapi_3_0(json).map_err(|err| format!("OpenAPI parse failed: {err}")),
            window,
            cx,
        );
    }

    fn prompt_import_apifox(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.prompt_import_json_project(
            "Select Apifox JSON file",
            "Apifox",
            |json| parse_apifox(json).map_err(|err| format!("Apifox parse failed: {err}")),
            window,
            cx,
        );
    }

    fn prompt_export_openapi(
        &mut self,
        project_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self
            .workspace
            .projects
            .iter()
            .find(|p| p.doc.id == project_id)
            .cloned()
        else {
            return;
        };

        let json = match export_openapi_3_0(&project) {
            Ok(json) => json,
            Err(err) => {
                window.push_notification(format!("OpenAPI export failed: {err}"), cx);
                return;
            }
        };

        let safe_name: String = project
            .doc
            .name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let suggested_name = format!("{}.openapi.json", safe_name.trim_matches('-'));
        let prompt = cx.prompt_for_new_path(&self.workspace_dir, Some(&suggested_name));

        cx.spawn_in(window, async move |_, window| {
            let path = prompt.await.ok()?.ok()??;
            let result = std::fs::write(&path, json.as_bytes())
                .map_err(|err| format!("Failed writing {}: {err}", path.display()));
            window
                .update(|window, cx| match result {
                    Ok(()) => {
                        window.push_notification(
                            format!("Exported OpenAPI 3.0 JSON to {}.", path.display()),
                            cx,
                        );
                    }
                    Err(err) => {
                        window.push_notification(err, cx);
                    }
                })
                .ok()
        })
        .detach();
    }

    fn prompt_import_json_project(
        &mut self,
        prompt_title: &'static str,
        kind_label: &'static str,
        parser: fn(&str) -> Result<ImportedProjectBundle, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(prompt_title.into()),
        });
        let workspace_root = self.workspace.root_dir.clone();
        let entity = cx.entity();
        cx.spawn_in(window, async move |_, window| {
            let path = prompt.await.ok()?.ok()??.into_iter().next()?;
            let parsed = std::fs::read_to_string(&path)
                .map_err(|err| format!("Failed reading {}: {err}", path.display()))
                .and_then(|json| parser(&json));
            window
                .update(|window, cx| {
                    entity.update(cx, |this, cx| match parsed {
                        Ok(bundle) => {
                            match import_project_bundle(&workspace_root, &bundle) {
                                Ok(proj) => {
                                    let proj_id = proj.doc.id.clone();
                                    let proj_name = proj.doc.name.clone();
                                    if let Ok(envs) = load_workspace_environments(&workspace_root) {
                                        this.workspace.environments = envs;
                                    }
                                    if this.workspace.active_environment_id.is_none() {
                                        this.workspace.active_environment_id = this
                                            .workspace
                                            .environments
                                            .first()
                                            .map(|e| e.doc.id.clone());
                                    }
                                    this.workspace.projects.push(proj);
                                    this.workspace
                                        .projects
                                        .sort_by_key(|p| p.doc.name.to_lowercase());
                                    this.workspace.active_project_id = Some(proj_id.clone());
                                    let _ = save_workspace_state(
                                        &this.workspace.root_dir,
                                        this.workspace.active_project_id.as_deref(),
                                        this.workspace.active_environment_id.as_deref(),
                                    );
                                    this.commit_git_change(&format!("Import {kind_label} project: {proj_name}"));
                                    this.refresh_profile_selector(window, cx);
                                    this.sync_env_editor(window, cx);
                                    this.sync_all_tabs(window, cx);
                                    this.sync_sidebars(None, cx);
                                    this.reveal_and_select_in_explorers(&format!("proj:{proj_id}"), cx);
                                    window.push_notification(
                                        format!("Imported {kind_label} project \"{proj_name}\"."),
                                        cx,
                                    );
                                }
                                Err(err) => {
                                    window.push_notification(
                                        format!("Failed writing imported project: {err}"),
                                        cx,
                                    );
                                }
                            }
                        }
                        Err(msg) => {
                            window.push_notification(msg, cx);
                        }
                    });
                })
                .ok()
        })
        .detach();
    }

    fn prompt_import_postman_zip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select Postman Collection (.zip or .json)".into()),
        });
        let workspace_root = self.workspace.root_dir.clone();
        let entity = cx.entity();
        cx.spawn_in(window, async move |_, window| {
            let path = prompt.await.ok()?.ok()??.into_iter().next()?;
            let is_zip = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
            let parsed = if is_zip {
                std::fs::read(&path)
                    .map_err(|err| format!("Failed reading {}: {err}", path.display()))
                    .and_then(|bytes| {
                        parse_postman_zip(&bytes)
                            .map_err(|err| format!("Postman ZIP parse failed: {err}"))
                    })
            } else {
                std::fs::read_to_string(&path)
                    .map_err(|err| format!("Failed reading {}: {err}", path.display()))
                    .and_then(|json| {
                        parse_postman_collection(&json)
                            .map(|b| vec![b])
                            .map_err(|err| format!("Postman JSON parse failed: {err}"))
                    })
            };

            window
                .update(|window, cx| {
                    entity.update(cx, |this, cx| match parsed {
                        Ok(bundles) => {
                            let mut imported_names = Vec::new();
                            let mut last_proj_id = None;

                            for bundle in bundles {
                                match import_project_bundle(&workspace_root, &bundle) {
                                    Ok(proj) => {
                                        let proj_id = proj.doc.id.clone();
                                        let proj_name = proj.doc.name.clone();
                                        this.workspace.projects.push(proj);
                                        imported_names.push(proj_name);
                                        last_proj_id = Some(proj_id);
                                    }
                                    Err(err) => {
                                        window.push_notification(
                                            format!("Failed writing imported project: {err}"),
                                            cx,
                                        );
                                    }
                                }
                            }

                            if let Some(proj_id) = last_proj_id {
                                if let Ok(envs) = load_workspace_environments(&workspace_root) {
                                    this.workspace.environments = envs;
                                }
                                if this.workspace.active_environment_id.is_none() {
                                    this.workspace.active_environment_id = this
                                        .workspace
                                        .environments
                                        .first()
                                        .map(|e| e.doc.id.clone());
                                }
                                this.workspace
                                    .projects
                                    .sort_by_key(|p| p.doc.name.to_lowercase());
                                this.workspace.active_project_id = Some(proj_id.clone());
                                this.commit_git_change(&format!(
                                    "Import Postman: {}",
                                    imported_names.join(", ")
                                ));
                                let _ = save_workspace_state(
                                    &this.workspace.root_dir,
                                    this.workspace.active_project_id.as_deref(),
                                    this.workspace.active_environment_id.as_deref(),
                                );
                                this.refresh_profile_selector(window, cx);
                                this.sync_env_editor(window, cx);
                                this.sync_all_tabs(window, cx);
                                this.sync_sidebars(None, cx);
                                this.reveal_and_select_in_explorers(&format!("proj:{proj_id}"), cx);
                                window.push_notification(
                                    format!(
                                        "Imported Postman project(s): {}.",
                                        imported_names.join(", ")
                                    ),
                                    cx,
                                );
                            }
                        }
                        Err(msg) => {
                            window.push_notification(msg, cx);
                        }
                    });
                })
                .ok()
        })
        .detach();
    }

    fn move_request_node(
        &mut self,
        source_project_id: &str,
        request_id: &str,
        target_project_id: &str,
        target_folder_id: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target_dir) = self.workspace.find_project(target_project_id).and_then(|p| {
            match target_folder_id {
                Some(fid) => find_folder_dir_in_folders(&p.folders, fid),
                None => Some(p.root_dir.clone()),
            }
        }) else {
            return;
        };

        let Some(source_proj) = self.workspace.find_project_mut(source_project_id) else {
            return;
        };
        let Some(removed) = take_request_from_project(source_proj, request_id) else {
            return;
        };
        if removed.file_path.parent() == Some(target_dir.as_path()) {
            insert_request_into_project(source_proj, None, removed);
            return;
        }

        match move_request(&removed.file_path, &target_dir, &removed.doc) {
            Ok(moved) => {
                let moved_name = moved.doc.name.clone();
                let new_file_path = moved.file_path.clone();
                if let Some(target_proj) = self.workspace.find_project_mut(target_project_id) {
                    insert_request_into_project(target_proj, target_folder_id, moved);
                }
                self.commit_git_change(&format!("Move request: {moved_name}"));
                for tab in &self.tabs {
                    if let WorkbenchTab::Request(view) = tab
                        && view.read(cx).request_id() == request_id
                    {
                        let pid = target_project_id.to_string();
                        let fp = new_file_path.clone();
                        view.update(cx, |v, _| v.set_location(pid, fp));
                    }
                }
                self.sync_sidebars(None, cx);
                self.reveal_and_select_in_explorers(&format!("req:{request_id}"), cx);
                self.sync_all_tabs(window, cx);
                window.push_notification(format!("Moved \"{moved_name}\"."), cx);
                cx.notify();
            }
            Err(err) => {
                // Restore into original project on failure.
                if let Some(source_proj) = self.workspace.find_project_mut(source_project_id) {
                    insert_request_into_project(source_proj, None, removed);
                }
                window.push_notification(format!("Failed moving request: {err}"), cx);
            }
        }
    }

    fn reload_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match load_workspace(&self.workspace_dir) {
            Ok(tree) => {
                self.workspace = tree;
                self.history_entries = load_workspace_history(&self.workspace.root_dir);
                self.request_sets = load_workspace_sets(&self.workspace.root_dir);
                self.sync_sidebars(None, cx);
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

    fn sync_share_settings_to_tabs(&self, cx: &mut Context<Self>) {
        let settings = self.share_settings.clone();
        for tab in &self.tabs {
            if let WorkbenchTab::Container(view) = tab {
                view.update(cx, |v, cx| v.set_share_settings(settings.clone(), cx));
            }
        }
    }

    fn revoke_share(
        &mut self,
        share_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pos) = self.active_shares.iter().position(|s| s.id == share_id) else {
            return;
        };
        let record = self.active_shares.remove(pos);
        self.persist_remote_shares();
        cx.notify();

        match record.channel {
            ShareChannel::LocalNetwork => {
                if let Some(handle) = record.local_handle {
                    handle.stop();
                }
                window.push_notification(
                    format!("Stopped local network share for {}", record.project_name),
                    cx,
                );
                window.refresh();
            }
            ShareChannel::TestrixServer => {
                let settings = self.share_settings.clone();
                let proj_name = record.project_name.clone();
                let rx = dispatch_share_future(revoke_testrix_server_share(
                    settings.share_server_url,
                    settings.share_token,
                    record.project_slug,
                ));
                window.spawn(cx, async move |window| {
                    let outcome = rx.await;
                    _ = window.update(|window, cx| {
                        match outcome {
                            Ok(Ok(())) => window.push_notification(
                                format!("Revoked Testrix Server share for {proj_name}"),
                                cx,
                            ),
                            Ok(Err(err)) => window.push_notification(
                                format!("Failed revoking server share: {err}"),
                                cx,
                            ),
                            Err(_) => window.push_notification("Revoke task cancelled", cx),
                        }
                        window.refresh();
                    });
                })
                .detach();
            }
            ShareChannel::CloudflareR2 => {
                let settings = self.share_settings.clone();
                let proj_name = record.project_name.clone();
                let rx = dispatch_share_future(revoke_cloudflare_r2_share(
                    settings.cloudflare_account_id,
                    settings.cloudflare_r2_bucket,
                    settings.cloudflare_api_token,
                    record.project_slug,
                    record.file_paths,
                ));
                window.spawn(cx, async move |window| {
                    let outcome = rx.await;
                    _ = window.update(|window, cx| {
                        match outcome {
                            Ok(Ok(())) => window.push_notification(
                                format!("Revoked Cloudflare R2 share for {proj_name}"),
                                cx,
                            ),
                            Ok(Err(err)) => window.push_notification(
                                format!("Failed revoking R2 share: {err}"),
                                cx,
                            ),
                            Err(_) => window.push_notification("Revoke task cancelled", cx),
                        }
                        window.refresh();
                    });
                })
                .detach();
            }
        }
    }

    fn open_shares_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let app_entity = cx.entity();

        window.open_dialog(cx, move |dialog, _, cx| {
            let shares = app_entity.read(cx).active_shares.clone();
            let app_for_revoke = app_entity.clone();

            dialog
                .title("Active Shares")
                .width(px(580.))
                .child(
                    v_flex()
                        .gap_3()
                        .max_h(px(460.))
                        .overflow_y_scrollbar()
                        .when(shares.is_empty(), |parent| {
                            parent.child(
                                div()
                                    .py_6()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("No active shares in this session. Open any project's Share tab to publish via Testrix Server, Cloudflare R2, or Local Network."),
                            )
                        })
                        .children(shares.into_iter().map(|share| {
                            let share_id = share.id.clone();
                            let url_copy = share.share_url.clone();
                            let app_revoke = app_for_revoke.clone();
                            v_flex()
                                .gap_1p5()
                                .p_3()
                                .rounded_md()
                                .border_1()
                                .border_color(cx.theme().border)
                                .bg(cx.theme().secondary)
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .items_center()
                                                .child(
                                                    div()
                                                        .text_sm()
                                                        .font_semibold()
                                                        .child(share.project_name.clone()),
                                                )
                                                .child(
                                                    Tag::new()
                                                        .with_variant(TagVariant::Secondary)
                                                        .outline()
                                                        .small()
                                                        .child(share.channel.label()),
                                                ),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_1p5()
                                                .child(
                                                    Button::new(format!("copy-active-share-{}", share.id))
                                                        .xsmall()
                                                        .outline()
                                                        .label("Copy URL")
                                                        .on_click(move |_, window, cx| {
                                                            cx.write_to_clipboard(
                                                                ClipboardItem::new_string(
                                                                    url_copy.clone(),
                                                                ),
                                                            );
                                                            window.push_notification(
                                                                "Share URL copied to clipboard",
                                                                cx,
                                                            );
                                                        }),
                                                )
                                                .child(
                                                    Button::new(format!("revoke-share-{}", share.id))
                                                        .xsmall()
                                                        .danger()
                                                        .label("Revoke")
                                                        .on_click(move |_, window, cx| {
                                                            let id = share_id.clone();
                                                            app_revoke.update(cx, |this, cx| {
                                                                this.revoke_share(id, window, cx);
                                                            });
                                                        }),
                                                ),
                                        ),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .child(share.share_url),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!("Target: {}", share.target_info)),
                                )
                        })),
                )
                .footer(
                    DialogFooter::new().child(
                        DialogClose::new().child(
                            Button::new("shares-done-btn")
                                .primary()
                                .label(crate::i18n::t("done")),
                        ),
                    ),
                )
        });
    }

    fn set_theme_mode(&mut self, mode: ThemeMode, window: &mut Window, cx: &mut Context<Self>) {
        Theme::change(mode, Some(window), cx);
        crate::theme_helpers::configure_syntax_highlighting(cx);
        Theme::global_mut(cx).font_size = self.ui_font_size.pixels();
        let mode_str = if mode.is_dark() { "dark" } else { "light" };
        let _ = save_config_appearance(
            Some(mode_str),
            self.ui_font_size.code(),
            self.editor_font_size,
        );
        cx.notify();
    }

    fn set_ui_font_size(&mut self, size: UiFontSize, cx: &mut Context<Self>) {
        self.ui_font_size = size;
        Theme::global_mut(cx).font_size = size.pixels();
        let mode_str = if cx.theme().is_dark() { "dark" } else { "light" };
        let _ = save_config_appearance(
            Some(mode_str),
            self.ui_font_size.code(),
            self.editor_font_size,
        );
        cx.notify();
    }

    fn set_editor_font_size(&mut self, size: u16, cx: &mut Context<Self>) {
        let clamped = size.clamp(10, 24);
        self.editor_font_size = clamped;
        for tab in &self.tabs {
            if let WorkbenchTab::Request(view) = tab {
                view.update(cx, |v, cx| v.set_editor_font_size(f32::from(clamped), cx));
            }
        }
        let mode_str = if cx.theme().is_dark() { "dark" } else { "light" };
        let _ = save_config_appearance(
            Some(mode_str),
            self.ui_font_size.code(),
            self.editor_font_size,
        );
        cx.notify();
    }

    fn open_preferences_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let app_entity = cx.entity();
        self.pref_keymap_error = None;
        let current_share = self.share_settings.clone();

        let keymap_inputs: Vec<(&'static str, Entity<InputState>)> = CUSTOMIZABLE_SHORTCUTS
            .iter()
            .map(|spec| {
                let current = effective_shortcut(&self.custom_key_bindings, spec);
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(spec.default_shortcut)
                        .default_value(current)
                });
                (spec.id, input)
            })
            .collect();

        let server_url_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://testrix.bitifyware.com")
                .default_value(current_share.share_server_url.clone())
        });
        let email_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("you@example.com")
                .default_value(current_share.share_email.clone())
        });
        let password_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Password")
                .masked(true)
        });

        let cf_account_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Account ID (32-char hex, NOT email)")
                .default_value(current_share.cloudflare_account_id.clone())
        });
        let cf_bucket_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("testrix-share")
                .default_value(current_share.cloudflare_r2_bucket.clone())
        });
        let cf_token_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Cloudflare API Token (R2 Write)")
                .masked(true)
                .default_value(current_share.cloudflare_api_token.clone())
        });
        let cf_pages_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("https://your-site.pages.dev (optional)")
                .default_value(current_share.cloudflare_pages_url.clone())
        });
        let access_token_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Access Token (optional, limits access to shared site / Cloudflare page)")
                .default_value(current_share.share_access_token.clone())
        });

        window.open_dialog(cx, move |dialog, _, cx| {
            let (
                remember_tabs,
                remember_responses,
                language_select,
                close_behavior_select,
                share_state,
                pref_section,
                ui_font_size,
                editor_font_size,
                custom_key_bindings,
                keymap_error,
            ) = {
                let app = app_entity.read(cx);
                (
                    app.remember_open_tabs,
                    app.remember_responses,
                    app.language_select.clone(),
                    app.close_behavior_select.clone(),
                    app.share_settings.clone(),
                    app.pref_section,
                    app.ui_font_size,
                    app.editor_font_size,
                    app.custom_key_bindings.clone(),
                    app.pref_keymap_error.clone(),
                )
            };
            let is_logged_in = !share_state.share_token.trim().is_empty();
            let is_dark = cx.theme().is_dark();

            let app_tabs = app_entity.clone();
            let app_resps = app_entity.clone();
            let app_login = app_entity.clone();
            let app_logout = app_entity.clone();
            let app_cf = app_entity.clone();
            let app_access = app_entity.clone();
            let app_theme_light = app_entity.clone();
            let app_theme_dark = app_entity.clone();
            let app_ui_small = app_entity.clone();
            let app_ui_large = app_entity.clone();
            let app_ed_dec = app_entity.clone();
            let app_ed_inc = app_entity.clone();
            let app_ed_reset = app_entity.clone();
            let app_km_reset_all = app_entity.clone();

            let srv_for_login = server_url_input.clone();
            let email_for_login = email_input.clone();
            let pwd_for_login = password_input.clone();

            let cf_acc_save = cf_account_input.clone();
            let cf_bkt_save = cf_bucket_input.clone();
            let cf_tok_save = cf_token_input.clone();
            let cf_url_save = cf_pages_input.clone();
            let access_tok_for_cf = access_token_input.clone();
            let access_tok_for_save = access_token_input.clone();

            let nav_item = |id: &'static str,
                            label: &'static str,
                            section: PreferencesSection,
                            app_handle: Entity<Self>| {
                let active = pref_section == section;
                Button::new(id)
                    .w_full()
                    .justify_start()
                    .small()
                    .when(active, |b| b.primary())
                    .when(!active, |b| b.ghost())
                    .label(label)
                    .on_click(move |_, window, cx| {
                        app_handle.update(cx, |this, cx| {
                            this.pref_section = section;
                            cx.notify();
                        });
                        window.refresh();
                    })
            };

            let keymap_inputs_for_reset = keymap_inputs.clone();

            dialog
                .title(crate::i18n::t("preferences"))
                .width(px(700.))
                .child(
                    h_flex()
                        .w_full()
                        .h(px(460.))
                        .items_stretch()
                        .gap_3()
                        .child(
                            v_flex()
                                .w(px(164.))
                                .flex_none()
                                .border_r_1()
                                .border_color(cx.theme().border)
                                .pr_3()
                                .gap_1()
                                .child(nav_item(
                                    "pref-nav-general",
                                    "General",
                                    PreferencesSection::General,
                                    app_entity.clone(),
                                ))
                                .child(nav_item(
                                    "pref-nav-appearance",
                                    "Appearance",
                                    PreferencesSection::Appearance,
                                    app_entity.clone(),
                                ))
                                .child(nav_item(
                                    "pref-nav-keybindings",
                                    "Keybindings",
                                    PreferencesSection::Keybindings,
                                    app_entity.clone(),
                                ))
                                .child(nav_item(
                                    "pref-nav-share",
                                    "Share & Sync",
                                    PreferencesSection::ShareSync,
                                    app_entity.clone(),
                                )),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .pr_1()
                                .overflow_y_scrollbar()
                                .gap_3()
                                .when(pref_section == PreferencesSection::General, |pane| {
                                    pane.child(
                            h_flex()
                                .w_full()
                                .justify_between()
                                .items_center()
                                .gap_3()
                                .child(
                                    v_flex()
                                        .gap_0p5()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_medium()
                                                .child(crate::i18n::t("language")),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(crate::i18n::t("language_desc")),
                                        ),
                                )
                                .child(
                                    div().w(px(172.)).flex_none().child(
                                        Select::new(&language_select)
                                            .id("pref-language-select")
                                            .small(),
                                    ),
                                ),
                        )
                        .child(
                            h_flex()
                                .w_full()
                                .justify_between()
                                .items_center()
                                .gap_3()
                                .child(
                                    v_flex()
                                        .gap_0p5()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_medium()
                                                .child(crate::i18n::t("remember_tabs")),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(crate::i18n::t("remember_tabs_desc")),
                                        ),
                                )
                                .child(
                                    Switch::new("pref-remember-tabs")
                                        .checked(remember_tabs)
                                        .on_change(move |next, window, cx| {
                                            app_tabs.update(cx, |this, cx| {
                                                this.remember_open_tabs = *next;
                                                let _ = save_config_preferences(
                                                    this.remember_open_tabs,
                                                    this.remember_responses,
                                                    crate::i18n::current_language().code(),
                                                    this.close_behavior.as_deref(),
                                                );
                                                if *next {
                                                    this.persist_open_tabs(cx);
                                                } else {
                                                    let _ = save_workspace_tabs(
                                                        &this.workspace.root_dir,
                                                        &[],
                                                        None,
                                                    );
                                                }
                                                cx.notify();
                                            });
                                            window.refresh();
                                        }),
                                ),
                        )
                        .child(
                            h_flex()
                                .w_full()
                                .justify_between()
                                .items_center()
                                .gap_3()
                                .child(
                                    v_flex()
                                        .gap_0p5()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_medium()
                                                .child(crate::i18n::t("remember_responses")),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(crate::i18n::t("remember_responses_desc")),
                                        ),
                                )
                                .child(
                                    Switch::new("pref-remember-responses")
                                        .checked(remember_responses)
                                        .on_change(move |next, window, cx| {
                                            app_resps.update(cx, |this, cx| {
                                                this.remember_responses = *next;
                                                let _ = save_config_preferences(
                                                    this.remember_open_tabs,
                                                    this.remember_responses,
                                                    crate::i18n::current_language().code(),
                                                    this.close_behavior.as_deref(),
                                                );
                                                if !*next {
                                                    let _ = clear_saved_responses(
                                                        &this.workspace.root_dir,
                                                    );
                                                }
                                                cx.notify();
                                            });
                                            window.refresh();
                                        }),
                                ),
                        )
                        .child(
                            h_flex()
                                .w_full()
                                .justify_between()
                                .items_center()
                                .gap_3()
                                .child(
                                    v_flex()
                                        .gap_0p5()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_medium()
                                                .child(crate::i18n::t("close_behavior")),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(crate::i18n::t("close_behavior_desc")),
                                        ),
                                )
                                .child(
                                    div().w(px(196.)).flex_none().child(
                                        Select::new(&close_behavior_select)
                                            .id("pref-close-behavior-select")
                                            .small(),
                                    ),
                                ),
                        )
                                })
                                .when(pref_section == PreferencesSection::Appearance, |pane| {
                                    pane.child(
                                        h_flex()
                                            .w_full()
                                            .justify_between()
                                            .items_center()
                                            .gap_3()
                                            .child(
                                                v_flex()
                                                    .gap_0p5()
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .font_medium()
                                                            .child("Theme"),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child("Switch between Light and Dark appearance"),
                                                    ),
                                            )
                                            .child(
                                                h_flex()
                                                    .gap_1()
                                                    .child(
                                                        Button::new("pref-theme-light")
                                                            .small()
                                                            .icon(IconName::Sun)
                                                            .label("Light")
                                                            .when(!is_dark, |b| b.primary())
                                                            .when(is_dark, |b| b.outline())
                                                            .on_click(move |_, window, cx| {
                                                                app_theme_light.update(cx, |this, cx| {
                                                                    this.set_theme_mode(ThemeMode::Light, window, cx);
                                                                });
                                                                window.refresh();
                                                            }),
                                                    )
                                                    .child(
                                                        Button::new("pref-theme-dark")
                                                            .small()
                                                            .icon(IconName::Moon)
                                                            .label("Dark")
                                                            .when(is_dark, |b| b.primary())
                                                            .when(!is_dark, |b| b.outline())
                                                            .on_click(move |_, window, cx| {
                                                                app_theme_dark.update(cx, |this, cx| {
                                                                    this.set_theme_mode(ThemeMode::Dark, window, cx);
                                                                });
                                                                window.refresh();
                                                            }),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .w_full()
                                            .justify_between()
                                            .items_center()
                                            .gap_3()
                                            .child(
                                                v_flex()
                                                    .gap_0p5()
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .font_medium()
                                                            .child("UI Font Size"),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child("Controls overall interface text size (default: Small)"),
                                                    ),
                                            )
                                            .child(
                                                h_flex()
                                                    .gap_1()
                                                    .child(
                                                        Button::new("pref-ui-font-small")
                                                            .small()
                                                            .label("Small")
                                                            .when(ui_font_size == UiFontSize::Small, |b| b.primary())
                                                            .when(ui_font_size != UiFontSize::Small, |b| b.outline())
                                                            .on_click(move |_, window, cx| {
                                                                app_ui_small.update(cx, |this, cx| {
                                                                    this.set_ui_font_size(UiFontSize::Small, cx);
                                                                });
                                                                window.refresh();
                                                            }),
                                                    )
                                                    .child(
                                                        Button::new("pref-ui-font-large")
                                                            .small()
                                                            .label("Large")
                                                            .when(ui_font_size == UiFontSize::Large, |b| b.primary())
                                                            .when(ui_font_size != UiFontSize::Large, |b| b.outline())
                                                            .on_click(move |_, window, cx| {
                                                                app_ui_large.update(cx, |this, cx| {
                                                                    this.set_ui_font_size(UiFontSize::Large, cx);
                                                                });
                                                                window.refresh();
                                                            }),
                                                    ),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .w_full()
                                            .justify_between()
                                            .items_center()
                                            .gap_3()
                                            .child(
                                                v_flex()
                                                    .gap_0p5()
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .font_medium()
                                                            .child("Editor Font Size"),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child("Request Body, Response Body, and Post-Script"),
                                                    ),
                                            )
                                            .child(
                                                h_flex()
                                                    .gap_1()
                                                    .items_center()
                                                    .child(
                                                        Button::new("pref-editor-font-dec")
                                                            .small()
                                                            .outline()
                                                            .label("-")
                                                            .disabled(editor_font_size <= 10)
                                                            .on_click(move |_, window, cx| {
                                                                app_ed_dec.update(cx, |this, cx| {
                                                                    this.set_editor_font_size(this.editor_font_size.saturating_sub(1), cx);
                                                                });
                                                                window.refresh();
                                                            }),
                                                    )
                                                    .child(
                                                        div()
                                                            .w(px(52.))
                                                            .text_center()
                                                            .text_sm()
                                                            .font_medium()
                                                            .child(format!("{editor_font_size} px")),
                                                    )
                                                    .child(
                                                        Button::new("pref-editor-font-inc")
                                                            .small()
                                                            .outline()
                                                            .label("+")
                                                            .disabled(editor_font_size >= 24)
                                                            .on_click(move |_, window, cx| {
                                                                app_ed_inc.update(cx, |this, cx| {
                                                                    this.set_editor_font_size(this.editor_font_size.saturating_add(1), cx);
                                                                });
                                                                window.refresh();
                                                            }),
                                                    )
                                                    .child(
                                                        Button::new("pref-editor-font-reset")
                                                            .small()
                                                            .ghost()
                                                            .label("Reset")
                                                            .disabled(editor_font_size == 13)
                                                            .on_click(move |_, window, cx| {
                                                                app_ed_reset.update(cx, |this, cx| {
                                                                    this.set_editor_font_size(13, cx);
                                                                });
                                                                window.refresh();
                                                            }),
                                                    ),
                                            ),
                                    )
                                })
                                .when(pref_section == PreferencesSection::Keybindings, |pane| {
                                    pane.child(
                                        h_flex()
                                            .w_full()
                                            .justify_between()
                                            .items_center()
                                            .child(
                                                v_flex()
                                                    .gap_0p5()
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .font_semibold()
                                                            .child("Keyboard Shortcuts"),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child("Use modifiers like cmd-s, ctrl-shift-f, alt-enter"),
                                                    ),
                                            )
                                            .child(
                                                Button::new("pref-keymap-reset-all")
                                                    .small()
                                                    .outline()
                                                    .label("Reset All")
                                                    .disabled(custom_key_bindings.is_empty())
                                                    .on_click(move |_, window, cx| {
                                                        for (id, input) in &keymap_inputs_for_reset {
                                                            if let Some(spec) = CUSTOMIZABLE_SHORTCUTS.iter().find(|s| s.id == *id) {
                                                                input.update(cx, |st, cx| {
                                                                    st.set_value(spec.default_shortcut, window, cx);
                                                                });
                                                            }
                                                        }
                                                        app_km_reset_all.update(cx, |this, cx| {
                                                            this.custom_key_bindings.clear();
                                                            this.pref_keymap_error = None;
                                                            apply_keybindings(&this.custom_key_bindings, cx);
                                                            let _ = save_config_key_bindings(&this.custom_key_bindings);
                                                            cx.notify();
                                                        });
                                                        window.refresh();
                                                    }),
                                            ),
                                    )
                                    .when_some(keymap_error, |pane, err| {
                                        pane.child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().danger)
                                                .child(err),
                                        )
                                    })
                                    .children(CUSTOMIZABLE_SHORTCUTS.iter().enumerate().map(|(ix, spec)| {
                                        let input = keymap_inputs[ix].1.clone();
                                        let input_apply = input.clone();
                                        let input_reset = input.clone();
                                        let app_apply = app_entity.clone();
                                        let app_reset = app_entity.clone();
                                        let is_custom = custom_key_bindings.contains_key(spec.id);
                                        let action_id = spec.id;
                                        let default_sc = spec.default_shortcut;

                                        h_flex()
                                            .w_full()
                                            .justify_between()
                                            .items_center()
                                            .gap_2()
                                            .py_1()
                                            .border_b_1()
                                            .border_color(cx.theme().border)
                                            .child(
                                                v_flex()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .gap_0p5()
                                                    .child(
                                                        div()
                                                            .text_sm()
                                                            .font_medium()
                                                            .child(spec.label),
                                                    )
                                                    .child(
                                                        div()
                                                            .text_xs()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child(spec.description),
                                                    ),
                                            )
                                            .child(
                                                h_flex()
                                                    .gap_1()
                                                    .items_center()
                                                    .flex_none()
                                                    .child(
                                                        div()
                                                            .w(px(124.))
                                                            .child(Input::new(&input).small()),
                                                    )
                                                    .child(
                                                        Button::new(SharedString::from(format!(
                                                            "pref-keymap-apply-{action_id}"
                                                        )))
                                                        .small()
                                                        .outline()
                                                        .label("Apply")
                                                        .on_click(move |_, window, cx| {
                                                            let raw = input_apply.read(cx).value().to_string();
                                                            match validate_shortcut(&raw) {
                                                                Ok(valid) => {
                                                                    app_apply.update(cx, |this, cx| {
                                                                        if valid == default_sc {
                                                                            this.custom_key_bindings.remove(action_id);
                                                                        } else {
                                                                            this.custom_key_bindings.insert(action_id.to_string(), valid);
                                                                        }
                                                                        this.pref_keymap_error = None;
                                                                        apply_keybindings(&this.custom_key_bindings, cx);
                                                                        let _ = save_config_key_bindings(&this.custom_key_bindings);
                                                                        cx.notify();
                                                                    });
                                                                }
                                                                Err(err) => {
                                                                    app_apply.update(cx, |this, cx| {
                                                                        this.pref_keymap_error = Some(err);
                                                                        cx.notify();
                                                                    });
                                                                }
                                                            }
                                                            window.refresh();
                                                        }),
                                                    )
                                                    .child(
                                                        Button::new(SharedString::from(format!(
                                                            "pref-keymap-reset-{action_id}"
                                                        )))
                                                        .small()
                                                        .ghost()
                                                        .label("Reset")
                                                        .disabled(!is_custom)
                                                        .on_click(move |_, window, cx| {
                                                            input_reset.update(cx, |st, cx| {
                                                                st.set_value(default_sc, window, cx);
                                                            });
                                                            app_reset.update(cx, |this, cx| {
                                                                this.custom_key_bindings.remove(action_id);
                                                                this.pref_keymap_error = None;
                                                                apply_keybindings(&this.custom_key_bindings, cx);
                                                                let _ = save_config_key_bindings(&this.custom_key_bindings);
                                                                cx.notify();
                                                            });
                                                            window.refresh();
                                                        }),
                                                    ),
                                            )
                                    }))
                                })
                                .when(pref_section == PreferencesSection::ShareSync, |pane| {
                                    pane.child(
                            v_flex()
                                .gap_2()
                                .child(
                                    h_flex()
                                        .justify_between()
                                        .items_center()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_semibold()
                                                .child("Testrix Share Server Account"),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(if is_logged_in {
                                                    format!("Signed in as {}", share_state.share_email)
                                                } else {
                                                    "Not signed in".to_string()
                                                }),
                                        ),
                                )
                                .child(
                                    v_flex()
                                        .gap_1p5()
                                        .child(Input::new(&server_url_input).small())
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .child(div().flex_1().child(Input::new(&email_input).small()))
                                                .child(div().flex_1().child(Input::new(&password_input).small())),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(
                                            Button::new("pref-share-login-btn")
                                                .small()
                                                .primary()
                                                .label(if is_logged_in { "Re-login" } else { "Login" })
                                                .on_click(move |_, window, cx| {
                                                    let server_url = srv_for_login.read(cx).value().trim().to_string();
                                                    let email = email_for_login.read(cx).value().trim().to_string();
                                                    let password = pwd_for_login.read(cx).value().to_string();
                                                    let app_handle = app_login.clone();
                                                    let rx = dispatch_share_future(login_testrix_server(
                                                        server_url.clone(),
                                                        email,
                                                        password,
                                                    ));
                                                    window.spawn(cx, async move |window| {
                                                        let outcome = rx.await;
                                                        _ = window.update(|window, cx| {
                                                            match outcome {
                                                                Ok(Ok(login)) => {
                                                                    app_handle.update(cx, |this, cx| {
                                                                        this.share_settings.share_server_url = server_url;
                                                                        this.share_settings.share_email = login.email.clone();
                                                                        this.share_settings.share_token = login.token;
                                                                        let _ = save_config_share_settings(&this.share_settings);
                                                                        this.sync_share_settings_to_tabs(cx);
                                                                        this.maybe_auto_git_sync(window, cx);
                                                                        cx.notify();
                                                                    });
                                                                    window.push_notification(
                                                                        format!("Signed in to Share Server as {}", login.email),
                                                                        cx,
                                                                    );
                                                                    window.refresh();
                                                                }
                                                                Ok(Err(err)) => {
                                                                    window.push_notification(format!("Login failed: {err}"), cx);
                                                                }
                                                                Err(_) => {
                                                                    window.push_notification("Login task cancelled", cx);
                                                                }
                                                            }
                                                        });
                                                    })
                                                    .detach();
                                                }),
                                        )
                                        .when(is_logged_in, |row| {
                                            row.child(
                                                Button::new("pref-share-logout-btn")
                                                    .small()
                                                    .outline()
                                                    .label("Logout")
                                                    .on_click(move |_, window, cx| {
                                                        app_logout.update(cx, |this, cx| {
                                                            this.confirm_logout_with_cloud_cleanup(window, cx);
                                                        });
                                                    }),
                                            )
                                        }),
                                ),
                        )
                        .child(
                            v_flex()
                                .gap_2()
                                .pt_3()
                                .border_t_1()
                                .border_color(cx.theme().border)
                                .child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .child("Cloudflare R2 Storage"),
                                )
                                .child(
                                    v_flex()
                                        .gap_1p5()
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .child(div().flex_1().child(Input::new(&cf_account_input).small()))
                                                .child(div().flex_1().child(Input::new(&cf_bucket_input).small())),
                                        )
                                        .child(Input::new(&cf_token_input).small())
                                        .child(Input::new(&cf_pages_input).small()),
                                )
                                .child(
                                    h_flex().child(
                                        Button::new("pref-cf-save-btn")
                                            .small()
                                            .outline()
                                            .label("Save Cloudflare Settings")
                                            .on_click(move |_, window, cx| {
                                                let acc = cf_acc_save.read(cx).value().trim().to_string();
                                                let bkt = cf_bkt_save.read(cx).value().trim().to_string();
                                                let tok = cf_tok_save.read(cx).value().trim().to_string();
                                                let url = cf_url_save.read(cx).value().trim().to_string();
                                                let access_tok = access_tok_for_cf.read(cx).value().trim().to_string();
                                                app_cf.update(cx, |this, cx| {
                                                    this.share_settings.cloudflare_account_id = acc;
                                                    this.share_settings.cloudflare_r2_bucket = bkt;
                                                    this.share_settings.cloudflare_api_token = tok;
                                                    this.share_settings.cloudflare_pages_url = url;
                                                    this.share_settings.share_access_token = access_tok;
                                                    let _ = save_config_share_settings(&this.share_settings);
                                                    this.sync_share_settings_to_tabs(cx);
                                                    cx.notify();
                                                });
                                                window.push_notification("Saved Cloudflare R2 settings", cx);
                                                window.refresh();
                                            }),
                                    ),
                                ),
                        )
                        .child(
                            v_flex()
                                .gap_2()
                                .pt_3()
                                .border_t_1()
                                .border_color(cx.theme().border)
                                .child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .child("Share Access Control"),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Optional access token required to view your shared project on Testrix Server or Cloudflare Pages."),
                                )
                                .child(Input::new(&access_token_input).small())
                                .child(
                                    h_flex().child(
                                        Button::new("pref-access-save-btn")
                                            .small()
                                            .outline()
                                            .label("Save Access Token")
                                            .on_click(move |_, window, cx| {
                                                let access_tok = access_tok_for_save.read(cx).value().trim().to_string();
                                                app_access.update(cx, |this, cx| {
                                                    this.share_settings.share_access_token = access_tok;
                                                    let _ = save_config_share_settings(&this.share_settings);
                                                    this.sync_share_settings_to_tabs(cx);
                                                    let remote_project_ids: Vec<String> = this
                                                        .active_shares
                                                        .iter()
                                                        .filter(|s| s.channel != ShareChannel::LocalNetwork)
                                                        .map(|s| s.project_id.clone())
                                                        .collect::<std::collections::HashSet<_>>()
                                                        .into_iter()
                                                        .collect();
                                                    for project_id in remote_project_ids {
                                                        this.sync_shared_project_after_save(&project_id, None, window, cx);
                                                    }
                                                    cx.notify();
                                                });
                                                window.push_notification("Saved Share Access Token", cx);
                                                window.refresh();
                                            }),
                                    ),
                                ),
                        )
                                }),
                        ),
                )
                .footer(
                    DialogFooter::new().child(
                        DialogClose::new().child(
                            Button::new("pref-done-btn")
                                .primary()
                                .label(crate::i18n::t("done")),
                        ),
                    ),
                )
        });
    }

    fn open_about_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let info = current_build_info();
        let app_entity = cx.entity();

        window.open_dialog(cx, move |dialog, _, cx| {
            let row = |label: &'static str, value: String| {
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(label),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_medium()
                            .child(value),
                    )
            };

            dialog
                .title(crate::i18n::t("about"))
                .width(px(460.))
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            h_flex()
                                .gap_3()
                                .items_center()
                                .child(
                                    img(ImageSource::Image(app_titlebar_icon()))
                                        .size(px(48.))
                                        .rounded(px(10.)),
                                )
                                .child(
                                    v_flex()
                                        .gap_0p5()
                                        .child(div().text_base().font_bold().child("Testrix"))
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("YAML-First Desktop API Workbench"),
                                        ),
                                ),
                        )
                        .child(
                            v_flex()
                                .gap_2()
                                .p_3()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(cx.theme().border)
                                .child(row(crate::i18n::t("about_version"), info.version.clone()))
                                .child(row(
                                    crate::i18n::t("about_build_time"),
                                    info.build_time.clone(),
                                ))
                                .child(row(crate::i18n::t("about_branch"), info.branch.clone()))
                                .child(row(crate::i18n::t("about_commit"), info.commit.clone()))
                                .child(
                                    h_flex()
                                        .w_full()
                                        .justify_between()
                                        .items_center()
                                        .gap_3()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(crate::i18n::t("about_email")),
                                        )
                                        .child(
                                            Button::new("about-email-btn")
                                                .link()
                                                .xsmall()
                                                .label("support@bitifyware.com")
                                                .on_click(|_, _, cx| {
                                                    cx.open_url("mailto:support@bitifyware.com");
                                                }),
                                        ),
                                )
                                .child(
                                    h_flex()
                                        .w_full()
                                        .justify_between()
                                        .items_center()
                                        .gap_3()
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(cx.theme().muted_foreground)
                                                .child(crate::i18n::t("about_website")),
                                        )
                                        .child(
                                            Button::new("about-website-btn")
                                                .link()
                                                .xsmall()
                                                .label("https://bitifyware.com")
                                                .on_click(|_, _, cx| {
                                                    cx.open_url("https://bitifyware.com");
                                                }),
                                        ),
                                ),
                        ),
                )
                .footer({
                    let app_check = app_entity.clone();
                    let checking = matches!(
                        app_entity.read(cx).upgrade_status,
                        UpgradeStatus::Checking
                    );
                    DialogFooter::new()
                        .gap_2()
                        .child(
                            Button::new("about-check-updates-btn")
                                .outline()
                                .disabled(checking)
                                .label(if checking {
                                    crate::i18n::t("upgrade_checking")
                                } else {
                                    crate::i18n::t("upgrade_check_btn")
                                })
                                .on_click(move |_, window, cx| {
                                    app_check.update(cx, |this, cx| {
                                        this.check_for_updates(true, window, cx);
                                    });
                                }),
                        )
                        .child(
                            DialogClose::new().child(
                                Button::new("about-done-btn")
                                    .primary()
                                    .label(crate::i18n::t("done")),
                            ),
                        )
                })
        });
    }

    fn check_for_updates(
        &mut self,
        interactive: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(
            self.upgrade_status,
            UpgradeStatus::Checking | UpgradeStatus::Downloading { .. }
        ) {
            return;
        }
        let current_version = current_build_info().version;
        self.upgrade_status = UpgradeStatus::Checking;
        cx.notify();

        let rx = dispatch_share_future(async move { check_for_upgrade(&current_version).await });
        cx.spawn_in(window, async move |weak_self, cx| {
            let outcome = rx.await;
            _ = weak_self.update_in(cx, |this, window, cx| {
                match outcome {
                    Ok(Ok(res)) if res.has_upgrade => {
                        this.upgrade_status = UpgradeStatus::Available(res.latest);
                        this.open_upgrade_dialog(window, cx);
                    }
                    Ok(Ok(res)) => {
                        this.upgrade_status = UpgradeStatus::UpToDate;
                        if interactive {
                            window.push_notification(
                                format!(
                                    "{} ({})",
                                    crate::i18n::t("upgrade_up_to_date"),
                                    res.current_version
                                ),
                                cx,
                            );
                        }
                    }
                    Ok(Err(err)) => {
                        if interactive {
                            this.upgrade_status = UpgradeStatus::Error {
                                release: None,
                                message: err.clone(),
                            };
                            window.push_notification(
                                format!("{}: {err}", crate::i18n::t("upgrade_check_failed")),
                                cx,
                            );
                        } else {
                            this.upgrade_status = UpgradeStatus::Idle;
                            tracing::debug!(error = %err, "background upgrade check failed");
                        }
                    }
                    Err(_) => {
                        this.upgrade_status = UpgradeStatus::Idle;
                    }
                }
                cx.notify();
                window.refresh();
            });
        })
        .detach();
    }

    fn start_upgrade_download(
        &mut self,
        release: ReleaseInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(asset) = release.asset.clone() else {
            cx.open_url(&release.html_url);
            return;
        };

        let progress = Arc::new(DownloadProgressState::new());
        self.upgrade_status = UpgradeStatus::Downloading {
            release: release.clone(),
            downloaded: 0,
            total: (asset.size > 0).then_some(asset.size),
        };
        cx.notify();
        window.refresh();

        let progress_worker = progress.clone();
        let url = asset.download_url;
        let file_name = asset.name;
        let mut rx =
            dispatch_share_future(async move { download_release_asset(&url, &file_name, progress_worker).await });

        cx.spawn_in(window, async move |weak_self, cx| {
            let outcome = loop {
                match rx.try_recv() {
                    Ok(res) => break Ok(res),
                    Err(OneshotTryRecvError::Empty) => {
                        let (downloaded, total) = progress.snapshot();
                        let updated = weak_self
                            .update_in(cx, |this, window, cx| {
                                if let UpgradeStatus::Downloading {
                                    downloaded: cur_dl,
                                    total: cur_tot,
                                    ..
                                } = &mut this.upgrade_status
                                {
                                    *cur_dl = downloaded;
                                    if total.is_some() {
                                        *cur_tot = total;
                                    }
                                    cx.notify();
                                    window.refresh();
                                }
                            })
                            .is_ok();
                        if !updated {
                            return;
                        }
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(100))
                            .await;
                    }
                    Err(OneshotTryRecvError::Closed) => {
                        break Err("Download task closed unexpectedly".to_string());
                    }
                }
            };

            _ = weak_self.update_in(cx, |this, window, cx| {
                match outcome {
                    Ok(Ok(installer_path)) => {
                        this.launch_downloaded_installer_and_quit(
                            release,
                            &installer_path,
                            window,
                            cx,
                        );
                    }
                    Ok(Err(err)) | Err(err) => {
                        this.upgrade_status = UpgradeStatus::Error {
                            release: Some(release),
                            message: err,
                        };
                        cx.notify();
                        window.refresh();
                    }
                }
            });
        })
        .detach();
    }

    fn launch_downloaded_installer_and_quit(
        &mut self,
        release: ReleaseInfo,
        installer_path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.upgrade_status = UpgradeStatus::ReadyToInstall {
            release: release.clone(),
            installer_path: installer_path.to_path_buf(),
        };
        match launch_installer(installer_path) {
            Ok(()) => {
                self.force_quit = true;
                cx.quit();
            }
            Err(err) => {
                self.upgrade_status = UpgradeStatus::Error {
                    release: Some(release),
                    message: err,
                };
                cx.notify();
                window.refresh();
            }
        }
    }

    fn open_upgrade_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let app_entity = cx.entity();
        let current_version = current_build_info().version;

        window.open_dialog(cx, move |dialog, _, cx| {
            let status = app_entity.read(cx).upgrade_status.clone();
            let app_download = app_entity.clone();
            let app_install = app_entity.clone();
            let app_retry = app_entity.clone();

            let Some(release) = status.active_release().cloned() else {
                return dialog
                    .title(crate::i18n::t("upgrade_available_title"))
                    .width(px(480.))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(crate::i18n::t("upgrade_up_to_date")),
                    )
                    .footer(
                        DialogFooter::new().child(
                            DialogClose::new().child(
                                Button::new("upgrade-close-empty")
                                    .primary()
                                    .label(crate::i18n::t("done")),
                            ),
                        ),
                    );
            };

            let notes = release.body.trim().to_string();
            let html_url = release.html_url.clone();

            let mut body = v_flex()
                .gap_3()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(crate::i18n::t("upgrade_available_desc")),
                )
                .child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .items_center()
                        .p_3()
                        .rounded(cx.theme().radius)
                        .border_1()
                        .border_color(cx.theme().border)
                        .child(
                            v_flex()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(crate::i18n::t("upgrade_current_version")),
                                )
                                .child(div().text_sm().font_medium().child(current_version.clone())),
                        )
                        .child(Icon::new(LucideIcon::ArrowRight).size_4())
                        .child(
                            v_flex()
                                .gap_0p5()
                                .items_end()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(crate::i18n::t("upgrade_latest_version")),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_bold()
                                        .text_color(cx.theme().primary)
                                        .child(release.tag_name.clone()),
                                ),
                        ),
                );

            if !notes.is_empty() {
                body = body.child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .font_medium()
                                .text_color(cx.theme().muted_foreground)
                                .child(crate::i18n::t("upgrade_release_notes")),
                        )
                        .child(
                            div()
                                .id("upgrade-release-notes")
                                .max_h(px(180.))
                                .overflow_y_scrollbar()
                                .p_2p5()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(cx.theme().border)
                                .text_xs()
                                .child(notes),
                        ),
                );
            }

            if let Some(asset) = &release.asset {
                let size_str = if asset.size > 0 {
                    format!(" ({})", format_bytes(asset.size))
                } else {
                    String::new()
                };
                body = body.child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .items_center()
                        .text_xs()
                        .child(
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .child(crate::i18n::t("upgrade_package")),
                        )
                        .child(
                            div()
                                .font_medium()
                                .child(format!("{}{}", asset.name, size_str)),
                        ),
                );
            } else {
                body = body.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(crate::i18n::t("upgrade_no_asset")),
                );
            }

            match &status {
                UpgradeStatus::Downloading {
                    downloaded, total, ..
                } => {
                    let fraction = total
                        .filter(|&t| t > 0)
                        .map(|t| (*downloaded as f32 / t as f32).clamp(0.0, 1.0))
                        .unwrap_or(0.0);
                    let pct = (fraction * 100.0).round() as u64;
                    let progress_label = match total {
                        Some(t) if *t > 0 => format!(
                            "{} {}% ({} / {})",
                            crate::i18n::t("upgrade_downloading"),
                            pct,
                            format_bytes(*downloaded),
                            format_bytes(*t)
                        ),
                        _ => format!(
                            "{} ({})",
                            crate::i18n::t("upgrade_downloading"),
                            format_bytes(*downloaded)
                        ),
                    };
                    body = body.child(
                        v_flex()
                            .gap_1p5()
                            .child(div().text_xs().font_medium().child(progress_label))
                            .child(
                                div()
                                    .w(px(430.))
                                    .h(px(6.))
                                    .rounded_full()
                                    .bg(cx.theme().secondary)
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .h_full()
                                            .w(px(430.0 * fraction))
                                            .rounded_full()
                                            .bg(cx.theme().primary),
                                    ),
                            ),
                    );
                }
                UpgradeStatus::ReadyToInstall { .. } => {
                    body = body.child(
                        div()
                            .text_xs()
                            .font_medium()
                            .text_color(cx.theme().primary)
                            .child(crate::i18n::t("upgrade_ready_launching")),
                    );
                }
                UpgradeStatus::Error { message, .. } => {
                    body = body.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(message.clone()),
                    );
                }
                _ => {}
            }

            let is_downloading = matches!(status, UpgradeStatus::Downloading { .. });
            let mut footer = DialogFooter::new().gap_2().child(
                DialogClose::new().child(
                    Button::new("upgrade-later-btn")
                        .outline()
                        .disabled(is_downloading)
                        .label(crate::i18n::t("upgrade_later")),
                ),
            );

            match &status {
                UpgradeStatus::Downloading { .. } => {
                    footer = footer.child(
                        Button::new("upgrade-downloading-btn")
                            .primary()
                            .disabled(true)
                            .label(crate::i18n::t("upgrade_downloading")),
                    );
                }
                UpgradeStatus::ReadyToInstall {
                    release,
                    installer_path,
                } => {
                    let rel = release.clone();
                    let path = installer_path.clone();
                    footer = footer.child(
                        Button::new("upgrade-install-btn")
                            .primary()
                            .icon(LucideIcon::Rocket)
                            .label(crate::i18n::t("upgrade_install_and_quit"))
                            .on_click(move |_, window, cx| {
                                app_install.update(cx, |this, cx| {
                                    this.launch_downloaded_installer_and_quit(
                                        rel.clone(),
                                        &path,
                                        window,
                                        cx,
                                    );
                                });
                            }),
                    );
                }
                UpgradeStatus::Error { .. } => {
                    let rel = release;
                    let url_open = html_url;
                    footer = footer
                        .child(
                            Button::new("upgrade-open-releases-btn")
                                .outline()
                                .label(crate::i18n::t("upgrade_open_releases"))
                                .on_click(move |_, _, cx| {
                                    cx.open_url(&url_open);
                                }),
                        )
                        .child(
                            Button::new("upgrade-retry-btn")
                                .primary()
                                .label(crate::i18n::t("upgrade_retry"))
                                .on_click(move |_, window, cx| {
                                    app_retry.update(cx, |this, cx| {
                                        this.start_upgrade_download(rel.clone(), window, cx);
                                    });
                                }),
                        );
                }
                _ => {
                    if release.asset.is_some() {
                        let rel = release;
                        footer = footer.child(
                            Button::new("upgrade-download-btn")
                                .primary()
                                .icon(LucideIcon::Download)
                                .label(crate::i18n::t("upgrade_download_and_install"))
                                .on_click(move |_, window, cx| {
                                    app_download.update(cx, |this, cx| {
                                        this.start_upgrade_download(rel.clone(), window, cx);
                                    });
                                }),
                        );
                    } else {
                        let url_open = html_url;
                        footer = footer.child(
                            Button::new("upgrade-open-releases-btn")
                                .primary()
                                .label(crate::i18n::t("upgrade_open_releases"))
                                .on_click(move |_, _, cx| {
                                    cx.open_url(&url_open);
                                }),
                        );
                    }
                }
            }

            dialog
                .title(crate::i18n::t("upgrade_available_title"))
                .width(px(480.))
                .child(body)
                .footer(footer)
        });
    }

    fn switch_workspace_dir(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match ensure_seeded_workspace(&dir) {
            Ok(tree) => {
                info!(dir = %dir.display(), "switched workspace directory");
                if let Err(err) = ensure_workspace_git_repo(&tree.root_dir) {
                    tracing::warn!(error = %err, "failed initializing workspace git repo");
                }
                if let Err(err) = save_config_workspace_dir(&dir) {
                    tracing::warn!(error = %err, "failed updating ~/.testrix/config.yaml");
                }
                self.workspace_dir = dir;
                self.workspace = tree;
                self.tabs.clear();
                self.active_tab_ix = None;
                let saved_expanded = load_workspace_explorer_state(&self.workspace.root_dir);
                self.history_entries = load_workspace_history(&self.workspace.root_dir);
                self.request_sets = load_workspace_sets(&self.workspace.root_dir);
                self.sync_sidebars(Some(&saved_expanded), cx);
                self.refresh_profile_selector(window, cx);
                self.sync_env_editor(window, cx);
                self.open_initial_request(window, cx);
                self.prompt_sync_after_workspace_switch(window, cx);
                cx.notify();
            }
            Err(err) => {
                window.push_notification(format!("Failed opening workspace: {err}"), cx);
            }
        }
    }

    fn prompt_sync_after_workspace_switch(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (server_url, email, token, saved_slug) = self.effective_sync_credentials();
        if server_url.is_empty() || token.is_empty() {
            return;
        }

        let workspace_name = self
            .workspace_dir
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("workspace")
            .to_string();

        let slug_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("workspace")
                .default_value(saved_slug)
        });
        let app_entity = cx.entity();

        window.open_dialog(cx, move |dialog, _, _cx| {
            let app_ok = app_entity.clone();
            let app_no = app_entity.clone();
            let slug_ok = slug_input.clone();
            let srv = server_url.clone();
            let acct = if email.is_empty() {
                "your account".to_string()
            } else {
                email.clone()
            };

            dialog
                .title("Sync Workspace with node-web?")
                .width(px(460.))
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            div().text_sm().child(format!(
                                "Switched to workspace \"{workspace_name}\". Would you like to run a bidirectional Git sync & merge with {srv} ({acct})?"
                            )),
                        )
                        .child(
                            Form::new().child(
                                Field::new()
                                    .label("Sync Repository Slug")
                                    .child(Input::new(&slug_input)),
                            ),
                        ),
                )
                .footer(
                    DialogFooter::new()
                        .gap_2()
                        .child(
                            Button::new("ws-sync-skip")
                                .label("No (Disconnect)")
                                .outline()
                                .on_click(move |_, window, cx| {
                                    app_no.update(cx, |this, cx| {
                                        this.sync_settings.sync_token.clear();
                                        this.share_settings.share_token.clear();
                                        let _ = save_config_sync_settings(&this.sync_settings);
                                        let _ = save_config_share_settings(&this.share_settings);
                                        this.sync_share_settings_to_tabs(cx);
                                        cx.notify();
                                    });
                                    window.close_dialog(cx);
                                    window.push_notification(
                                        "Disconnected node-web account.",
                                        cx,
                                    );
                                }),
                        )
                        .child(
                            Button::new("ws-sync-confirm")
                                .primary()
                                .icon(Icon::new(LucideIcon::GitMerge))
                                .label("Sync & Merge")
                                .on_click(move |_, window, cx| {
                                    let slug = slug_ok.read(cx).value().trim().to_string();
                                    app_ok.update(cx, |this, cx| {
                                        if !slug.is_empty() {
                                            let ws_dir = this.workspace_dir.clone();
                                            this.sync_settings
                                                .set_repo_slug_for_workspace(&ws_dir, &slug);
                                            let _ = save_config_sync_settings(&this.sync_settings);
                                        }
                                        this.start_bidirectional_git_sync(window, cx);
                                    });
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
    }

    fn confirm_logout_with_cloud_cleanup(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (server_url, email, token, repo_slug) = self.effective_sync_credentials();
        let share_server_url = self.share_settings.share_server_url.trim().to_string();
        let share_token = self.share_settings.share_token.trim().to_string();
        let testrix_share_slugs: Vec<String> = self
            .active_shares
            .iter()
            .filter(|s| s.channel == ShareChannel::TestrixServer)
            .map(|s| s.project_slug.clone())
            .collect();

        let app_entity = cx.entity();
        window.open_dialog(cx, move |dialog, _, _cx| {
            let app_keep = app_entity.clone();
            let app_delete = app_entity.clone();
            let srv = if !server_url.is_empty() {
                server_url.clone()
            } else {
                share_server_url.clone()
            };
            let acct = if email.is_empty() {
                "your account".to_string()
            } else {
                email.clone()
            };
            let del_srv = server_url.clone();
            let del_token = token.clone();
            let del_slug = repo_slug.clone();
            let del_share_srv = share_server_url.clone();
            let del_share_token = share_token.clone();
            let del_share_slugs = testrix_share_slugs.clone();

            dialog
                .title("Sign Out")
                .width(px(540.))
                .child(
                    v_flex().gap_2().child(
                        div().text_sm().child(format!(
                            "Do you also want to delete your cloud data (Git sync repo & shared projects) on {srv} for {acct} before signing out?"
                        )),
                    ),
                )
                .footer(
                    DialogFooter::new()
                        .gap_2()
                        .child(
                            DialogClose::new()
                                .child(Button::new("logout-cancel").label("Cancel").outline()),
                        )
                        .child(
                            Button::new("logout-keep-cloud")
                                .outline()
                                .label("Sign Out Only")
                                .on_click(move |_, window, cx| {
                                    app_keep.update(cx, |this, cx| {
                                        this.sync_settings.sync_token.clear();
                                        this.share_settings.share_token.clear();
                                        let _ = save_config_sync_settings(&this.sync_settings);
                                        let _ = save_config_share_settings(&this.share_settings);
                                        this.sync_share_settings_to_tabs(cx);
                                        cx.notify();
                                    });
                                    window.close_dialog(cx);
                                    window.push_notification("Signed out (cloud data kept).", cx);
                                    window.refresh();
                                }),
                        )
                        .child(
                            Button::new("logout-delete-cloud")
                                .danger()
                                .label("Delete & Sign Out")
                                .on_click(move |_, window, cx| {
                                    let srv_sync = del_srv.clone();
                                    let tok_sync = del_token.clone();
                                    let slug_sync = del_slug.clone();
                                    let srv_share = del_share_srv.clone();
                                    let tok_share = del_share_token.clone();
                                    let slugs_share = del_share_slugs.clone();

                                    let rx = dispatch_share_future(async move {
                                        let mut errs = Vec::new();
                                        if !srv_sync.is_empty()
                                            && !tok_sync.is_empty()
                                            && !slug_sync.is_empty()
                                            && let Err(e) = delete_remote_sync_repo(
                                                &srv_sync,
                                                &tok_sync,
                                                &slug_sync,
                                            )
                                            .await
                                        {
                                            errs.push(format!("Git sync delete: {e}"));
                                        }
                                        if !srv_share.is_empty() && !tok_share.is_empty() {
                                            for s in slugs_share {
                                                if let Err(e) = revoke_testrix_server_share(
                                                    srv_share.clone(),
                                                    tok_share.clone(),
                                                    s.clone(),
                                                )
                                                .await
                                                {
                                                    errs.push(format!("Share revoke ({s}): {e}"));
                                                }
                                            }
                                        }
                                        errs
                                    });

                                    let app_done = app_delete.clone();
                                    window.close_dialog(cx);
                                    window.spawn(cx, async move |window| {
                                        let errs = rx.await.unwrap_or_default();
                                        let _ = window.update(|window, cx| {
                                            app_done.update(cx, |this, cx| {
                                                this.sync_settings.sync_token.clear();
                                                this.share_settings.share_token.clear();
                                                this.active_shares.retain(|s| {
                                                    s.channel != ShareChannel::TestrixServer
                                                });
                                                this.persist_remote_shares();
                                                let _ = save_config_sync_settings(
                                                    &this.sync_settings,
                                                );
                                                let _ = save_config_share_settings(
                                                    &this.share_settings,
                                                );
                                                this.sync_share_settings_to_tabs(cx);
                                                cx.notify();
                                            });
                                            if errs.is_empty() {
                                                window.push_notification(
                                                    "Deleted cloud data and signed out.",
                                                    cx,
                                                );
                                            } else {
                                                window.push_notification(
                                                    format!(
                                                        "Signed out with warnings: {}",
                                                        errs.join("; ")
                                                    ),
                                                    cx,
                                                );
                                            }
                                            window.refresh();
                                        });
                                    })
                                    .detach();
                                }),
                        ),
                )
        });
    }

    fn effective_sync_credentials(&self) -> (String, String, String, String) {
        let server_url = if !self.sync_settings.sync_server_url.trim().is_empty() {
            self.sync_settings.sync_server_url.trim().to_string()
        } else {
            self.share_settings.share_server_url.trim().to_string()
        };
        let email = if !self.sync_settings.sync_email.trim().is_empty() {
            self.sync_settings.sync_email.trim().to_string()
        } else {
            self.share_settings.share_email.trim().to_string()
        };
        let token = if !self.sync_settings.sync_token.trim().is_empty() {
            self.sync_settings.sync_token.trim().to_string()
        } else {
            self.share_settings.share_token.trim().to_string()
        };
        let repo_slug = self
            .sync_settings
            .repo_slug_for_workspace(&self.workspace_dir);
        (server_url, email, token, repo_slug)
    }

    fn open_git_sync_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (current_url, current_email, _, current_slug) = self.effective_sync_credentials();
        let current_url = if current_url.is_empty() {
            "http://localhost:3456".to_string()
        } else {
            current_url
        };

        let server_url_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("http://localhost:3456")
                .default_value(current_url)
        });
        let email_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("you@example.com")
                .default_value(current_email)
        });
        let password_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Password (required to sign in)")
                .masked(true)
        });
        let repo_slug_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("workspace")
                .default_value(current_slug)
        });

        let app_entity = cx.entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let app_ref = app_entity.read(cx);
            let (_, signed_in_email, eff_token, _) = app_ref.effective_sync_credentials();
            let logged_in = !eff_token.is_empty();
            let syncing = app_ref.sync_in_progress;

            let app_login = app_entity.clone();
            let app_logout = app_entity.clone();
            let app_sync = app_entity.clone();

            let url_login = server_url_input.clone();
            let email_login = email_input.clone();
            let pass_login = password_input.clone();
            let slug_login = repo_slug_input.clone();

            let url_sync = server_url_input.clone();
            let email_sync = email_input.clone();
            let slug_sync = repo_slug_input.clone();

            dialog
                .title("Git Bidirectional Sync (Merge)")
                .width(px(520.))
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Independent 3-way Git merge between your local workspace and node-web (separate from Sharing)."),
                        )
                        .child(
                            Form::new()
                                .child(
                                    Field::new()
                                        .label("node-web Server URL")
                                        .child(Input::new(&server_url_input)),
                                )
                                .child(
                                    Field::new()
                                        .label("Sync Repository Slug")
                                        .child(Input::new(&repo_slug_input)),
                                )
                                .when(!logged_in, |form| {
                                    form.child(
                                        Field::new()
                                            .label("Email")
                                            .child(Input::new(&email_input)),
                                    )
                                    .child(
                                        Field::new()
                                            .label("Password")
                                            .child(Input::new(&password_input)),
                                    )
                                }),
                        )
                        .when(logged_in, |col| {
                            col.child(
                                h_flex()
                                    .justify_between()
                                    .items_center()
                                    .p_2p5()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .bg(cx.theme().secondary.opacity(0.4))
                                    .child(
                                        div()
                                            .text_xs()
                                            .child(format!("Signed in as {signed_in_email}")),
                                    )
                                    .child(
                                        Button::new("git-sync-signout")
                                            .outline()
                                            .xsmall()
                                            .label("Sign Out")
                                            .on_click(move |_, window, cx| {
                                                app_logout.update(cx, |this, cx| {
                                                    this.confirm_logout_with_cloud_cleanup(window, cx);
                                                });
                                            }),
                                    ),
                            )
                        }),
                )
                .footer(
                    DialogFooter::new()
                        .gap_2()
                        .child(
                            DialogClose::new()
                                .child(Button::new("git-sync-close").label("Close").outline()),
                        )
                        .when(!logged_in, |footer| {
                            footer.child(
                                Button::new("git-sync-login")
                                    .primary()
                                    .label("Sign In")
                                    .on_click(move |_, window, cx| {
                                        let url = url_login.read(cx).value().trim().to_string();
                                        let email = email_login.read(cx).value().trim().to_string();
                                        let password = pass_login.read(cx).value().to_string();
                                        let slug = slug_login.read(cx).value().trim().to_string();
                                        app_login.update(cx, |this, cx| {
                                            let ws_dir = this.workspace_dir.clone();
                                            this.sync_settings.sync_server_url = url.clone();
                                            this.sync_settings.sync_email = email.clone();
                                            this.sync_settings
                                                .set_repo_slug_for_workspace(&ws_dir, &slug);
                                            let _ = save_config_sync_settings(&this.sync_settings);
                                            this.login_git_sync(url, email, password, window, cx);
                                        });
                                    }),
                            )
                        })
                        .when(logged_in, |footer| {
                            footer.child(
                                Button::new("git-sync-merge-btn")
                                    .primary()
                                    .disabled(syncing)
                                    .icon(Icon::new(LucideIcon::GitMerge))
                                    .label(if syncing {
                                        "Syncing…"
                                    } else {
                                        "Sync & Merge Now"
                                    })
                                    .on_click(move |_, window, cx| {
                                        let url = url_sync.read(cx).value().trim().to_string();
                                        let email = email_sync.read(cx).value().trim().to_string();
                                        let slug = slug_sync.read(cx).value().trim().to_string();
                                        app_sync.update(cx, |this, cx| {
                                            let ws_dir = this.workspace_dir.clone();
                                            this.sync_settings.sync_server_url = url;
                                            if !email.is_empty() {
                                                this.sync_settings.sync_email = email;
                                            }
                                            this.sync_settings
                                                .set_repo_slug_for_workspace(&ws_dir, &slug);
                                            let _ = save_config_sync_settings(&this.sync_settings);
                                            this.start_bidirectional_git_sync(window, cx);
                                        });
                                        window.close_dialog(cx);
                                    }),
                            )
                        }),
                )
        });
    }

    fn login_git_sync(
        &mut self,
        server_url: String,
        email: String,
        password: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rx = dispatch_share_future(async move {
            login_sync_server(&server_url, &email, &password).await
        });
        let app_entity = cx.entity();
        cx.spawn_in(window, async move |_, window| {
            let res = rx.await;
            let _ = window.update(|window, cx| {
                match res {
                    Ok(Ok(login)) => {
                        app_entity.update(cx, |this, cx| {
                            this.sync_settings.sync_token = login.token.clone();
                            this.sync_settings.sync_email = login.email.clone();
                            let ws_dir = this.workspace_dir.clone();
                            let slug = this.sync_settings.repo_slug_for_workspace(&ws_dir);
                            this.sync_settings
                                .set_repo_slug_for_workspace(&ws_dir, &slug);
                            let _ = save_config_sync_settings(&this.sync_settings);
                            if this.share_settings.share_token.trim().is_empty() {
                                if !this.sync_settings.sync_server_url.trim().is_empty() {
                                    this.share_settings.share_server_url =
                                        this.sync_settings.sync_server_url.clone();
                                }
                                this.share_settings.share_email = login.email.clone();
                                this.share_settings.share_token = login.token;
                                let _ = save_config_share_settings(&this.share_settings);
                                this.sync_share_settings_to_tabs(cx);
                            }
                            this.start_bidirectional_git_sync(window, cx);
                            cx.notify();
                        });
                        window.close_dialog(cx);
                        window.push_notification(
                            format!("Signed in to Git Sync as {}. Starting sync…", login.email),
                            cx,
                        );
                    }
                    Ok(Err(err)) => {
                        window.push_notification(format!("Git Sync sign in failed: {err}"), cx);
                    }
                    Err(_) => {
                        window.push_notification("Git Sync sign in cancelled.", cx);
                    }
                }
            });
        })
        .detach();
    }

    fn maybe_auto_git_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (server_url, _, token, repo_slug) = self.effective_sync_credentials();
        if !server_url.is_empty()
            && !token.is_empty()
            && !repo_slug.is_empty()
            && !self.sync_in_progress
        {
            self.start_bidirectional_git_sync_with_options(false, window, cx);
        }
    }

    fn start_bidirectional_git_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_bidirectional_git_sync_with_options(true, window, cx);
    }

    fn start_bidirectional_git_sync_with_options(
        &mut self,
        notify_when_up_to_date: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.sync_in_progress {
            return;
        }
        let (server_url, _, token, repo_slug) = self.effective_sync_credentials();
        if server_url.is_empty() || token.is_empty() || repo_slug.is_empty() {
            window.push_notification(
                "Please configure Sync Server URL, Repository Slug, and sign in first.",
                cx,
            );
            return;
        }

        self.sync_in_progress = true;
        cx.notify();

        let workspace_root = self.workspace_dir.clone();
        let rx = dispatch_share_future(async move {
            if let Err(err) = ensure_workspace_git_repo(&workspace_root) {
                return Err(format!("Failed initializing local git repo: {err}"));
            }
            if let Err(err) = commit_workspace_change(&workspace_root, "Pre-sync local snapshot") {
                return Err(format!("Failed committing local changes: {err}"));
            }
            let local_files = collect_workspace_tracked_files(&workspace_root)
                .map_err(|e| format!("Failed reading local files: {e}"))?;
            let base_snap = load_sync_base(&workspace_root, &repo_slug)
                .map_err(|e| format!("Failed loading sync base: {e}"))?;
            let known_head = base_snap
                .as_ref()
                .and_then(|s| s.remote_head_oid.as_deref());
            let remote =
                fetch_remote_sync_state(&server_url, &token, &repo_slug, known_head).await?;
            // If the remote repository has no commits yet (empty/newly created or recreated),
            // do not treat a stale local sync base as remote deletions.
            let base_files = if remote.head_oid.is_none() {
                std::collections::BTreeMap::new()
            } else {
                base_snap.map(|s| s.files).unwrap_or_default()
            };
            let remote_files = if remote.unchanged {
                base_files.clone()
            } else {
                remote.files
            };
            let merge = three_way_merge_trees(&base_files, &local_files, &remote_files)
                .map_err(|e| format!("3-way merge failed: {e}"))?;

            if merge.local_changed {
                apply_synced_files_and_commit(
                    &workspace_root,
                    &merge.merged_files,
                    &merge.deleted_local_paths,
                    "Sync merge with node-web",
                )
                .map_err(|e| format!("Failed applying merged files locally: {e}"))?;
            }

            let mut next_remote_head = remote.head_oid.clone();
            if merge.remote_changed || (remote.head_oid.is_none() && !merge.merged_files.is_empty())
            {
                let mut changed_files = std::collections::BTreeMap::new();
                for (path, content) in &merge.merged_files {
                    if remote_files.get(path) != Some(content) {
                        changed_files.insert(path.clone(), content.clone());
                    }
                }
                let deleted_remote_paths: Vec<String> = remote_files
                    .keys()
                    .filter(|path| !merge.merged_files.contains_key(*path))
                    .cloned()
                    .collect();

                if !changed_files.is_empty() || !deleted_remote_paths.is_empty() {
                    let push_res = push_merged_sync_state(
                        &server_url,
                        &token,
                        &repo_slug,
                        remote.head_oid.as_deref(),
                        &changed_files,
                        &deleted_remote_paths,
                        false,
                        "Sync merge from Testrix",
                    )
                    .await?;
                    next_remote_head = push_res.head_oid;
                }
            }

            save_sync_base(
                &workspace_root,
                &repo_slug,
                &testrix_storage::SyncBaseSnapshot {
                    remote_head_oid: next_remote_head,
                    files: merge.merged_files.clone(),
                },
            )
            .map_err(|e| format!("Failed saving sync base: {e}"))?;

            Ok(merge)
        });

        let app_entity = cx.entity();
        cx.spawn_in(window, async move |_, window| {
            let res = rx.await;
            let _ = window.update(|window, cx| {
                app_entity.update(cx, |this, cx| {
                    this.sync_in_progress = false;
                    match res {
                        Ok(Ok(merge)) => {
                            if merge.local_changed {
                                this.reload_workspace(window, cx);
                            }
                            if !merge.conflicts.is_empty() {
                                window.push_notification(
                                    format!(
                                        "Sync merged with {} conflict(s): {}",
                                        merge.conflicts.len(),
                                        merge.conflicts.join(", ")
                                    ),
                                    cx,
                                );
                            } else if !merge.local_changed && !merge.remote_changed {
                                if notify_when_up_to_date {
                                    window.push_notification(
                                        "Already up to date with node-web.",
                                        cx,
                                    );
                                }
                            } else {
                                window.push_notification(
                                    format!(
                                        "Bidirectional Git sync complete ({} files).",
                                        merge.merged_files.len()
                                    ),
                                    cx,
                                );
                            }
                        }
                        Ok(Err(err)) => {
                            window.push_notification(format!("Git Sync failed: {err}"), cx);
                        }
                        Err(_) => {
                            window.push_notification("Git Sync task cancelled.", cx);
                        }
                    }
                    cx.notify();
                });
            });
        })
        .detach();
    }

    fn open_env_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.sync_env_editor(window, cx);
        let env_editor = self.env_editor.clone();
        env_editor.update(cx, |editor, cx| {
            window.focus(&editor.focus_handle(cx), cx);
        });
        window.open_sheet_at(Placement::Right, cx, move |sheet, _, _| {
            sheet
                .title("Profiles & Environment Variables")
                .size(px(640.))
                .child(div().size_full().p_4().child(env_editor.clone()))
                .footer(
                    Button::new("env-sheet-done-btn")
                        .primary()
                        .label(crate::i18n::t("done"))
                        .on_click(|_, window, cx| {
                            window.close_sheet(cx);
                        }),
                )
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
        let script_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(5)
                .placeholder("Optional: paste cURL, wget, or PowerShell (Invoke-WebRequest) command…")
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
            let script_ok = script_input.clone();
            let name_parse = name_input.clone();
            let url_parse = url_input.clone();
            let script_parse = script_input.clone();
            let pid = project_id.clone();
            let fid = folder_id.clone();

            let script_preview = script_input.read(cx).value().to_string();
            let parsed_preview = if script_preview.trim().is_empty() {
                None
            } else {
                Some(parse_request_script(&script_preview))
            };

            dialog
                .title("New HTTP Request")
                .width(px(540.))
                .child(
                    Form::new()
                        .child(Field::new().label("Name").child(Input::new(&name_input)))
                        .child(Field::new().label("URL").child(Input::new(&url_input)))
                        .child(
                            Field::new()
                                .label("Import from cURL / wget / PowerShell (Optional)")
                                .child(Textarea::new(&script_input)),
                        )
                        .when_some(parsed_preview, |form, res| {
                            form.child(
                                Field::new().child(match res {
                                    Ok(spec) => div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!(
                                            "Detected: {} {} ({} param(s), {} header(s))",
                                            spec.method.as_str(),
                                            spec.url,
                                            spec.params.len(),
                                            spec.headers.len()
                                        )),
                                    Err(err) => div()
                                        .text_xs()
                                        .text_color(cx.theme().danger)
                                        .child(format!("Parse error: {err}")),
                                }),
                            )
                        }),
                )
                .footer(
                    DialogFooter::new()
                        .gap_2()
                        .child(
                            Button::new("new-req-parse")
                                .label("Parse Script")
                                .outline()
                                .on_click(move |_, window, cx| {
                                    let raw = script_parse.read(cx).value().to_string();
                                    match parse_request_script(&raw) {
                                        Ok(spec) => {
                                            let url_val = spec.url.clone();
                                            url_parse.update(cx, |s, cx| {
                                                s.set_value(url_val, window, cx);
                                            });
                                            if name_parse.read(cx).value().trim().is_empty() {
                                                let inferred = infer_request_name(&spec);
                                                name_parse.update(cx, |s, cx| {
                                                    s.set_value(inferred, window, cx);
                                                });
                                            }
                                        }
                                        Err(err) => {
                                            window.push_notification(
                                                format!("Failed parsing script: {err}"),
                                                cx,
                                            );
                                        }
                                    }
                                }),
                        )
                        .child(
                            DialogClose::new()
                                .child(Button::new("new-req-cancel").label("Cancel").outline()),
                        )
                        .child(
                            Button::new("new-req-create")
                                .label("Create")
                                .primary()
                                .on_click(move |_, window, cx| {
                                    let raw_name = name_ok.read(cx).value().trim().to_string();
                                    let raw_url = url_ok.read(cx).value().trim().to_string();
                                    let raw_script = script_ok.read(cx).value().to_string();

                                    if !raw_script.trim().is_empty() {
                                        match parse_request_script(&raw_script) {
                                            Ok(spec) => {
                                                let name = if raw_name.is_empty() {
                                                    infer_request_name(&spec)
                                                } else {
                                                    raw_name
                                                };
                                                app_ok.update(cx, |this, cx| {
                                                    this.create_request_from_spec(
                                                        &pid,
                                                        fid.as_deref(),
                                                        &name,
                                                        spec,
                                                        window,
                                                        cx,
                                                    );
                                                });
                                                window.close_dialog(cx);
                                            }
                                            Err(err) => {
                                                window.push_notification(
                                                    format!("Failed parsing script: {err}"),
                                                    cx,
                                                );
                                            }
                                        }
                                    } else {
                                        app_ok.update(cx, |this, cx| {
                                            this.create_request_node(
                                                &pid,
                                                fid.as_deref(),
                                                &raw_name,
                                                &raw_url,
                                                window,
                                                cx,
                                            );
                                        });
                                        window.close_dialog(cx);
                                    }
                                }),
                        ),
                )
        });
    }

    fn create_request_from_spec(
        &mut self,
        project_id: &str,
        folder_id: Option<&str>,
        name: &str,
        spec: HttpRequestSpec,
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

        match create_request(&parent_dir, name, spec.method, &spec.url) {
            Ok(mut req_node) => {
                req_node.doc.request = spec;
                if let Err(err) = save_request(&req_node) {
                    window.push_notification(format!("Failed saving parsed request: {err}"), cx);
                    return;
                }
                let req_id = req_node.doc.id.clone();
                let req_name = req_node.doc.name.clone();
                let req_file_path = req_node.file_path.clone();
                let changed_file = share_file_from_disk(&proj.root_dir, &req_file_path);
                if let Some(proj_mut) = self.workspace.find_project_mut(project_id) {
                    if let Some(fid) = folder_id
                        && let Some(folder_mut) = proj_mut.find_folder_mut(fid)
                    {
                        folder_mut.requests.push(req_node);
                    } else {
                        proj_mut.requests.push(req_node);
                    }
                }
                self.commit_git_change(&format!("Create request: {req_name}"));
                self.sync_sidebars(None, cx);
                self.open_node(
                    ExplorerNode::Request {
                        project_id: project_id.to_string(),
                        request_id: req_id,
                    },
                    window,
                    cx,
                );
                self.sync_shared_project_after_save(project_id, changed_file, window, cx);
            }
            Err(err) => {
                window.push_notification(format!("Failed creating request: {err}"), cx);
            }
        }
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
                let req_name = req_node.doc.name.clone();
                if let Some(proj_mut) = self.workspace.find_project_mut(project_id) {
                    if let Some(fid) = folder_id
                        && let Some(folder_mut) = proj_mut.find_folder_mut(fid)
                    {
                        folder_mut.requests.push(req_node);
                    } else {
                        proj_mut.requests.push(req_node);
                    }
                }
                self.commit_git_change(&format!("Create request: {req_name}"));
                self.sync_sidebars(None, cx);
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
                let fname = folder_node.doc.name.clone();
                if let Some(proj_mut) = self.workspace.find_project_mut(project_id) {
                    if let Some(parent_id) = parent_folder_id
                        && let Some(parent_mut) = proj_mut.find_folder_mut(parent_id)
                    {
                        parent_mut.folders.push(folder_node);
                    } else {
                        proj_mut.folders.push(folder_node);
                    }
                }
                self.commit_git_change(&format!("Create folder: {fname}"));
                self.sync_sidebars(None, cx);
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
                let pname = proj_node.doc.name.clone();
                self.workspace.projects.push(proj_node);
                self.workspace.active_project_id = Some(pid.clone());
                self.commit_git_change(&format!("Create project: {pname}"));
                let _ = save_workspace_state(
                    &self.workspace_dir,
                    self.workspace.active_project_id.as_deref(),
                    self.workspace.active_environment_id.as_deref(),
                );
                self.sync_sidebars(None, cx);
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
                let copy_name = copy_node.doc.name.clone();
                if let Some(proj_mut) = self.workspace.find_project_mut(project_id) {
                    if let Some(fid) = parent_folder_id
                        && let Some(folder_mut) = proj_mut.find_folder_mut(&fid)
                    {
                        folder_mut.requests.push(copy_node);
                    } else {
                        proj_mut.requests.push(copy_node);
                    }
                }
                self.commit_git_change(&format!("Duplicate request: {copy_name}"));
                self.sync_sidebars(None, cx);
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
        let req_name_for_commit = req_name.clone();
        let file_path = req.file_path.clone();
        let involved_packages = packages_referencing_request(&self.request_sets, &request_id);
        let description = if involved_packages.is_empty() {
            "This permanently removes the YAML request file from disk.".to_string()
        } else {
            let shown: Vec<&str> = involved_packages
                .iter()
                .take(5)
                .map(String::as_str)
                .collect();
            let suffix = if involved_packages.len() > 5 {
                ", ..."
            } else {
                ""
            };
            format!(
                "This permanently removes the YAML request file from disk.\nInvolved packages: {}{suffix}",
                shown.join(", ")
            )
        };
        let app_entity = cx.entity();

        window.open_alert_dialog(cx, move |alert, _, _| {
            let app_ok = app_entity.clone();
            let pid = project_id.clone();
            let rid = request_id.clone();
            let path = file_path.clone();
            let del_name = req_name_for_commit.clone();
            let desc = description.clone();
            alert
                .title(format!("Delete \"{req_name}\"?"))
                .description(desc)
                .confirm()
                .ok_text("Delete")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    app_ok.update(cx, |this, cx| {
                        if let Err(err) = delete_request(&path) {
                            window.push_notification(format!("Delete failed: {err}"), cx);
                            return;
                        }
                        this.commit_git_change(&format!("Delete request: {del_name}"));
                        this.close_tabs_by_keys(&[format!("req:{rid}")], cx);
                        if let Some(proj) = this.workspace.find_project_mut(&pid) {
                            remove_request_from_project(proj, &rid);
                        }
                        this.sync_sidebars(None, cx);
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
        let folder_name_for_commit = folder_name.clone();
        let dir_path = folder.dir_path.clone();
        let app_entity = cx.entity();

        window.open_alert_dialog(cx, move |alert, _, _| {
            let app_ok = app_entity.clone();
            let path = dir_path.clone();
            let fid = folder_id.clone();
            let del_name = folder_name_for_commit.clone();
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
                        this.commit_git_change(&format!("Delete folder: {del_name}"));
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

    fn confirm_delete_project(
        &mut self,
        project_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.workspace.find_project(&project_id) else {
            return;
        };
        let project_name = project.doc.name.clone();
        let project_name_for_commit = project_name.clone();
        let root_dir = project.root_dir.clone();
        let app_entity = cx.entity();

        window.open_alert_dialog(cx, move |alert, _, _| {
            let app_ok = app_entity.clone();
            let path = root_dir.clone();
            let pid = project_id.clone();
            let del_name = project_name_for_commit.clone();
            alert
                .title(format!("Delete project \"{project_name}\"?"))
                .description(
                    "The project directory and all folders, requests, and environments inside it will be deleted from disk.",
                )
                .confirm()
                .ok_text("Delete")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    app_ok.update(cx, |this, cx| {
                        if let Err(err) = delete_project(&path) {
                            window.push_notification(format!("Delete failed: {err}"), cx);
                            return;
                        }
                        this.commit_git_change(&format!("Delete project: {del_name}"));
                        let removed_project = this.workspace.find_project(&pid).cloned();
                        this.tabs.retain(|t| {
                            let key = t.key(cx);
                            if key == format!("proj:{pid}") {
                                return false;
                            }
                            if let Some(proj) = &removed_project {
                                if let Some(rid) = key.strip_prefix("req:") {
                                    return proj.find_request(rid).is_none();
                                }
                                if let Some(fid) = key.strip_prefix("fld:") {
                                    return proj.find_folder(fid).is_none();
                                }
                            }
                            true
                        });
                        if this.workspace.active_project_id.as_deref() == Some(&pid) {
                            this.workspace.active_project_id = None;
                            this.workspace.active_environment_id = None;
                        }
                        this.reload_workspace(window, cx);
                    });
                    true
                })
        });
    }

    fn save_active_tab(&mut self, cx: &mut Context<Self>) {
        info!(active_tab_ix = ?self.active_tab_ix, tab_count = self.tabs.len(), "save_active_tab triggered");
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

    fn open_active_body_search(&mut self, replace_mode: bool, cx: &mut Context<Self>) {
        if let Some(ix) = self.active_tab_ix
            && let Some(WorkbenchTab::Request(view)) = self.tabs.get(ix)
        {
            view.update(cx, |v, cx| {
                if !v.open_request_body_search(replace_mode, cx) {
                    v.open_response_body_search(cx);
                }
            });
        }
    }
}

impl Render for TestrixApp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .on_action(cx.listener(|this, _: &Quit, window, cx| this.request_app_exit(window, cx)))
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
                if let Some(pane) = this.sidebar_panes.first().cloned() {
                    pane.update(cx, |p, cx| {
                        p.explorer.update(cx, |panel, cx| panel.focus_search(window, cx));
                    });
                }
            }))
            .on_action(cx.listener(|this, _: &CloseActiveTab, window, cx| {
                if let Some(ix) = this.active_tab_ix {
                    this.close_tab(ix, window, cx);
                }
            }))
            .on_action(cx.listener(|_, _: &ToggleTheme, _, cx| {
                let next = if cx.theme().is_dark() {
                    ThemeMode::Light
                } else {
                    ThemeMode::Dark
                };
                Theme::change(next, None, cx);
                crate::theme_helpers::configure_syntax_highlighting(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleEnvSheet, window, cx| {
                this.open_env_sheet(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ReloadWorkspace, window, cx| {
                this.reload_workspace(window, cx);
            }))
            .on_action(cx.listener(|this, _: &Search, _, cx| {
                this.open_active_body_search(false, cx);
            }))
            .on_action(cx.listener(|this, _: &Replace, _, cx| {
                this.open_active_body_search(true, cx);
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
                                            img(ImageSource::Image(app_titlebar_icon()))
                                                .size(px(18.))
                                                .rounded(px(4.)),
                                        )
                                        .child(div().text_sm().font_bold().child("Testrix")),
                                )
                                .child(
                                    Button::new("open-workspace-btn")
                                        .ghost()
                                        .xsmall()
                                        .icon(LucideIcon::FolderOpen)
                                        .label(crate::i18n::t("workspace"))
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
                                    Button::new("shares-window-btn")
                                        .ghost()
                                        .xsmall()
                                        .icon(LucideIcon::Globe)
                                        .label(if self.active_shares.is_empty() {
                                            "Shares".to_string()
                                        } else {
                                            format!("Shares ({})", self.active_shares.len())
                                        })
                                        .tooltip("View and revoke active shares")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_shares_dialog(window, cx);
                                        })),
                                )
                                .child(
                                    Button::new("preferences-btn")
                                        .ghost()
                                        .xsmall()
                                        .icon(LucideIcon::SlidersHorizontal)
                                        .tooltip(crate::i18n::t("preferences"))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_preferences_dialog(window, cx);
                                        })),
                                )
                                .child(
                                    Button::new("about-btn")
                                        .ghost()
                                        .xsmall()
                                        .icon(LucideIcon::Info)
                                        .tooltip(crate::i18n::t("about"))
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_about_dialog(window, cx);
                                        })),
                                )
                                .when_some(self.upgrade_status.active_release().cloned(), |this, release| {
                                    this.child(
                                        Button::new("upgrade-available-btn")
                                            .primary()
                                            .xsmall()
                                            .icon(LucideIcon::Download)
                                            .label(format!(
                                                "{} {}",
                                                crate::i18n::t("upgrade_badge"),
                                                release.tag_name
                                            ))
                                            .tooltip(crate::i18n::t("upgrade_available_title"))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.open_upgrade_dialog(window, cx);
                                            })),
                                    )
                                }),
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
                                    Button::new("git-sync-button")
                                        .outline()
                                        .xsmall()
                                        .icon(LucideIcon::GitMerge)
                                        .label(if self.sync_in_progress {
                                            "Syncing…"
                                        } else {
                                            "Sync"
                                        })
                                        .tooltip("Bidirectional Git Sync & Merge with node-web")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_git_sync_dialog(window, cx);
                                        })),
                                )
                                .child(
                                    Button::new("env-vars-button")
                                        .outline()
                                        .xsmall()
                                        .icon(LucideIcon::SlidersHorizontal)
                                        .label(format!("{} ({enabled_var_count})", crate::i18n::t("env_vars")))
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
                                        .child(if self.sidebar_panes.len() <= 1 {
                                            self.sidebar_panes
                                                .first()
                                                .cloned()
                                                .map(|p| p.into_any_element())
                                                .unwrap_or_else(|| div().into_any_element())
                                        } else {
                                            let mut split = v_resizable("sidebar-vertical-split");
                                            for pane in &self.sidebar_panes {
                                                split = split.child(
                                                    resizable_panel()
                                                        .min_size(px(120.))
                                                        .child(pane.clone()),
                                                );
                                            }
                                            split.into_any_element()
                                        }),
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
                    .when(self.share_sync_count > 0, |sb| {
                        sb.right(
                            h_flex()
                                .items_center()
                                .gap_1()
                                .text_xs()
                                .text_color(cx.theme().primary)
                                .child(Spinner::new().xsmall())
                                .child(Icon::new(IconName::RefreshCw).xsmall())
                                .child(if self.share_sync_count == 1 {
                                    "Syncing share…".to_string()
                                } else {
                                    format!("Syncing {} shares…", self.share_sync_count)
                                }),
                        )
                    })
                    .right(active_auth_summary)
                    .right(format!("Profile: {active_env_name}")),
            )
    }
}

impl TestrixApp {
    fn render_workbench_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected_ix = self.active_tab_ix.unwrap_or(0);
        let app_entity = cx.entity();

        div()
            .id("workbench-tabs-wrap")
            .w_full()
            .context_menu(move |menu, _, cx| {
                let app_read = app_entity.read(cx);
                if app_read.tabs.is_empty() {
                    return menu;
                }
                let target_ix = app_read
                    .context_tab_ix
                    .or(app_read.active_tab_ix)
                    .unwrap_or(0)
                    .min(app_read.tabs.len() - 1);
                let has_others = app_read.tabs.len() > 1;

                let app_close = app_entity.clone();
                let app_others = app_entity.clone();
                let app_all = app_entity.clone();

                menu.item(PopupMenuItem::new(crate::i18n::t("close_current_tab")).on_click(
                    move |_, window, cx| {
                        app_close.update(cx, |this, cx| {
                            this.close_tab(target_ix, window, cx);
                        });
                    },
                ))
                .when(has_others, |m| {
                    m.item(PopupMenuItem::new(crate::i18n::t("close_other_tabs")).on_click(
                        move |_, window, cx| {
                            app_others.update(cx, |this, cx| {
                                this.close_other_tabs(target_ix, window, cx);
                            });
                        },
                    ))
                })
                .separator()
                .item(PopupMenuItem::new(crate::i18n::t("close_all_tabs")).on_click(
                    move |_, window, cx| {
                        app_all.update(cx, |this, cx| {
                            this.close_all_tabs(window, cx);
                        });
                    },
                ))
            })
            .child(
                TabBar::new("workbench-tabs")
                    .w_full()
                    .folder()
                    .small()
                    .menu(true)
                    .max_width(px(220.))
                    .selected_index(selected_ix)
                    .on_click(cx.listener(|this, ix: &usize, window, cx| {
                        if *ix < this.tabs.len() {
                            this.active_tab_ix = Some(*ix);
                            let project_id = match &this.tabs[*ix] {
                                WorkbenchTab::Request(view) => view.read(cx).project_id().to_string(),
                                WorkbenchTab::Container(view) => {
                                    view.read(cx).project_id().to_string()
                                }
                            };
                            this.set_active_project(&project_id, window, cx);
                            let key = this.tabs[*ix].key(cx);
                            this.reveal_and_select_in_explorers(&key, cx);
                            this.persist_open_tabs(cx);
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
                            .on_mouse_down(
                                MouseButton::Right,
                                cx.listener(move |this, _, _, _| {
                                    this.context_tab_ix = Some(ix);
                                }),
                            )
                            .suffix(
                                Button::new(SharedString::from(format!("close-tab-{key}")))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Close)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.close_tab(ix, window, cx);
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
            ),
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

    for env in &workspace.environments {
        let enabled = env.doc.variables.iter().filter(|v| v.enabled).count();
        items.push(ProfileOption {
            env_id: Some(env.doc.id.clone()),
            label: SharedString::from(format!("{} ({enabled})", env.doc.name)),
            var_count: enabled,
        });
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

fn find_folder_dir_in_folders(folders: &[FolderNode], folder_id: &str) -> Option<PathBuf> {
    for f in folders {
        if f.doc.id == folder_id {
            return Some(f.dir_path.clone());
        }
        if let Some(found) = find_folder_dir_in_folders(&f.folders, folder_id) {
            return Some(found);
        }
    }
    None
}

fn take_request_from_folders(
    folders: &mut [FolderNode],
    request_id: &str,
) -> Option<testrix_core::RequestNode> {
    for f in folders {
        if let Some(pos) = f.requests.iter().position(|r| r.doc.id == request_id) {
            return Some(f.requests.remove(pos));
        }
        if let Some(found) = take_request_from_folders(&mut f.folders, request_id) {
            return Some(found);
        }
    }
    None
}

fn take_request_from_project(
    proj: &mut ProjectNode,
    request_id: &str,
) -> Option<testrix_core::RequestNode> {
    if let Some(pos) = proj.requests.iter().position(|r| r.doc.id == request_id) {
        return Some(proj.requests.remove(pos));
    }
    take_request_from_folders(&mut proj.folders, request_id)
}

fn insert_request_into_folders(
    folders: &mut [FolderNode],
    folder_id: &str,
    mut req: testrix_core::RequestNode,
) -> Result<(), testrix_core::RequestNode> {
    for f in folders {
        if f.doc.id == folder_id {
            f.requests.push(req);
            f.requests.sort_by(|a, b| {
                a.doc
                    .order
                    .cmp(&b.doc.order)
                    .then_with(|| a.doc.name.to_lowercase().cmp(&b.doc.name.to_lowercase()))
            });
            return Ok(());
        }
        match insert_request_into_folders(&mut f.folders, folder_id, req) {
            Ok(()) => return Ok(()),
            Err(returned) => req = returned,
        }
    }
    Err(req)
}

fn insert_request_into_project(
    proj: &mut ProjectNode,
    folder_id: Option<&str>,
    req: testrix_core::RequestNode,
) {
    if let Some(fid) = folder_id
        && insert_request_into_folders(&mut proj.folders, fid, req.clone()).is_ok()
    {
        return;
    }
    proj.requests.push(req);
    proj.requests.sort_by(|a, b| {
        a.doc
            .order
            .cmp(&b.doc.order)
            .then_with(|| a.doc.name.to_lowercase().cmp(&b.doc.name.to_lowercase()))
    });
}

fn packages_referencing_request(sets: &[RequestSet], request_id: &str) -> Vec<String> {
    fn folder_has_request(folder: &RequestSetFolder, request_id: &str) -> bool {
        folder.requests.iter().any(|r| r.request_id == request_id)
            || folder
                .folders
                .iter()
                .any(|sub| folder_has_request(sub, request_id))
    }

    sets.iter()
        .filter(|set| {
            set.requests.iter().any(|r| r.request_id == request_id)
                || set
                    .folders
                    .iter()
                    .any(|f| folder_has_request(f, request_id))
        })
        .map(|set| set.name.clone())
        .collect()
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
    gpui_shell::init(cx);
    crate::theme_helpers::configure_syntax_highlighting(cx);
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

fn utc_now_iso8601() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hour = rem / 3_600;
    let min = (rem % 3_600) / 60;
    let sec = rem % 60;

    // Howard Hinnant's civil-from-days algorithm (UTC).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}

fn insert_set_folder(
    folders: &mut [RequestSetFolder],
    parent_id: &str,
    mut new_folder: RequestSetFolder,
) -> Result<(), RequestSetFolder> {
    for f in folders {
        if f.id == parent_id {
            f.folders.push(new_folder);
            return Ok(());
        }
        match insert_set_folder(&mut f.folders, parent_id, new_folder) {
            Ok(()) => return Ok(()),
            Err(returned) => new_folder = returned,
        }
    }
    Err(new_folder)
}

fn find_set_folder_name(folders: &[RequestSetFolder], folder_id: &str) -> Option<String> {
    for f in folders {
        if f.id == folder_id {
            return Some(f.name.clone());
        }
        if let Some(name) = find_set_folder_name(&f.folders, folder_id) {
            return Some(name);
        }
    }
    None
}

fn rename_set_folder(folders: &mut [RequestSetFolder], folder_id: &str, next_name: &str) -> bool {
    for f in folders {
        if f.id == folder_id {
            f.name = next_name.to_string();
            return true;
        }
        if rename_set_folder(&mut f.folders, folder_id, next_name) {
            return true;
        }
    }
    false
}

fn remove_set_folder(folders: &mut Vec<RequestSetFolder>, folder_id: &str) -> bool {
    if let Some(pos) = folders.iter().position(|f| f.id == folder_id) {
        folders.remove(pos);
        return true;
    }
    for f in folders {
        if remove_set_folder(&mut f.folders, folder_id) {
            return true;
        }
    }
    false
}

fn take_item_from_set_folders(
    folders: &mut [RequestSetFolder],
    item_id: &str,
) -> Option<RequestSetItem> {
    for f in folders {
        if let Some(pos) = f.requests.iter().position(|r| r.id == item_id) {
            return Some(f.requests.remove(pos));
        }
        if let Some(found) = take_item_from_set_folders(&mut f.folders, item_id) {
            return Some(found);
        }
    }
    None
}

fn take_item_from_set(set: &mut RequestSet, item_id: &str) -> Option<RequestSetItem> {
    if let Some(pos) = set.requests.iter().position(|r| r.id == item_id) {
        return Some(set.requests.remove(pos));
    }
    take_item_from_set_folders(&mut set.folders, item_id)
}

fn insert_item_into_set_folders(
    folders: &mut [RequestSetFolder],
    folder_id: &str,
    mut item: RequestSetItem,
) -> Result<(), RequestSetItem> {
    for f in folders {
        if f.id == folder_id {
            if !f
                .requests
                .iter()
                .any(|r| r.project_id == item.project_id && r.request_id == item.request_id)
            {
                f.requests.push(item);
            }
            return Ok(());
        }
        match insert_item_into_set_folders(&mut f.folders, folder_id, item) {
            Ok(()) => return Ok(()),
            Err(returned) => item = returned,
        }
    }
    Err(item)
}

fn insert_item_into_set(set: &mut RequestSet, folder_id: Option<&str>, item: RequestSetItem) {
    if let Some(fid) = folder_id {
        if let Err(returned) = insert_item_into_set_folders(&mut set.folders, fid, item) {
            if !set
                .requests
                .iter()
                .any(|r| r.project_id == returned.project_id && r.request_id == returned.request_id)
            {
                set.requests.push(returned);
            }
        }
        return;
    }
    if !set
        .requests
        .iter()
        .any(|r| r.project_id == item.project_id && r.request_id == item.request_id)
    {
        set.requests.push(item);
    }
}

fn rewrite_string_in_place(
    target: &mut String,
    pairs: &[(String, String)],
    wrap_braces: bool,
) -> bool {
    let updated = if wrap_braces {
        rename_template_variables(target, pairs)
    } else {
        replace_template_variables(target, pairs)
    };
    if let Some(updated) = updated {
        *target = updated;
        true
    } else {
        false
    }
}

fn rewrite_kv_rows(
    rows: &mut [KeyValueRow],
    pairs: &[(String, String)],
    wrap_braces: bool,
) -> bool {
    let mut changed = false;
    for row in rows {
        changed |= rewrite_string_in_place(&mut row.name, pairs, wrap_braces);
        changed |= rewrite_string_in_place(&mut row.value, pairs, wrap_braces);
    }
    changed
}

fn rewrite_auth_config(
    auth: &mut AuthConfig,
    pairs: &[(String, String)],
    wrap_braces: bool,
) -> bool {
    match auth {
        AuthConfig::Inherit | AuthConfig::None => false,
        AuthConfig::Bearer { token, prefix } => {
            let mut changed = rewrite_string_in_place(token, pairs, wrap_braces);
            changed |= rewrite_string_in_place(prefix, pairs, wrap_braces);
            changed
        }
        AuthConfig::Basic { username, password } => {
            let mut changed = rewrite_string_in_place(username, pairs, wrap_braces);
            changed |= rewrite_string_in_place(password, pairs, wrap_braces);
            changed
        }
        AuthConfig::ApiKey { key, value, .. } => {
            let mut changed = rewrite_string_in_place(key, pairs, wrap_braces);
            changed |= rewrite_string_in_place(value, pairs, wrap_braces);
            changed
        }
    }
}

fn rewrite_request_doc(
    doc: &mut HttpRequestDoc,
    pairs: &[(String, String)],
    wrap_braces: bool,
) -> bool {
    let mut changed = rewrite_string_in_place(&mut doc.request.url, pairs, wrap_braces);
    changed |= rewrite_kv_rows(&mut doc.request.params, pairs, wrap_braces);
    changed |= rewrite_kv_rows(&mut doc.request.headers, pairs, wrap_braces);
    changed |= rewrite_auth_config(&mut doc.request.auth, pairs, wrap_braces);
    match &mut doc.request.body {
        RequestBody::None | RequestBody::Binary { .. } => {}
        RequestBody::Json { content } | RequestBody::Raw { content, .. } => {
            changed |= rewrite_string_in_place(content, pairs, wrap_braces);
        }
        RequestBody::FormUrlEncoded { fields } => {
            changed |= rewrite_kv_rows(fields, pairs, wrap_braces);
        }
        RequestBody::Multipart { fields } => {
            for field in fields {
                changed |= rewrite_string_in_place(&mut field.name, pairs, wrap_braces);
                changed |= rewrite_string_in_place(&mut field.value, pairs, wrap_braces);
            }
        }
    }
    changed
}

fn propagate_variable_rewrites_in_folder(
    folder: &mut FolderNode,
    pairs: &[(String, String)],
    wrap_braces: bool,
) {
    if rewrite_auth_config(&mut folder.doc.auth, pairs, wrap_braces) {
        let _ = save_folder(folder);
    }
    for req in &mut folder.requests {
        if rewrite_request_doc(&mut req.doc, pairs, wrap_braces) {
            let _ = save_request(req);
        }
    }
    for sub in &mut folder.folders {
        propagate_variable_rewrites_in_folder(sub, pairs, wrap_braces);
    }
}

fn propagate_variable_rewrites_in_project(
    proj: &mut ProjectNode,
    pairs: &[(String, String)],
    wrap_braces: bool,
) {
    let mut proj_changed = rewrite_string_in_place(&mut proj.doc.base_url, pairs, wrap_braces);
    proj_changed |= rewrite_kv_rows(&mut proj.doc.default_headers, pairs, wrap_braces);
    proj_changed |= rewrite_auth_config(&mut proj.doc.auth, pairs, wrap_braces);
    if proj_changed {
        let _ = save_project(proj);
    }
    for req in &mut proj.requests {
        if rewrite_request_doc(&mut req.doc, pairs, wrap_braces) {
            let _ = save_request(req);
        }
    }
    for folder in &mut proj.folders {
        propagate_variable_rewrites_in_folder(folder, pairs, wrap_braces);
    }
}

fn replace_url_base_prefix(url: &str, old_base: &str, new_base: &str) -> Option<String> {
    let trimmed = url.trim();
    let old_trimmed = old_base.trim().trim_end_matches('/');
    let new_trimmed = new_base.trim().trim_end_matches('/');
    if old_trimmed.is_empty() || new_trimmed.is_empty() || old_trimmed == new_trimmed {
        return None;
    }
    let rest = trimmed.strip_prefix(old_trimmed)?;
    if rest.is_empty() || rest.starts_with('/') || rest.starts_with('?') || rest.starts_with('#') {
        Some(format!("{new_trimmed}{rest}"))
    } else {
        None
    }
}

fn propagate_base_url_in_folder(folder: &mut FolderNode, old_base: &str, new_base: &str) {
    for req in &mut folder.requests {
        if let Some(next_url) = replace_url_base_prefix(&req.doc.request.url, old_base, new_base) {
            req.doc.request.url = next_url;
            let _ = save_request(req);
        }
    }
    for sub in &mut folder.folders {
        propagate_base_url_in_folder(sub, old_base, new_base);
    }
}

fn propagate_base_url_in_project(proj: &mut ProjectNode, old_base: &str, new_base: &str) {
    for req in &mut proj.requests {
        if let Some(next_url) = replace_url_base_prefix(&req.doc.request.url, old_base, new_base) {
            req.doc.request.url = next_url;
            let _ = save_request(req);
        }
    }
    for folder in &mut proj.folders {
        propagate_base_url_in_folder(folder, old_base, new_base);
    }
}

fn infer_request_name(spec: &HttpRequestSpec) -> String {
    let path_part = spec
        .url
        .split('?')
        .next()
        .unwrap_or(&spec.url)
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("")
        .trim();
    if path_part.is_empty() || path_part.contains("://") {
        format!("{} Request", spec.method.as_str())
    } else {
        format!("{} {}", spec.method.as_str(), path_part)
    }
}

fn share_file_from_disk(
    project_root: &std::path::Path,
    file_path: &std::path::Path,
) -> Option<ShareFile> {
    let rel = file_path.strip_prefix(project_root).ok()?;
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    let content = std::fs::read_to_string(file_path).ok()?;
    Some(ShareFile {
        path: rel_str,
        content,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_matching_base_url_prefix_and_preserves_other_variables() {
        assert_eq!(
            replace_url_base_prefix("{{base_url}}/get", "{{base_url}}", "{{base_url_emr}}"),
            Some("{{base_url_emr}}/get".to_string())
        );
        assert_eq!(
            replace_url_base_prefix("{{base_url}}", "{{base_url}}", "{{base_url_emr}}"),
            Some("{{base_url_emr}}".to_string())
        );
        assert_eq!(
            replace_url_base_prefix("{{other_var}}/get", "{{base_url}}", "{{base_url_emr}}"),
            None
        );
        assert_eq!(
            replace_url_base_prefix("/get", "{{base_url}}", "{{base_url_emr}}"),
            None
        );
    }
}
