use std::collections::{BTreeMap, HashMap};

use gpui_kit::{
    AppContext as _, Bounds, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement as _, PathBuilder, Pixels, Point,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
    assets::IconName as LucideIcon,
    canvas,
    component::{
        ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
        WindowExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Editor, EditorState, InputEvent},
        scroll::ScrollableElement as _,
        tab::{Tab, TabBar},
        tag::{Tag, TagVariant},
        tooltip::Tooltip,
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};
use testrix_core::{
    DagNode, EnvironmentDoc, HttpMethod, HttpRequestDoc, PackageDag, ProjectNode, RequestSet,
    RequestSetFolder, RequestSetItem, WorkspaceTree, generate_id,
    materialize_request_doc_with_extra_vars,
};
use testrix_http::{CancellationToken, HttpRunner};
use testrix_storage::to_yaml_pretty;

use crate::{
    explorer::DraggedRequest,
    post_script::run_post_script_with_thread,
    theme_helpers::method_badge,
};

const NODE_WIDTH: f32 = 216.0;
const NODE_HEIGHT: f32 = 58.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackageSection {
    Dag,
    Yaml,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DagNodeRunStatus {
    Idle,
    Pending,
    Running,
    Success { status_code: u16, duration_ms: u64 },
    Failed(String),
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConsoleLogLevel {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone)]
struct ConsoleLogEntry {
    level: ConsoleLogLevel,
    message: String,
}

#[derive(Debug, Clone)]
struct NodeDragState {
    node_id: String,
    offset_x: f32,
    offset_y: f32,
}

#[derive(Debug, Clone)]
struct ConnectingState {
    from_node_id: String,
    cursor_x: f32,
    cursor_y: f32,
}

#[derive(Debug, Clone)]
pub(crate) enum PackageTabEvent {
    SaveSet {
        set_id: String,
        set: RequestSet,
    },
    AddRequestToSet {
        set_id: String,
        item: RequestSetItem,
    },
    OpenRequest {
        project_id: String,
        request_id: String,
    },
    ApplyPostScriptEnv {
        env_id: String,
        vars: BTreeMap<String, String>,
    },
    ApplyPostScriptGlobalEnv {
        vars: BTreeMap<String, String>,
    },
    DirtyChanged,
}

pub(crate) struct PackageTabView {
    set: RequestSet,
    saved_set: RequestSet,
    workspace: WorkspaceTree,
    active_env: Option<EnvironmentDoc>,
    global_env: Option<EnvironmentDoc>,
    runner: HttpRunner,
    active_section: PackageSection,
    yaml_editor: Entity<EditorState>,
    yaml_error: Option<String>,
    suppress_yaml_sync: bool,
    canvas_bounds: Bounds<Pixels>,
    dragging_node: Option<NodeDragState>,
    connecting_from: Option<ConnectingState>,
    selected_node_id: Option<String>,
    selected_edge: Option<(String, String)>,
    is_running: bool,
    run_revision: u64,
    active_cancel: Option<CancellationToken>,
    node_statuses: HashMap<String, DagNodeRunStatus>,
    run_summary: Option<Result<String, String>>,
    console_logs: Vec<ConsoleLogEntry>,
    console_collapsed: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PackageTabEvent> for PackageTabView {}

impl PackageTabView {
    pub(crate) fn new(
        set: RequestSet,
        workspace: WorkspaceTree,
        active_env: Option<EnvironmentDoc>,
        global_env: Option<EnvironmentDoc>,
        runner: HttpRunner,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let yaml_text = to_yaml_pretty(&set.dag).unwrap_or_default();
        let yaml_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("yaml")
                .folding(true)
                .default_value(yaml_text)
        });

        let sub = cx.subscribe_in(
            &yaml_editor,
            window,
            |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) && !this.suppress_yaml_sync {
                    this.on_yaml_changed(cx);
                }
            },
        );

        Self {
            saved_set: set.clone(),
            set,
            workspace,
            active_env,
            global_env,
            runner,
            active_section: PackageSection::Dag,
            yaml_editor,
            yaml_error: None,
            suppress_yaml_sync: false,
            canvas_bounds: Bounds::default(),
            dragging_node: None,
            connecting_from: None,
            selected_node_id: None,
            selected_edge: None,
            is_running: false,
            run_revision: 0,
            active_cancel: None,
            node_statuses: HashMap::new(),
            run_summary: None,
            console_logs: Vec::new(),
            console_collapsed: true,
            _subscriptions: vec![sub],
        }
    }

    pub(crate) fn tab_key(&self) -> String {
        format!("pkg:{}", self.set.id)
    }

    pub(crate) fn set_id(&self) -> &str {
        &self.set.id
    }

    pub(crate) fn title(&self) -> String {
        self.set.name.clone()
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.set != self.saved_set
    }

    pub(crate) fn sync_context(
        &mut self,
        workspace: WorkspaceTree,
        active_env: Option<EnvironmentDoc>,
        global_env: Option<EnvironmentDoc>,
        latest_set: Option<&RequestSet>,
        cx: &mut Context<Self>,
    ) {
        self.workspace = workspace;
        self.active_env = active_env;
        self.global_env = global_env;
        if let Some(latest) = latest_set {
            self.set.name = latest.name.clone();
            self.set.folders = latest.folders.clone();
            self.set.requests = latest.requests.clone();
            self.saved_set.name = latest.name.clone();
            self.saved_set.folders = latest.folders.clone();
            self.saved_set.requests = latest.requests.clone();
        }
        cx.notify();
    }

    pub(crate) fn mark_saved(&mut self, saved: &RequestSet, cx: &mut Context<Self>) {
        self.saved_set = saved.clone();
        cx.emit(PackageTabEvent::DirtyChanged);
        cx.notify();
    }

    pub(crate) fn save(&mut self, cx: &mut Context<Self>) {
        if let Err(err) = self.set.dag.validate() {
            self.run_summary = Some(Err(format!("Cannot save invalid DAG: {err}")));
            cx.notify();
            return;
        }
        self.saved_set = self.set.clone();
        cx.emit(PackageTabEvent::SaveSet {
            set_id: self.set.id.clone(),
            set: self.set.clone(),
        });
        cx.emit(PackageTabEvent::DirtyChanged);
        cx.notify();
    }

    fn sync_yaml_from_dag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let yaml_text = to_yaml_pretty(&self.set.dag).unwrap_or_default();
        self.suppress_yaml_sync = true;
        self.yaml_editor.update(cx, |editor, cx| {
            editor.set_value(yaml_text, window, cx);
        });
        self.suppress_yaml_sync = false;
        self.yaml_error = None;
    }

    fn on_yaml_changed(&mut self, cx: &mut Context<Self>) {
        let raw = self.yaml_editor.read(cx).value().to_string();
        let parsed: Result<PackageDag, _> = if raw.trim().is_empty() {
            Ok(PackageDag::default())
        } else {
            yaml_serde::from_str(&raw)
        };
        match parsed {
            Ok(mut dag) => match dag.validate() {
                Ok(()) => {
                    dag.auto_layout_if_unpositioned();
                    self.set.dag = dag;
                    self.yaml_error = None;
                    cx.emit(PackageTabEvent::DirtyChanged);
                }
                Err(err) => {
                    self.yaml_error = Some(err.to_string());
                }
            },
            Err(err) => {
                self.yaml_error = Some(format!("YAML syntax error: {err}"));
            }
        }
        cx.notify();
    }

    fn resolve_request_info(&self, node: &DagNode) -> (HttpMethod, String, bool) {
        if !node.project_id.is_empty()
            && let Some(proj) = self.workspace.find_project(&node.project_id)
            && let Some((_, req)) = proj.find_request(&node.request_id)
        {
            return (req.doc.request.method, req.doc.name.clone(), true);
        }
        for proj in &self.workspace.projects {
            if let Some((_, req)) = proj.find_request(&node.request_id) {
                return (req.doc.request.method, req.doc.name.clone(), true);
            }
        }
        let fallback_name = if node.name.is_empty() {
            node.request_id.clone()
        } else {
            node.name.clone()
        };
        (HttpMethod::Get, fallback_name, false)
    }

    fn resolve_request_doc<'a>(
        workspace: &'a WorkspaceTree,
        node: &DagNode,
    ) -> Option<(&'a ProjectNode, Vec<&'a testrix_core::FolderNode>, &'a HttpRequestDoc)> {
        if !node.project_id.is_empty()
            && let Some(proj) = workspace.find_project(&node.project_id)
            && let Some((ancestors, req)) = proj.find_request(&node.request_id)
        {
            return Some((proj, ancestors, &req.doc));
        }
        for proj in &workspace.projects {
            if let Some((ancestors, req)) = proj.find_request(&node.request_id) {
                return Some((proj, ancestors, &req.doc));
            }
        }
        None
    }

    fn add_node_from_drop(
        &mut self,
        drag: &DraggedRequest,
        drop_pos: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(request_id) = drag.request_id.clone() else {
            window.push_notification(
                "Only saved Explorer or Package requests can be added to a DAG.",
                cx,
            );
            return;
        };
        let project_id = drag.project_id.clone().unwrap_or_default();
        let origin = self.canvas_bounds.origin;
        let x = (f32::from(drop_pos.x - origin.x) - NODE_WIDTH * 0.5).max(16.0);
        let y = (f32::from(drop_pos.y - origin.y) - NODE_HEIGHT * 0.5).max(16.0);

        let mut node_id = generate_id("node");
        while self.set.dag.nodes.iter().any(|n| n.id == node_id) {
            node_id = generate_id("node");
        }

        self.set.dag.nodes.push(DagNode {
            id: node_id.clone(),
            project_id: project_id.clone(),
            request_id: request_id.clone(),
            name: drag.name.clone(),
            needs: Vec::new(),
            x,
            y,
        });
        if !project_id.is_empty() && !set_contains_request(&self.set, &project_id, &request_id) {
            let item = RequestSetItem {
                id: generate_id("seti"),
                project_id,
                request_id,
                name: drag.name.clone(),
                method: drag.method,
                url: drag.url.clone(),
            };
            self.set.requests.push(item.clone());
            self.saved_set.requests.push(item.clone());
            cx.emit(PackageTabEvent::AddRequestToSet {
                set_id: self.set.id.clone(),
                item,
            });
        }
        self.selected_node_id = Some(node_id);
        self.selected_edge = None;
        self.sync_yaml_from_dag(window, cx);
        cx.emit(PackageTabEvent::DirtyChanged);
        cx.notify();
    }

    fn try_connect_nodes(
        &mut self,
        from_id: &str,
        to_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if from_id == to_id {
            window.push_notification("Cannot connect a node to itself (loops are not allowed).", cx);
            return;
        }
        if self
            .set
            .dag
            .nodes
            .iter()
            .any(|n| n.id == to_id && n.needs.iter().any(|d| d == from_id))
        {
            return;
        }
        if self.set.dag.would_create_cycle(from_id, to_id) {
            window.push_notification(
                "Cannot connect: would create a loop in the DAG.",
                cx,
            );
            return;
        }
        if let Some(target) = self.set.dag.nodes.iter_mut().find(|n| n.id == to_id) {
            target.needs.push(from_id.to_string());
        }
        self.selected_edge = Some((from_id.to_string(), to_id.to_string()));
        self.selected_node_id = None;
        self.sync_yaml_from_dag(window, cx);
        cx.emit(PackageTabEvent::DirtyChanged);
        cx.notify();
    }

    fn delete_node(&mut self, node_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.set.dag.nodes.retain(|n| n.id != node_id);
        for node in &mut self.set.dag.nodes {
            node.needs.retain(|d| d != node_id);
        }
        if self.selected_node_id.as_deref() == Some(node_id) {
            self.selected_node_id = None;
        }
        self.node_statuses.remove(node_id);
        self.sync_yaml_from_dag(window, cx);
        cx.emit(PackageTabEvent::DirtyChanged);
        cx.notify();
    }

    fn delete_edge(
        &mut self,
        from: &str,
        to: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(target) = self.set.dag.nodes.iter_mut().find(|n| n.id == to) {
            target.needs.retain(|d| d != from);
        }
        if self
            .selected_edge
            .as_ref()
            .is_some_and(|(f, t)| f == from && t == to)
        {
            self.selected_edge = None;
        }
        self.sync_yaml_from_dag(window, cx);
        cx.emit(PackageTabEvent::DirtyChanged);
        cx.notify();
    }

    fn run_dag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_running {
            return;
        }
        let order = match self.set.dag.topological_order() {
            Ok(order) => order,
            Err(err) => {
                self.run_summary = Some(Err(err.to_string()));
                window.push_notification(format!("Invalid DAG: {err}"), cx);
                cx.notify();
                return;
            }
        };
        if order.is_empty() {
            return;
        }

        self.run_revision = self.run_revision.wrapping_add(1);
        let revision = self.run_revision;
        let cancel_token = CancellationToken::new();
        self.active_cancel = Some(cancel_token.clone());
        self.is_running = true;
        self.run_summary = None;
        self.console_collapsed = false;
        self.console_logs.push(ConsoleLogEntry {
            level: ConsoleLogLevel::Info,
            message: format!("Starting DAG run ({} node(s))…", order.len()),
        });
        self.node_statuses.clear();
        for id in &order {
            self.node_statuses
                .insert(id.clone(), DagNodeRunStatus::Pending);
        }
        cx.notify();

        let nodes_by_id: HashMap<String, DagNode> = self
            .set
            .dag
            .nodes
            .iter()
            .cloned()
            .map(|n| (n.id.clone(), n))
            .collect();
        let total_count = order.len();

        cx.spawn_in(window, async move |weak_self, cx| {
            let mut thread_vars: BTreeMap<String, String> = BTreeMap::new();
            let mut completed_count = 0usize;
            let mut failure_message: Option<String> = None;

            for node_id in &order {
                if cancel_token.is_cancelled() {
                    break;
                }
                let Some(node) = nodes_by_id.get(node_id).cloned() else {
                    continue;
                };

                // Mark current node Running and snapshot workspace/env from main view.
                let prep_result = weak_self.update_in(cx, |this, _, cx| {
                    if this.run_revision != revision {
                        return Err("stale run".to_string());
                    }
                    this.node_statuses
                        .insert(node_id.clone(), DagNodeRunStatus::Running);
                    cx.notify();

                    let Some((proj, ancestors, req_doc)) =
                        Self::resolve_request_doc(&this.workspace, &node)
                    else {
                        return Err(format!("Request `{}` not found", node.request_id));
                    };

                    let mut extra_vars = BTreeMap::new();
                    for (k, v) in &thread_vars {
                        extra_vars.insert(format!("thread.{k}"), v.clone());
                    }

                    let prepared = materialize_request_doc_with_extra_vars(
                        proj,
                        &ancestors,
                        req_doc,
                        this.active_env.as_ref(),
                        &extra_vars,
                    )
                    .map_err(|e| e.to_string())?;

                    Ok((
                        this.runner.clone(),
                        prepared,
                        req_doc.clone(),
                        this.active_env.clone(),
                        this.global_env.clone(),
                    ))
                });

                let (runner, prepared, req_doc, active_env, global_env) = match prep_result {
                    Ok(Ok(data)) => data,
                    Ok(Err(err)) => {
                        failure_message = Some(format!("{}: {err}", node.name));
                        let _ = weak_self.update(cx, |this, cx| {
                            if this.run_revision == revision {
                                this.node_statuses.insert(
                                    node_id.clone(),
                                    DagNodeRunStatus::Failed(err.clone()),
                                );
                                this.console_logs.push(ConsoleLogEntry {
                                    level: ConsoleLogLevel::Error,
                                    message: format!("✗ {}: {err}", node.name),
                                });
                                cx.notify();
                            }
                        });
                        break;
                    }
                    Err(_) => return,
                };

                let rx = runner.dispatch(prepared, cancel_token.clone());
                let outcome = rx.await;

                match outcome {
                    Ok(Ok(snapshot)) => {
                        let script = req_doc.request.post_script.clone();
                        let mut script_err: Option<String> = None;
                        if !script.trim().is_empty() {
                            match run_post_script_with_thread(
                                &script,
                                &req_doc.id,
                                &req_doc.name,
                                active_env.as_ref(),
                                global_env.as_ref(),
                                Some(&thread_vars),
                                &snapshot,
                            ) {
                                Ok(res) => {
                                    thread_vars = res.next_thread_vars;
                                    let _ = weak_self.update(cx, |this, cx| {
                                        if this.run_revision != revision {
                                            return;
                                        }
                                        if !res.changed_keys.is_empty()
                                            && let Some(env) = &this.active_env
                                        {
                                            cx.emit(PackageTabEvent::ApplyPostScriptEnv {
                                                env_id: env.id.clone(),
                                                vars: res.next_vars,
                                            });
                                        }
                                        if !res.changed_global_keys.is_empty() {
                                            cx.emit(PackageTabEvent::ApplyPostScriptGlobalEnv {
                                                vars: res.next_global_vars,
                                            });
                                        }
                                    });
                                }
                                Err(err) => {
                                    script_err = Some(format!("Post-script error: {err}"));
                                }
                            }
                        }

                        if let Some(err) = script_err {
                            failure_message = Some(format!("{}: {err}", node.name));
                            let _ = weak_self.update(cx, |this, cx| {
                                if this.run_revision == revision {
                                    this.console_logs.push(ConsoleLogEntry {
                                        level: ConsoleLogLevel::Error,
                                        message: format!("✗ {}: {err}", node.name),
                                    });
                                    this.node_statuses
                                        .insert(node_id.clone(), DagNodeRunStatus::Failed(err));
                                    cx.notify();
                                }
                            });
                            break;
                        }

                        completed_count += 1;
                        let status_code = snapshot.status_code;
                        let duration_ms = snapshot.duration_ms;
                        let _ = weak_self.update(cx, |this, cx| {
                            if this.run_revision == revision {
                                this.node_statuses.insert(
                                    node_id.clone(),
                                    DagNodeRunStatus::Success {
                                        status_code,
                                        duration_ms,
                                    },
                                );
                                this.console_logs.push(ConsoleLogEntry {
                                    level: ConsoleLogLevel::Success,
                                    message: format!(
                                        "✓ {}: {status_code} · {duration_ms}ms",
                                        node.name
                                    ),
                                });
                                cx.notify();
                            }
                        });
                    }
                    Ok(Err(err)) => {
                        let msg = err.to_string();
                        failure_message = Some(format!("{}: {msg}", node.name));
                        let _ = weak_self.update(cx, |this, cx| {
                            if this.run_revision == revision {
                                this.console_logs.push(ConsoleLogEntry {
                                    level: ConsoleLogLevel::Error,
                                    message: format!("✗ {}: {msg}", node.name),
                                });
                                this.node_statuses
                                    .insert(node_id.clone(), DagNodeRunStatus::Failed(msg));
                                cx.notify();
                            }
                        });
                        break;
                    }
                    Err(_) => {
                        failure_message = Some("Execution channel closed".to_string());
                        break;
                    }
                }
            }

            let cancelled = cancel_token.is_cancelled();
            let _ = weak_self.update(cx, |this, cx| {
                if this.run_revision != revision {
                    return;
                }
                this.is_running = false;
                this.active_cancel = None;
                for id in &order {
                    if matches!(
                        this.node_statuses.get(id),
                        Some(DagNodeRunStatus::Pending | DagNodeRunStatus::Running)
                    ) {
                        this.node_statuses
                            .insert(id.clone(), DagNodeRunStatus::Skipped);
                    }
                }
                if cancelled {
                    let msg = format!("Stopped ({completed_count}/{total_count} completed)");
                    this.console_logs.push(ConsoleLogEntry {
                        level: ConsoleLogLevel::Error,
                        message: msg.clone(),
                    });
                    this.run_summary = Some(Err(msg));
                } else if let Some(err) = failure_message {
                    this.console_logs.push(ConsoleLogEntry {
                        level: ConsoleLogLevel::Error,
                        message: format!("DAG run failed: {err}"),
                    });
                    this.run_summary = Some(Err(format!("Failed ({completed_count}/{total_count})")));
                } else {
                    let msg = format!("Completed {completed_count}/{total_count} nodes");
                    this.console_logs.push(ConsoleLogEntry {
                        level: ConsoleLogLevel::Success,
                        message: msg.clone(),
                    });
                    this.run_summary = Some(Ok(msg));
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn stop_dag(&mut self, cx: &mut Context<Self>) {
        if let Some(token) = self.active_cancel.take() {
            token.cancel();
        }
        self.is_running = false;
        cx.notify();
    }

    fn render_draw_pane(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let view = cx.entity();
        let view_drop = view.clone();
        let view_move = view.clone();
        let view_up = view.clone();
        let view_bg_down = view.clone();
        let view_play = view.clone();
        let view_stop = view.clone();
        let view_canvas = view.clone();

        let nodes = self.set.dag.nodes.clone();
        let edges: Vec<(String, String)> = nodes
            .iter()
            .flat_map(|n| n.needs.iter().map(move |dep| (dep.clone(), n.id.clone())))
            .collect();
        let connecting = self.connecting_from.clone();
        let selected_edge = self.selected_edge.clone();
        let node_statuses = self.node_statuses.clone();

        let edge_color = cx.theme().muted_foreground;
        let active_edge_color = cx.theme().primary;
        let success_edge_color = gpui_kit::rgb(0x22c55e);

        let nodes_map: HashMap<&str, &DagNode> =
            nodes.iter().map(|n| (n.id.as_str(), n)).collect();

        // Collect edge midpoint coordinates for delete pills.
        let mut edge_midpoints: Vec<(String, String, f32, f32, bool)> = Vec::new();
        for (from_id, to_id) in &edges {
            if let (Some(from_n), Some(to_n)) =
                (nodes_map.get(from_id.as_str()), nodes_map.get(to_id.as_str()))
            {
                let x1 = from_n.x + NODE_WIDTH;
                let y1 = from_n.y + NODE_HEIGHT * 0.5;
                let x2 = to_n.x;
                let y2 = to_n.y + NODE_HEIGHT * 0.5;
                let is_sel = selected_edge
                    .as_ref()
                    .is_some_and(|(f, t)| f == from_id && t == to_id);
                edge_midpoints.push((
                    from_id.clone(),
                    to_id.clone(),
                    (x1 + x2) * 0.5,
                    (y1 + y2) * 0.5,
                    is_sel,
                ));
            }
        }

        let canvas_nodes = nodes.clone();
        let canvas_edges = edges.clone();

        div()
            .relative()
            .flex_1()
            .w_full()
            .h_full()
            .overflow_hidden()
            .bg(cx.theme().background)
            .drag_over::<DraggedRequest>(|style, _, _, cx| {
                style.border_2().border_color(cx.theme().primary)
            })
            .on_drop(move |drag: &DraggedRequest, window, cx| {
                let pos = window.mouse_position();
                let drag = drag.clone();
                view_drop.update(cx, |this, cx| {
                    this.add_node_from_drop(&drag, pos, window, cx);
                });
            })
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                view_bg_down.update(cx, |this, cx| {
                    this.selected_node_id = None;
                    this.selected_edge = None;
                    cx.notify();
                });
            })
            .on_mouse_move(move |event: &MouseMoveEvent, _, cx| {
                view_move.update(cx, |this, cx| {
                    let origin = this.canvas_bounds.origin;
                    if let Some(drag) = &this.dragging_node {
                        let nx = (f32::from(event.position.x - origin.x) - drag.offset_x).max(8.0);
                        let ny = (f32::from(event.position.y - origin.y) - drag.offset_y).max(8.0);
                        if let Some(node) =
                            this.set.dag.nodes.iter_mut().find(|n| n.id == drag.node_id)
                        {
                            node.x = nx;
                            node.y = ny;
                            cx.notify();
                        }
                    } else if let Some(conn) = &mut this.connecting_from {
                        conn.cursor_x = f32::from(event.position.x - origin.x);
                        conn.cursor_y = f32::from(event.position.y - origin.y);
                        cx.notify();
                    }
                });
            })
            .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                view_up.update(cx, |this, cx| {
                    if this.dragging_node.take().is_some() {
                        this.sync_yaml_from_dag(window, cx);
                        cx.emit(PackageTabEvent::DirtyChanged);
                        cx.notify();
                    }
                    if this.connecting_from.take().is_some() {
                        cx.notify();
                    }
                });
            })
            // Vector arrow layer
            .child(
                canvas(
                    move |bounds, _, cx| {
                        view_canvas.update(cx, |this, _| {
                            this.canvas_bounds = bounds;
                        });
                        bounds
                    },
                    move |bounds, _, window, _| {
                        let ox = f32::from(bounds.origin.x);
                        let oy = f32::from(bounds.origin.y);
                        let map: HashMap<&str, &DagNode> =
                            canvas_nodes.iter().map(|n| (n.id.as_str(), n)).collect();

                        for (from_id, to_id) in &canvas_edges {
                            let (Some(from_n), Some(to_n)) =
                                (map.get(from_id.as_str()), map.get(to_id.as_str()))
                            else {
                                continue;
                            };
                            let p1 = gpui_kit::point(
                                px(ox + from_n.x + NODE_WIDTH),
                                px(oy + from_n.y + NODE_HEIGHT * 0.5),
                            );
                            let p2 = gpui_kit::point(
                                px(ox + to_n.x),
                                px(oy + to_n.y + NODE_HEIGHT * 0.5),
                            );
                            let color = if selected_edge
                                .as_ref()
                                .is_some_and(|(f, t)| f == from_id && t == to_id)
                            {
                                active_edge_color
                            } else if matches!(
                                node_statuses.get(from_id),
                                Some(DagNodeRunStatus::Success { .. })
                            ) {
                                success_edge_color.into()
                            } else {
                                edge_color
                            };
                            paint_arrow_curve(window, p1, p2, color);
                        }

                        if let Some(conn) = &connecting
                            && let Some(from_n) = map.get(conn.from_node_id.as_str())
                        {
                            let p1 = gpui_kit::point(
                                px(ox + from_n.x + NODE_WIDTH),
                                px(oy + from_n.y + NODE_HEIGHT * 0.5),
                            );
                            let p2 = gpui_kit::point(
                                px(ox + conn.cursor_x),
                                px(oy + conn.cursor_y),
                            );
                            paint_arrow_curve(window, p1, p2, active_edge_color);
                        }
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            )
            // Empty state hint
            .when(self.set.dag.nodes.is_empty(), |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            v_flex()
                                .items_center()
                                .gap_2()
                                .p_6()
                                .rounded(cx.theme().radius_lg)
                                .border_1()
                                .border_dashed()
                                .border_color(cx.theme().border)
                                .child(
                                    Icon::new(LucideIcon::Workflow)
                                        .size(px(28.))
                                        .text_color(cx.theme().muted_foreground),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_medium()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Drag & drop requests here to build a DAG"),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Connect right output port → left input port. Loops are prevented automatically."),
                                ),
                        ),
                )
            })
            // Edge midpoint delete pills
            .children(edge_midpoints.into_iter().enumerate().map(
                |(ix, (from_id, to_id, mx, my, is_sel))| {
                    let v_sel = view.clone();
                    let v_del = view.clone();
                    let f1 = from_id.clone();
                    let t1 = to_id.clone();
                    div()
                        .id(SharedString::from(format!("dag-edge-pill-{ix}")))
                        .absolute()
                        .left(px(mx - 10.0))
                        .top(px(my - 10.0))
                        .w(px(20.))
                        .h(px(20.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(cx.theme().popover)
                        .border_1()
                        .border_color(if is_sel {
                            cx.theme().primary
                        } else {
                            cx.theme().border
                        })
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .hover(|s| {
                            s.border_color(cx.theme().danger)
                                .text_color(cx.theme().danger)
                        })
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            cx.stop_propagation();
                            v_sel.update(cx, |this, cx| {
                                this.selected_edge = Some((f1.clone(), t1.clone()));
                                cx.notify();
                            });
                        })
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            v_del.update(cx, |this, cx| {
                                this.delete_edge(&from_id, &to_id, window, cx);
                            });
                        })
                        .child("×")
                },
            ))
            // Node cards
            .children(self.set.dag.nodes.iter().map(|node| {
                self.render_dag_node(node, cx)
            }))
            // Top-right corner Play / Stop controls
            .child(
                h_flex()
                    .absolute()
                    .top_3()
                    .right_3()
                    .items_center()
                    .gap_1p5()
                    .px_2()
                    .py_1()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().popover)
                    .border_1()
                    .border_color(cx.theme().border)
                    .shadow_sm()
                    .when_some(self.run_summary.as_ref(), |row, summary| {
                        let (label, variant) = match summary {
                            Ok(msg) => (msg.clone(), TagVariant::Success),
                            Err(msg) => (msg.clone(), TagVariant::Danger),
                        };
                        row.child(Tag::new().with_variant(variant).child(label))
                    })
                    .child(
                        Button::new("dag-play-btn")
                            .primary()
                            .xsmall()
                            .icon(Icon::new(IconName::Play))
                            .tooltip("Run DAG")
                            .disabled(self.is_running || self.set.dag.nodes.is_empty())
                            .on_click(move |_, window, cx| {
                                view_play.update(cx, |this, cx| {
                                    this.run_dag(window, cx);
                                });
                            }),
                    )
                    .child(
                        Button::new("dag-stop-btn")
                            .danger()
                            .xsmall()
                            .icon(Icon::new(LucideIcon::Square))
                            .tooltip("Stop DAG")
                            .disabled(!self.is_running)
                            .on_click(move |_, _, cx| {
                                view_stop.update(cx, |this, cx| {
                                    this.stop_dag(cx);
                                });
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_dag_node(&self, node: &DagNode, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let view = cx.entity();
        let node_id = node.id.clone();
        let is_selected = self.selected_node_id.as_deref() == Some(node.id.as_str());
        let (method, req_name, found) = self.resolve_request_info(node);
        let status = self
            .node_statuses
            .get(&node.id)
            .cloned()
            .unwrap_or(DagNodeRunStatus::Idle);

        let border_color = match &status {
            DagNodeRunStatus::Running => cx.theme().primary,
            DagNodeRunStatus::Success { .. } => gpui_kit::rgb(0x22c55e).into(),
            DagNodeRunStatus::Failed(_) => cx.theme().danger,
            _ if is_selected => cx.theme().primary,
            _ => cx.theme().border,
        };

        let nid_drag = node_id.clone();
        let nid_up = node_id.clone();
        let nid_out = node_id.clone();
        let nid_in = node_id.clone();
        let nid_del = node_id.clone();
        let pid_open = node.project_id.clone();
        let rid_open = node.request_id.clone();

        let v_drag = view.clone();
        let v_up = view.clone();
        let v_out = view.clone();
        let v_in = view.clone();
        let v_del = view.clone();
        let v_open = view.clone();

        div()
            .id(SharedString::from(format!("dag-node-{}", node.id)))
            .absolute()
            .left(px(node.x))
            .top(px(node.y))
            .w(px(NODE_WIDTH))
            .h(px(NODE_HEIGHT))
            .rounded(cx.theme().radius)
            .bg(cx.theme().popover)
            .border_2()
            .border_color(border_color)
            .shadow_sm()
            .px_3()
            .py_1p5()
            .flex()
            .flex_col()
            .justify_between()
            .cursor_grab()
            .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                if event.click_count >= 2 && !pid_open.is_empty() {
                    v_open.update(cx, |_, cx| {
                        cx.emit(PackageTabEvent::OpenRequest {
                            project_id: pid_open.clone(),
                            request_id: rid_open.clone(),
                        });
                    });
                    return;
                }
                v_drag.update(cx, |this, cx| {
                    this.selected_node_id = Some(nid_drag.clone());
                    this.selected_edge = None;
                    let origin = this.canvas_bounds.origin;
                    if let Some(n) = this.set.dag.nodes.iter().find(|n| n.id == nid_drag) {
                        let lx = f32::from(event.position.x - origin.x);
                        let ly = f32::from(event.position.y - origin.y);
                        this.dragging_node = Some(NodeDragState {
                            node_id: nid_drag.clone(),
                            offset_x: lx - n.x,
                            offset_y: ly - n.y,
                        });
                    }
                    cx.notify();
                });
            })
            .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                v_up.update(cx, |this, cx| {
                    if let Some(conn) = this.connecting_from.take() {
                        this.try_connect_nodes(&conn.from_node_id, &nid_up, window, cx);
                    }
                    if this.dragging_node.take().is_some() {
                        this.sync_yaml_from_dag(window, cx);
                        cx.emit(PackageTabEvent::DirtyChanged);
                        cx.notify();
                    }
                });
            })
            // Header row: method badge + name + delete button
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_1p5()
                    .child(method_badge(method, true))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_xs()
                            .font_semibold()
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .text_color(if found {
                                cx.theme().foreground
                            } else {
                                cx.theme().danger
                            })
                            .child(req_name),
                    )
                    .child(
                        Button::new(SharedString::from(format!("dag-node-del-{}", node.id)))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(IconName::Close))
                            .on_click(move |_, window, cx| {
                                cx.stop_propagation();
                                v_del.update(cx, |this, cx| {
                                    this.delete_node(&nid_del, window, cx);
                                });
                            }),
                    ),
            )
            // Status row
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .justify_between()
                    .gap_1()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(match &status {
                        DagNodeRunStatus::Idle => div().child("Ready").into_any_element(),
                        DagNodeRunStatus::Pending => div().child("Pending…").into_any_element(),
                        DagNodeRunStatus::Running => div().child("Running…").into_any_element(),
                        DagNodeRunStatus::Success {
                            status_code,
                            duration_ms,
                        } => div()
                            .text_color(gpui_kit::rgb(0x16a34a))
                            .child(format!("{status_code} · {duration_ms}ms"))
                            .into_any_element(),
                        DagNodeRunStatus::Failed(err) => {
                            let err_msg = err.clone();
                            h_flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .text_color(cx.theme().danger)
                                        .font_medium()
                                        .child("Failed"),
                                )
                                .child(
                                    div()
                                        .id(SharedString::from(format!("dag-node-warn-{}", node.id)))
                                        .flex()
                                        .items_center()
                                        .cursor_pointer()
                                        .tooltip(move |window, cx| {
                                            let msg = err_msg.clone();
                                            Tooltip::element(move |_, _| {
                                                div()
                                                    .max_w(px(320.))
                                                    .whitespace_normal()
                                                    .text_xs()
                                                    .child(msg.clone())
                                            })
                                            .build(window, cx)
                                        })
                                        .child(
                                            Icon::new(IconName::TriangleAlert)
                                                .size(px(14.))
                                                .text_color(cx.theme().warning),
                                        ),
                                )
                                .into_any_element()
                        }
                        DagNodeRunStatus::Skipped => div().child("Skipped").into_any_element(),
                    }),
            )
            // Input Port (left)
            .child(
                div()
                    .id(SharedString::from(format!("dag-in-{}", node.id)))
                    .absolute()
                    .left(px(-7.0))
                    .top(px(NODE_HEIGHT * 0.5 - 7.0))
                    .w(px(14.))
                    .h(px(14.))
                    .rounded_full()
                    .bg(cx.theme().background)
                    .border_2()
                    .border_color(cx.theme().primary)
                    .cursor_pointer()
                    .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                        cx.stop_propagation();
                        v_in.update(cx, |this, cx| {
                            if let Some(conn) = this.connecting_from.take() {
                                this.try_connect_nodes(&conn.from_node_id, &nid_in, window, cx);
                            }
                        });
                    }),
            )
            // Output Port (right)
            .child(
                div()
                    .id(SharedString::from(format!("dag-out-{}", node.id)))
                    .absolute()
                    .right(px(-7.0))
                    .top(px(NODE_HEIGHT * 0.5 - 7.0))
                    .w(px(14.))
                    .h(px(14.))
                    .rounded_full()
                    .bg(cx.theme().primary)
                    .border_2()
                    .border_color(cx.theme().background)
                    .cursor_crosshair()
                    .on_mouse_down(MouseButton::Left, move |event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        v_out.update(cx, |this, cx| {
                            let origin = this.canvas_bounds.origin;
                            this.connecting_from = Some(ConnectingState {
                                from_node_id: nid_out.clone(),
                                cursor_x: f32::from(event.position.x - origin.x),
                                cursor_y: f32::from(event.position.y - origin.y),
                            });
                            cx.notify();
                        });
                    }),
            )
            .into_any_element()
    }

    fn render_yaml_pane(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        v_flex()
            .flex_1()
            .w_full()
            .h_full()
            .gap_2()
            .p_3()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Edit the DAG YAML directly. Changes sync to the Draw Pane when valid (cycles are rejected)."),
                    )
                    .child(match &self.yaml_error {
                        Some(err) => Tag::new()
                            .with_variant(TagVariant::Danger)
                            .child(err.clone())
                            .into_any_element(),
                        None => Tag::new()
                            .with_variant(TagVariant::Success)
                            .child("Valid DAG")
                            .into_any_element(),
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .w_full()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .overflow_hidden()
                    .child(Editor::new(&self.yaml_editor).h_full().w_full()),
            )
            .into_any_element()
    }
    fn render_console_panel(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        let view_toggle = cx.entity();
        let view_clear = cx.entity();
        let collapsed = self.console_collapsed;
        let log_count = self.console_logs.len();

        v_flex()
            .w_full()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .child(
                h_flex()
                    .w_full()
                    .px_3()
                    .py_1()
                    .items_center()
                    .justify_between()
                    .bg(cx.theme().secondary)
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1p5()
                            .child(
                                Button::new("dag-console-toggle-btn")
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(if collapsed {
                                        IconName::ChevronRight
                                    } else {
                                        IconName::ChevronDown
                                    }))
                                    .on_click(move |_, _, cx| {
                                        view_toggle.update(cx, |this, cx| {
                                            this.console_collapsed = !this.console_collapsed;
                                            cx.notify();
                                        });
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child(format!("Console ({log_count})")),
                            ),
                    )
                    .child(
                        Button::new("dag-console-clear-btn")
                            .ghost()
                            .xsmall()
                            .label("Clear")
                            .disabled(self.console_logs.is_empty())
                            .on_click(move |_, _, cx| {
                                view_clear.update(cx, |this, cx| {
                                    this.console_logs.clear();
                                    cx.notify();
                                });
                            }),
                    ),
            )
            .when(!collapsed, |panel| {
                panel.child(
                    div()
                        .id("dag-console-scroll")
                        .w_full()
                        .h(px(150.))
                        .overflow_y_scrollbar()
                        .p_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .when(self.console_logs.is_empty(), |list| {
                            list.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("No execution logs yet. Click Play to run the DAG."),
                            )
                        })
                        .children(self.console_logs.iter().map(|entry| {
                            let color = match entry.level {
                                ConsoleLogLevel::Info => cx.theme().muted_foreground,
                                ConsoleLogLevel::Success => gpui_kit::rgb(0x16a34a).into(),
                                ConsoleLogLevel::Error => cx.theme().danger,
                            };
                            div()
                                .w_full()
                                .text_xs()
                                .whitespace_normal()
                                .text_color(color)
                                .child(entry.message.clone())
                        })),
                )
            })
            .into_any_element()
    }
}

fn set_contains_request(set: &RequestSet, project_id: &str, request_id: &str) -> bool {
    fn folder_contains(folder: &RequestSetFolder, project_id: &str, request_id: &str) -> bool {
        folder
            .requests
            .iter()
            .any(|r| r.project_id == project_id && r.request_id == request_id)
            || folder
                .folders
                .iter()
                .any(|f| folder_contains(f, project_id, request_id))
    }
    set.requests
        .iter()
        .any(|r| r.project_id == project_id && r.request_id == request_id)
        || set
            .folders
            .iter()
            .any(|f| folder_contains(f, project_id, request_id))
}

fn paint_arrow_curve(
    window: &mut Window,
    p1: Point<Pixels>,
    p2: Point<Pixels>,
    color: gpui_kit::Hsla,
) {
    let dx = (f32::from(p2.x - p1.x).abs() * 0.5).max(40.0);
    let c1 = gpui_kit::point(px(f32::from(p1.x) + dx), p1.y);
    let c2 = gpui_kit::point(px(f32::from(p2.x) - dx), p2.y);

    let mut stroke = PathBuilder::stroke(px(2.0));
    stroke.move_to(p1);
    stroke.cubic_bezier_to(p2, c1, c2);
    if let Ok(path) = stroke.build() {
        window.paint_path(path, color);
    }

    // Arrowhead at p2 pointing along tangent from c2 -> p2
    let tx = f32::from(p2.x - c2.x);
    let ty = f32::from(p2.y - c2.y);
    let len = (tx * tx + ty * ty).sqrt().max(0.001);
    let ux = tx / len;
    let uy = ty / len;
    let arrow_len = 9.0_f32;
    let arrow_half = 4.5_f32;
    let tip_x = f32::from(p2.x);
    let tip_y = f32::from(p2.y);
    let base_x = tip_x - ux * arrow_len;
    let base_y = tip_y - uy * arrow_len;
    let left = gpui_kit::point(px(base_x - uy * arrow_half), px(base_y + ux * arrow_half));
    let right = gpui_kit::point(px(base_x + uy * arrow_half), px(base_y - ux * arrow_half));

    let mut head = PathBuilder::fill();
    head.move_to(p2);
    head.line_to(left);
    head.line_to(right);
    head.line_to(p2);
    if let Ok(path) = head.build() {
        window.paint_path(path, color);
    }
}

impl Render for PackageTabView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let active_ix = match self.active_section {
            PackageSection::Dag => 0,
            PackageSection::Yaml => 1,
        };
        let is_dirty = self.is_dirty();

        v_flex()
            .w_full()
            .h_full()
            .bg(cx.theme().background)
            .child(
                h_flex()
                    .w_full()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(LucideIcon::Layers)
                                    .size(px(16.))
                                    .text_color(cx.theme().primary),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .child(self.set.name.clone()),
                            )
                            .when(is_dirty, |row| {
                                row.child(Tag::new().with_variant(TagVariant::Warning).child("Modified"))
                            }),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                TabBar::new("pkg-section-tabs")
                                    .segmented()
                                    .small()
                                    .selected_index(active_ix)
                                    .on_click(cx.listener(|this, ix: &usize, window, cx| {
                                        this.active_section = match ix {
                                            0 => PackageSection::Dag,
                                            _ => {
                                                this.sync_yaml_from_dag(window, cx);
                                                PackageSection::Yaml
                                            }
                                        };
                                        cx.notify();
                                    }))
                                    .child(Tab::new().label("DAG"))
                                    .child(Tab::new().label("YAML")),
                            )
                            .child(
                                Button::new("pkg-save-btn")
                                    .primary()
                                    .small()
                                    .icon(Icon::new(LucideIcon::Save))
                                    .label("Save")
                                    .disabled(!is_dirty || self.yaml_error.is_some())
                                    .on_click(move |_, _, cx| {
                                        view.update(cx, |this, cx| {
                                            this.save(cx);
                                        });
                                    }),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(match self.active_section {
                        PackageSection::Dag => self.render_draw_pane(cx),
                        PackageSection::Yaml => self.render_yaml_pane(cx),
                    }),
            )
            .child(self.render_console_panel(cx))
    }
}
