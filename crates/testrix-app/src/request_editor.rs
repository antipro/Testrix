use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
};

use gpui_kit::{
    App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, IntoElement,
    ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Subscription, Window,
    assets::IconName as LucideIcon,
    component::{
        ActiveTheme as _, Icon, IconName, IndexPath, Sizable as _, StyledExt as _, WindowExt as _,
        alert::Alert,
        button::{Button, ButtonVariants as _},
        form::{Field, Form},
        h_flex,
        input::{Editor, EditorState, Input, InputEvent, InputState, Textarea, TextareaState},
        resizable::{resizable_panel, v_resizable},
        scroll::ScrollableElement as _,
        select::{Select, SelectEvent, SelectItem, SelectState},
        spinner::Spinner,
        switch::Switch,
        tab::{Tab, TabBar},
        tag::{Tag, TagVariant},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};
use testrix_core::{
    AuthNodeRef, CodegenTarget, EnvironmentDoc, FolderNode, HttpMethod, HttpRequestDoc,
    HttpResponseSnapshot, KeyValueRow, ProjectNode, RawBodyFormat, RequestBody,
    RequestExecutionState, RequestNode, RequestSettings, ResponseExample, ResponseMetaField,
    detect_body_language, extract_and_merge_response_meta, extract_url_query_params,
    generate_id, generate_request_code, materialize_request_doc, rename_template_variables,
    resolve_request_doc_auth,
};
use testrix_http::{CancellationToken, HttpError, HttpRunner};
use testrix_storage::to_yaml_pretty;

use crate::{
    auth_section::{AuthEditor, AuthEditorEvent},
    editable_combobox::{ComboboxOption, EditableCombobox, EditableComboboxEvent},
    kv_editor::{KeyValueTable, MultipartTable, TableChangeEvent, common_http_header_options},
    post_script::run_post_script,
    theme_helpers::{apply_unresolved_var_border, format_bytes, method_color, status_badge},
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct MethodOption(HttpMethod);

impl SelectItem for MethodOption {
    type Value = HttpMethod;

    fn title(&self) -> SharedString {
        self.0.as_str().into()
    }

    fn display_title(&self) -> Option<gpui_kit::AnyElement> {
        let method = self.0;
        Some(
            div()
                .font_semibold()
                .text_xs()
                .child(method.as_str())
                .into_any_element(),
        )
    }

    fn value(&self) -> &Self::Value {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestSection {
    Params,
    Headers,
    Body,
    Auth,
    PostScript,
    YamlSpec,
    Settings,
    Code,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponseSection {
    Body,
    Headers,
    SentRequest,
    Examples,
    MetaInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponseBodyFormat {
    Auto,
    Json,
    Xml,
    Html,
    Yaml,
    Text,
}

impl ResponseBodyFormat {
    const ALL: [Self; 6] = [
        Self::Auto,
        Self::Json,
        Self::Xml,
        Self::Html,
        Self::Yaml,
        Self::Text,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Json => "JSON",
            Self::Xml => "XML",
            Self::Html => "HTML",
            Self::Yaml => "YAML",
            Self::Text => "Text",
        }
    }

    fn language(self, snapshot: &HttpResponseSnapshot) -> &'static str {
        match self {
            Self::Auto => snapshot.editor_language(),
            Self::Json => "json",
            Self::Xml | Self::Html => "html",
            Self::Yaml => "yaml",
            Self::Text => "text",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyMode {
    None,
    Json,
    Raw,
    FormUrlEncoded,
    Multipart,
    Binary,
}

impl BodyMode {
    const ALL: [Self; 6] = [
        Self::None,
        Self::Json,
        Self::Raw,
        Self::FormUrlEncoded,
        Self::Multipart,
        Self::Binary,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Json => "JSON",
            Self::Raw => "Raw",
            Self::FormUrlEncoded => "x-www-form-urlencoded",
            Self::Multipart => "Multipart",
            Self::Binary => "Binary",
        }
    }

    fn from_body(body: &RequestBody) -> Self {
        match body {
            RequestBody::None => Self::None,
            RequestBody::Json { .. } => Self::Json,
            RequestBody::Raw { .. } => Self::Raw,
            RequestBody::FormUrlEncoded { .. } => Self::FormUrlEncoded,
            RequestBody::Multipart { .. } => Self::Multipart,
            RequestBody::Binary { .. } => Self::Binary,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum RequestTabEvent {
    SaveRequest {
        project_id: String,
        request_id: String,
        doc: Box<HttpRequestDoc>,
    },
    OpenAuthSource {
        project_id: String,
        node: AuthNodeRef,
    },
    ApplyPostScriptEnv {
        env_id: String,
        vars: BTreeMap<String, String>,
    },
    ResponseCompleted {
        request_id: String,
        snapshot: Box<HttpResponseSnapshot>,
    },
    DirtyChanged,
}

/// Center workbench view for inspecting, editing, and executing a single HTTP request YAML document.
pub(crate) struct RequestTabView {
    project_id: String,
    request_id: String,
    parent_folder_ids: Vec<String>,
    file_path: PathBuf,
    saved_doc: HttpRequestDoc,
    project_snapshot: ProjectNode,
    active_env: Option<EnvironmentDoc>,
    runner: HttpRunner,

    // Request controls
    method: HttpMethod,
    method_select: Entity<SelectState<Vec<MethodOption>>>,
    url_prefix_combo: Entity<EditableCombobox>,
    url_input: Entity<InputState>,
    name_input: Entity<InputState>,
    description_input: Entity<TextareaState>,
    params_table: Entity<KeyValueTable>,
    headers_table: Entity<KeyValueTable>,

    // Body controls
    body_mode: BodyMode,
    raw_format: RawBodyFormat,
    json_editor: Entity<EditorState>,
    raw_editor: Entity<EditorState>,
    form_table: Entity<KeyValueTable>,
    multipart_table: Entity<MultipartTable>,
    binary_path_input: Entity<InputState>,
    binary_ct_input: Entity<InputState>,

    // Auth + Post-Script + YAML + Settings controls
    auth_editor: Entity<AuthEditor>,
    post_script_editor: Entity<EditorState>,
    post_script_status: Option<Result<String, String>>,
    yaml_editor: Entity<EditorState>,
    code_target: CodegenTarget,
    code_editor: Entity<EditorState>,
    timeout_input: Entity<InputState>,
    follow_redirects: bool,
    verify_tls: bool,

    // Response state
    active_request_section: RequestSection,
    active_response_section: ResponseSection,
    response_body_format: ResponseBodyFormat,
    execution: RequestExecutionState,
    response_body_editor: Entity<EditorState>,
    show_secret_headers: bool,
    active_cancel: Option<CancellationToken>,
    request_revision: u64,

    // Saved response examples & extracted meta tree
    response_examples: Vec<ResponseExample>,
    selected_example_id: Option<String>,
    example_name_input: Entity<InputState>,
    example_body_editor: Entity<EditorState>,
    syncing_example_ui: bool,
    response_meta: Vec<ResponseMetaField>,
    collapsed_meta_paths: HashSet<String>,
    meta_desc_inputs: BTreeMap<String, Entity<InputState>>,
    meta_desc_subscriptions: Vec<Subscription>,

    breadcrumb: SharedString,
    dirty: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<RequestTabEvent> for RequestTabView {}

impl RequestTabView {
    pub(crate) fn new(
        project: &ProjectNode,
        parent_folders: &[&FolderNode],
        node: &RequestNode,
        active_env: Option<EnvironmentDoc>,
        runner: HttpRunner,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let doc = &node.doc;
        let file_path = node.file_path.clone();
        let method_options: Vec<MethodOption> =
            HttpMethod::ALL.into_iter().map(MethodOption).collect();
        let initial_method_ix = HttpMethod::ALL
            .iter()
            .position(|m| *m == doc.request.method)
            .map(|row| IndexPath::default().row(row));
        let method_select =
            cx.new(|cx| SelectState::new(method_options, initial_method_ix, window, cx));

        let (initial_prefix, initial_path) = split_url_prefix(&doc.request.url);
        let prefix_placeholder = if project.doc.base_url.trim().is_empty() {
            "{{base_url}}".to_string()
        } else {
            project.doc.base_url.clone()
        };
        let prefix_options = build_url_prefix_options(project, active_env.as_ref());
        let url_prefix_combo = cx.new(|cx| {
            let mut combo = EditableCombobox::new(
                format!("req-url-prefix-{}", doc.id),
                prefix_placeholder,
                initial_prefix,
                prefix_options,
                window,
                cx,
            )
            .cleanable(true)
            .menu_width(px(320.))
            .validate_variables(true);
            combo.set_active_env(active_env.clone(), cx);
            combo
        });
        let url_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("/v1/resource or path")
                .default_value(initial_path)
        });
        let name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Request name")
                .default_value(doc.name.clone())
        });
        let description_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .rows(3)
                .placeholder("Describe this endpoint…")
                .default_value(doc.description.clone())
        });

        let params_table = cx.new(|cx| {
            KeyValueTable::new(
                format!("req-params-{}", doc.id),
                "Parameter",
                "Value",
                "Add Param",
                &doc.request.params,
                window,
                cx,
            )
            .with_variable_validation(active_env.clone())
        });
        let headers_table = cx.new(|cx| {
            KeyValueTable::new(
                format!("req-headers-{}", doc.id),
                "Header",
                "Value",
                "Add Header",
                &doc.request.headers,
                window,
                cx,
            )
            .with_name_suggestions(common_http_header_options(), window, cx)
            .with_variable_validation(active_env.clone())
        });

        let body_mode = BodyMode::from_body(&doc.request.body);
        let (json_text, raw_format, raw_text, form_rows, multipart_rows, bin_path, bin_ct) =
            extract_body_fields(&doc.request.body);

        let json_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("json")
                .folding(true)
                .default_value(json_text)
        });
        let raw_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language(raw_format.editor_language())
                .folding(true)
                .default_value(raw_text)
        });
        let form_table = cx.new(|cx| {
            KeyValueTable::new(
                format!("req-form-{}", doc.id),
                "Field",
                "Value",
                "Add Form Field",
                &form_rows,
                window,
                cx,
            )
            .with_variable_validation(active_env.clone())
        });
        let multipart_table = cx.new(|cx| {
            MultipartTable::new(format!("req-mp-{}", doc.id), &multipart_rows, window, cx)
                .with_variable_validation(active_env.clone())
        });
        let binary_path_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("/path/to/payload.bin")
                .default_value(bin_path)
        });
        let binary_ct_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("application/octet-stream")
                .default_value(bin_ct)
        });

        let resolved_auth = resolve_request_doc_auth(project, parent_folders, doc);
        let auth_editor = cx.new(|cx| {
            let mut editor = AuthEditor::new(
                format!("req-auth-{}", doc.id),
                true,
                &doc.request.auth,
                window,
                cx,
            );
            editor.set_context(Some(resolved_auth), active_env.clone(), cx);
            editor
        });

        let post_script_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("javascript")
                .folding(true)
                .default_value(doc.request.post_script.clone())
        });

        let yaml_text = to_yaml_pretty(doc).unwrap_or_default();
        let yaml_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("yaml")
                .folding(true)
                .default_value(yaml_text)
        });
        let code_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("text")
                .folding(true)
                .default_value("")
        });

        let timeout_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("30000")
                .default_value(doc.request.settings.timeout_ms.to_string())
        });

        let response_body_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("json")
                .folding(true)
                .default_value("")
        });

        let response_examples = doc.request.response_examples.clone();
        let selected_example_id = response_examples.first().map(|ex| ex.id.clone());
        let example_name_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Example name")
                .default_value("")
        });
        let example_body_editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .language("json")
                .folding(true)
                .default_value("")
        });
        let response_meta = doc.request.response_meta.clone();

        let breadcrumb = build_breadcrumb(project, parent_folders, &doc.name);
        let parent_folder_ids = parent_folders.iter().map(|f| f.doc.id.clone()).collect();

        let mut view = Self {
            project_id: project.doc.id.clone(),
            request_id: doc.id.clone(),
            parent_folder_ids,
            file_path,
            saved_doc: doc.clone(),
            project_snapshot: project.clone(),
            active_env,
            runner,
            method: doc.request.method,
            method_select,
            url_prefix_combo,
            url_input,
            name_input,
            description_input,
            params_table,
            headers_table,
            body_mode,
            raw_format,
            json_editor,
            raw_editor,
            form_table,
            multipart_table,
            binary_path_input,
            binary_ct_input,
            auth_editor,
            post_script_editor,
            post_script_status: None,
            yaml_editor,
            code_target: CodegenTarget::Curl,
            code_editor,
            timeout_input,
            follow_redirects: doc.request.settings.follow_redirects,
            verify_tls: doc.request.settings.verify_tls,
            active_request_section: RequestSection::Params,
            active_response_section: ResponseSection::Body,
            response_body_format: ResponseBodyFormat::Auto,
            execution: RequestExecutionState::Idle,
            response_body_editor,
            show_secret_headers: false,
            active_cancel: None,
            request_revision: 0,
            response_examples,
            selected_example_id,
            example_name_input,
            example_body_editor,
            syncing_example_ui: false,
            response_meta,
            collapsed_meta_paths: HashSet::new(),
            meta_desc_inputs: BTreeMap::new(),
            meta_desc_subscriptions: Vec::new(),
            breadcrumb,
            dirty: false,
            _subscriptions: Vec::new(),
        };
        view.wire_subscriptions(window, cx);
        view.sync_selected_example_ui(window, cx);
        view.sync_meta_desc_inputs(window, cx);
        view.refresh_code_preview(window, cx);
        view
    }

    fn wire_subscriptions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut subs = Vec::new();
        subs.push(cx.subscribe_in(
            &self.method_select,
            window,
            |this, _, event: &SelectEvent<Vec<MethodOption>>, window, cx| {
                if let SelectEvent::Confirm(Some(method)) = event {
                    this.method = *method;
                    this.on_edited(window, cx);
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &self.url_prefix_combo,
            window,
            |this, _, _: &EditableComboboxEvent, window, cx| {
                this.on_edited(window, cx);
            },
        ));
        subs.push(cx.subscribe_in(
            &self.url_input,
            window,
            |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => this.on_url_input_changed(window, cx),
                InputEvent::PressEnter { .. } => this.send_request(window, cx),
                _ => {}
            },
        ));

        for input in [
            &self.name_input,
            &self.binary_path_input,
            &self.binary_ct_input,
            &self.timeout_input,
        ] {
            subs.push(
                cx.subscribe_in(input, window, |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.on_edited(window, cx);
                    }
                }),
            );
        }

        subs.push(cx.subscribe_in(
            &self.description_input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.on_edited(window, cx);
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &self.json_editor,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.on_edited(window, cx);
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &self.raw_editor,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.on_edited(window, cx);
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &self.post_script_editor,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) {
                    this.on_edited(window, cx);
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &self.params_table,
            window,
            |this, _, _: &TableChangeEvent, window, cx| this.on_edited(window, cx),
        ));
        subs.push(cx.subscribe_in(
            &self.headers_table,
            window,
            |this, _, _: &TableChangeEvent, window, cx| this.on_edited(window, cx),
        ));
        subs.push(cx.subscribe_in(
            &self.form_table,
            window,
            |this, _, _: &TableChangeEvent, window, cx| this.on_edited(window, cx),
        ));
        subs.push(cx.subscribe_in(
            &self.multipart_table,
            window,
            |this, _, _: &TableChangeEvent, window, cx| this.on_edited(window, cx),
        ));
        subs.push(cx.subscribe_in(
            &self.auth_editor,
            window,
            |this, _, event: &AuthEditorEvent, window, cx| match event {
                AuthEditorEvent::Changed => this.on_edited(window, cx),
                AuthEditorEvent::OpenSourceNode(node) => {
                    cx.emit(RequestTabEvent::OpenAuthSource {
                        project_id: this.project_id.clone(),
                        node: node.clone(),
                    });
                }
            },
        ));
        subs.push(cx.subscribe_in(
            &self.example_name_input,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::Change) && !this.syncing_example_ui {
                    this.on_example_name_changed(window, cx);
                }
            },
        ));

        self._subscriptions = subs;
    }

    pub(crate) fn request_id(&self) -> &str {
        &self.request_id
    }

    pub(crate) fn project_id(&self) -> &str {
        &self.project_id
    }

    pub(crate) fn set_location(&mut self, project_id: String, file_path: PathBuf) {
        self.project_id = project_id;
        self.file_path = file_path;
    }

    pub(crate) fn method(&self) -> HttpMethod {
        self.method
    }

    pub(crate) fn title(&self, cx: &App) -> String {
        let current = self.name_input.read(cx).value().trim().to_string();
        if current.is_empty() {
            self.saved_doc.name.clone()
        } else {
            current
        }
    }

    pub(crate) fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub(crate) fn is_loading(&self) -> bool {
        matches!(self.execution, RequestExecutionState::Loading { .. })
    }

    /// Updates project/environment context when parent auth, base URL, or active profile changes.
    pub(crate) fn sync_context(
        &mut self,
        project: &ProjectNode,
        active_env: Option<EnvironmentDoc>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.project_snapshot = project.clone();
        self.active_env = active_env.clone();

        let (ancestors, updated_file_path) = match project.find_request(&self.request_id) {
            Some((ancestors, req_node)) => {
                self.parent_folder_ids = ancestors.iter().map(|f| f.doc.id.clone()).collect();
                (ancestors, Some(req_node.file_path.clone()))
            }
            None => (Vec::new(), None),
        };
        if let Some(path) = updated_file_path {
            self.file_path = path;
        }

        let current_doc = self.build_doc(cx);
        self.breadcrumb = build_breadcrumb(project, &ancestors, &current_doc.name);
        let resolved_auth = resolve_request_doc_auth(project, &ancestors, &current_doc);
        let prefix_options = build_url_prefix_options(project, self.active_env.as_ref());
        let prefix_placeholder = if project.doc.base_url.trim().is_empty() {
            "{{base_url}}".to_string()
        } else {
            project.doc.base_url.clone()
        };
        self.url_prefix_combo.update(cx, |combo, cx| {
            combo.set_options(prefix_options, cx);
            combo.set_placeholder(prefix_placeholder, window, cx);
            combo.set_active_env(active_env.clone(), cx);
        });
        self.params_table
            .update(cx, |table, cx| table.set_active_env(active_env.clone(), cx));
        self.headers_table
            .update(cx, |table, cx| table.set_active_env(active_env.clone(), cx));
        self.form_table
            .update(cx, |table, cx| table.set_active_env(active_env.clone(), cx));
        self.multipart_table
            .update(cx, |table, cx| table.set_active_env(active_env.clone(), cx));
        self.auth_editor.update(cx, |editor, cx| {
            editor.set_context(Some(resolved_auth), active_env, cx);
        });
        self.refresh_yaml_preview(window, cx);
        self.refresh_code_preview(window, cx);
        cx.notify();
    }

    pub(crate) fn mark_saved(&mut self, cx: &mut Context<Self>) {
        self.saved_doc = self.build_doc(cx);
        self.dirty = false;
        cx.emit(RequestTabEvent::DirtyChanged);
        cx.notify();
    }

    pub(crate) fn restore_response(
        &mut self,
        snapshot: HttpResponseSnapshot,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let lang = self.response_body_format.language(&snapshot);
        let pretty = snapshot.pretty_body.clone();
        self.response_body_editor.update(cx, |editor, cx| {
            editor.set_highlighter(lang, cx);
            editor.set_value(pretty, window, cx);
        });
        self.execution = RequestExecutionState::Completed(Box::new(snapshot));
        cx.notify();
    }

    pub(crate) fn rename_variables(
        &mut self,
        renames: &[(String, String)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut changed = self
            .url_prefix_combo
            .update(cx, |combo, cx| combo.rename_variables(renames, window, cx));

        let url_val = self.url_input.read(cx).value().to_string();
        if let Some(updated) = rename_template_variables(&url_val, renames) {
            self.url_input
                .update(cx, |state, cx| state.set_value(updated, window, cx));
            changed = true;
        }

        changed |= self
            .params_table
            .update(cx, |t, cx| t.rename_variables(renames, window, cx));
        changed |= self
            .headers_table
            .update(cx, |t, cx| t.rename_variables(renames, window, cx));
        changed |= self
            .form_table
            .update(cx, |t, cx| t.rename_variables(renames, window, cx));
        changed |= self
            .multipart_table
            .update(cx, |t, cx| t.rename_variables(renames, window, cx));
        changed |= self
            .auth_editor
            .update(cx, |e, cx| e.rename_variables(renames, window, cx));

        for editor in [&self.json_editor, &self.raw_editor] {
            let current = editor.read(cx).value().to_string();
            if let Some(updated) = rename_template_variables(&current, renames) {
                editor.update(cx, |state, cx| state.set_value(updated, window, cx));
                changed = true;
            }
        }

        if changed {
            if !self.dirty {
                self.saved_doc = self.build_doc(cx);
            }
            self.refresh_yaml_preview(window, cx);
            cx.notify();
        }
        changed
    }

    pub(crate) fn replace_url_base(
        &mut self,
        old_base: &str,
        new_base: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let current_prefix = self.url_prefix_combo.read(cx).value(cx);
        let old_trimmed = old_base.trim().trim_end_matches('/');
        let new_trimmed = new_base.trim().trim_end_matches('/');
        if old_trimmed.is_empty()
            || new_trimmed.is_empty()
            || old_trimmed == new_trimmed
            || current_prefix.trim().trim_end_matches('/') != old_trimmed
        {
            return false;
        }
        self.url_prefix_combo.update(cx, |combo, cx| {
            combo.set_value(new_trimmed.to_string(), window, cx);
        });
        if !self.dirty {
            self.saved_doc = self.build_doc(cx);
        }
        self.refresh_yaml_preview(window, cx);
        self.refresh_code_preview(window, cx);
        cx.notify();
        true
    }

    pub(crate) fn save(&mut self, cx: &mut Context<Self>) {
        tracing::info!(
            project_id = %self.project_id,
            request_id = %self.request_id,
            dirty = self.dirty,
            "RequestTabView::save triggered"
        );
        let doc = self.build_doc(cx);
        cx.emit(RequestTabEvent::SaveRequest {
            project_id: self.project_id.clone(),
            request_id: self.request_id.clone(),
            doc: Box::new(doc),
        });
    }

    pub(crate) fn cancel_request(&mut self, cx: &mut Context<Self>) {
        if let Some(token) = self.active_cancel.take() {
            token.cancel();
            self.execution = RequestExecutionState::Cancelled;
            cx.notify();
        }
    }

    pub(crate) fn send_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(prev) = self.active_cancel.take() {
            prev.cancel();
        }

        let doc = self.build_doc(cx);
        let ancestors = self.resolve_ancestors();
        let prepared = match materialize_request_doc(
            &self.project_snapshot,
            &ancestors,
            &doc,
            self.active_env.as_ref(),
        ) {
            Ok(req) => req,
            Err(err) => {
                self.execution = RequestExecutionState::Failed {
                    message: err.to_string(),
                };
                cx.notify();
                return;
            }
        };

        self.request_revision += 1;
        let revision = self.request_revision;
        let cancel_token = CancellationToken::new();
        self.active_cancel = Some(cancel_token.clone());
        self.execution = RequestExecutionState::Loading {
            request_revision: revision,
        };
        cx.notify();

        let rx = self.runner.dispatch(prepared, cancel_token);
        cx.spawn_in(window, async move |weak_self, cx| {
            let outcome = rx.await;
            _ = weak_self.update_in(cx, |this, window, cx| {
                // Reject stale responses if user already cancelled or started a newer request.
                if this.request_revision != revision {
                    return;
                }
                this.active_cancel = None;
                match outcome {
                    Ok(Ok(snapshot)) => {
                        let lang = this.response_body_format.language(&snapshot);
                        let pretty = snapshot.pretty_body.clone();
                        this.response_body_editor.update(cx, |editor, cx| {
                            editor.set_highlighter(lang, cx);
                            editor.set_value(pretty, window, cx);
                        });
                        let script = this.post_script_editor.read(cx).value().to_string();
                        if !script.trim().is_empty() {
                            let req_name = this.name_input.read(cx).value().trim().to_string();
                            match run_post_script(
                                &script,
                                &this.request_id,
                                &req_name,
                                this.active_env.as_ref(),
                                &snapshot,
                            ) {
                                Ok(res) => {
                                    let count = res.changed_keys.len();
                                    if count > 0
                                        && let Some(env) = &this.active_env
                                    {
                                        cx.emit(RequestTabEvent::ApplyPostScriptEnv {
                                            env_id: env.id.clone(),
                                            vars: res.next_vars,
                                        });
                                    }
                                    this.post_script_status = Some(Ok(format!(
                                        "Post-script executed ({count} variable(s) updated)"
                                    )));
                                }
                                Err(err) => {
                                    let msg = err.to_string();
                                    this.post_script_status = Some(Err(msg.clone()));
                                    window.push_notification(
                                        format!("Post-script error: {msg}"),
                                        cx,
                                    );
                                }
                            }
                        } else {
                            this.post_script_status = None;
                        }
                        cx.emit(RequestTabEvent::ResponseCompleted {
                            request_id: this.request_id.clone(),
                            snapshot: Box::new(snapshot.clone()),
                        });
                        this.execution = RequestExecutionState::Completed(Box::new(snapshot));
                    }
                    Ok(Err(HttpError::Cancelled)) => {
                        this.execution = RequestExecutionState::Cancelled;
                    }
                    Ok(Err(err)) => {
                        this.execution = RequestExecutionState::Failed {
                            message: err.to_string(),
                        };
                    }
                    Err(_) => {
                        this.execution = RequestExecutionState::Failed {
                            message: "HTTP execution task terminated unexpectedly.".to_string(),
                        };
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn resolve_ancestors(&self) -> Vec<&FolderNode> {
        self.project_snapshot
            .find_request(&self.request_id)
            .map(|(ancestors, _)| ancestors)
            .unwrap_or_default()
    }

    fn on_url_input_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.url_input.read(cx).value().to_string();
        if let Some((clean_url, extracted)) = extract_url_query_params(&raw) {
            let mut rows = self.params_table.read(cx).snapshot(cx);
            for (key, value) in extracted {
                if let Some(existing) = rows.iter_mut().find(|r| r.enabled && r.name == key) {
                    existing.value = value;
                } else {
                    rows.push(KeyValueRow::new(key, value));
                }
            }
            self.params_table
                .update(cx, |table, cx| table.set_rows(&rows, window, cx));
            self.url_input
                .update(cx, |state, cx| state.set_value(clean_url, window, cx));
        }
        self.on_edited(window, cx);
    }

    fn on_edited(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = self.build_doc(cx);
        self.dirty = draft != self.saved_doc;

        let (breadcrumb, resolved_auth) = {
            let ancestors = self.resolve_ancestors();
            (
                build_breadcrumb(&self.project_snapshot, &ancestors, &draft.name),
                resolve_request_doc_auth(&self.project_snapshot, &ancestors, &draft),
            )
        };
        self.breadcrumb = breadcrumb;
        let active_env = self.active_env.clone();
        self.auth_editor.update(cx, |editor, cx| {
            editor.set_context(Some(resolved_auth), active_env, cx);
        });
        self.refresh_yaml_preview(window, cx);
        self.refresh_code_preview(window, cx);
        cx.emit(RequestTabEvent::DirtyChanged);
        cx.notify();
    }

    fn refresh_yaml_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let yaml = to_yaml_pretty(&self.build_doc(cx)).unwrap_or_default();
        self.yaml_editor.update(cx, |editor, cx| {
            editor.set_value(yaml, window, cx);
        });
    }

    fn refresh_code_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let doc = self.build_doc(cx);
        let ancestors = self.resolve_ancestors();
        let code = match materialize_request_doc(
            &self.project_snapshot,
            &ancestors,
            &doc,
            self.active_env.as_ref(),
        ) {
            Ok(prepared) => generate_request_code(&prepared, self.code_target),
            Err(err) => format!("// Unable to generate code: {err}"),
        };
        self.code_editor.update(cx, |editor, cx| {
            editor.set_value(code, window, cx);
        });
    }

    pub(crate) fn build_doc(&self, cx: &App) -> HttpRequestDoc {
        let mut doc = self.saved_doc.clone();
        let next_name = self.name_input.read(cx).value().trim().to_string();
        if !next_name.is_empty() {
            doc.name = next_name;
        }
        doc.description = self.description_input.read(cx).value().to_string();
        doc.request.method = self.method;
        doc.request.url = combine_url_prefix(
            &self.url_prefix_combo.read(cx).value(cx),
            &self.url_input.read(cx).value(),
        );
        doc.request.params = self.params_table.read(cx).snapshot(cx);
        doc.request.headers = self.headers_table.read(cx).snapshot(cx);
        doc.request.auth = self.auth_editor.read(cx).snapshot(cx);
        doc.request.post_script = self.post_script_editor.read(cx).value().to_string();
        doc.request.body = match self.body_mode {
            BodyMode::None => RequestBody::None,
            BodyMode::Json => RequestBody::Json {
                content: self.json_editor.read(cx).value().to_string(),
            },
            BodyMode::Raw => RequestBody::Raw {
                raw_format: self.raw_format,
                content: self.raw_editor.read(cx).value().to_string(),
            },
            BodyMode::FormUrlEncoded => RequestBody::FormUrlEncoded {
                fields: self.form_table.read(cx).snapshot(cx),
            },
            BodyMode::Multipart => RequestBody::Multipart {
                fields: self.multipart_table.read(cx).snapshot(cx),
            },
            BodyMode::Binary => {
                let ct = self.binary_ct_input.read(cx).value().trim().to_string();
                RequestBody::Binary {
                    file_path: self.binary_path_input.read(cx).value().to_string(),
                    content_type: if ct.is_empty() { None } else { Some(ct) },
                }
            }
        };

        let timeout_ms = self
            .timeout_input
            .read(cx)
            .value()
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|ms| *ms > 0)
            .unwrap_or(self.saved_doc.request.settings.timeout_ms.max(1));

        doc.request.settings =
            RequestSettings::new(timeout_ms, self.follow_redirects, self.verify_tls);
        doc.request.response_examples = self.response_examples.clone();
        doc.request.response_meta = self.response_meta.clone();
        doc
    }

    fn format_json_body(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.json_editor.read(cx).value().to_string();
        match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(parsed) => {
                if let Ok(pretty) = serde_json::to_string_pretty(&parsed) {
                    self.json_editor.update(cx, |editor, cx| {
                        editor.replace_all(pretty, window, cx);
                    });
                    self.on_edited(window, cx);
                }
            }
            Err(err) => {
                window.push_notification(format!("Invalid JSON: {err}"), cx);
            }
        }
    }

    fn render_header_and_url_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current_doc = self.build_doc(cx);
        let ancestors = self.resolve_ancestors();
        let resolved_auth =
            resolve_request_doc_auth(&self.project_snapshot, &ancestors, &current_doc);
        let preview = materialize_request_doc(
            &self.project_snapshot,
            &ancestors,
            &current_doc,
            self.active_env.as_ref(),
        );
        let loading = self.is_loading();
        let id_base = self.request_id.clone();

        v_flex()
            .w_full()
            .px_4()
            .py_2p5()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            // Breadcrumb & Auth summary strip
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .truncate()
                                    .child(self.breadcrumb.clone()),
                            )
                            .child(
                                Tag::new()
                                    .with_variant(if resolved_auth.is_inherited() {
                                        TagVariant::Info
                                    } else {
                                        TagVariant::Secondary
                                    })
                                    .outline()
                                    .small()
                                    .child(format!("Auth: {}", resolved_auth.summary())),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(
                                self.file_path
                                    .file_name()
                                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                            ),
                    ),
            )
            // Method + URL + Send + Save row
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(
                        div().w(px(116.)).flex_none().child(
                            Select::new(&self.method_select)
                                .id(SharedString::from(format!("req-method-{id_base}")))
                                .text_color(method_color(self.method, cx)),
                        ),
                    )
                    .child(
                        div()
                            .w(px(210.))
                            .flex_none()
                            .child(self.url_prefix_combo.clone()),
                    )
                    .child(
                        div().flex_1().min_w_0().child({
                            let url_text = self.url_input.read(cx).value().to_string();
                            apply_unresolved_var_border(
                                Input::new(&self.url_input)
                                    .id(SharedString::from(format!("req-url-{id_base}")))
                                    .w_full()
                                    .cleanable(true),
                                &url_text,
                                self.active_env.as_ref(),
                                cx,
                            )
                        }),
                    )
                    .child(
                        Button::new(SharedString::from(format!("req-send-{id_base}")))
                            .when(!loading, |b| {
                                b.primary()
                                    .icon(Icon::new(LucideIcon::SendHorizontal))
                                    .label(crate::i18n::t("send"))
                            })
                            .when(loading, |b| {
                                b.danger()
                                    .icon(Spinner::new())
                                    .label(crate::i18n::t("cancel_req"))
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.is_loading() {
                                    this.cancel_request(cx);
                                } else {
                                    this.send_request(window, cx);
                                }
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("req-save-{id_base}")))
                            .outline()
                            .icon(Icon::new(LucideIcon::Save))
                            .label(if self.dirty {
                                format!("{}*", crate::i18n::t("save"))
                            } else {
                                crate::i18n::t("save").to_string()
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.save(cx);
                            })),
                    ),
            )
            // Resolved URL / unresolved variables hint
            .child(match preview {
                Ok(prepared) => h_flex()
                    .w_full()
                    .gap_2()
                    .text_xs()
                    .child(
                        div()
                            .text_color(cx.theme().muted_foreground)
                            .child("Resolved:"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(cx.theme().foreground)
                            .child(prepared.url),
                    )
                    .when(!prepared.unresolved_variables.is_empty(), |row| {
                        row.child(
                            Tag::new()
                                .with_variant(TagVariant::Warning)
                                .outline()
                                .small()
                                .child(format!(
                                    "Unresolved: {}",
                                    prepared.unresolved_variables.join(", ")
                                )),
                        )
                    })
                    .into_any_element(),
                Err(err) => div()
                    .text_xs()
                    .text_color(cx.theme().warning)
                    .child(err.to_string())
                    .into_any_element(),
            })
    }

    fn render_request_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let doc = self.build_doc(cx);
        let param_count = doc.request.params.iter().filter(|r| r.enabled).count();
        let header_count = doc.request.headers.iter().filter(|r| r.enabled).count();
        let id_base = self.request_id.clone();

        let sections = [
            RequestSection::Params,
            RequestSection::Headers,
            RequestSection::Body,
            RequestSection::Auth,
            RequestSection::PostScript,
            RequestSection::YamlSpec,
            RequestSection::Settings,
            RequestSection::Code,
        ];
        let has_post_script = !self.post_script_editor.read(cx).value().trim().is_empty();
        let selected_ix = sections
            .iter()
            .position(|s| *s == self.active_request_section)
            .unwrap_or(0);

        v_flex()
            .size_full()
            .min_h_0()
            .child(
                TabBar::new(SharedString::from(format!("req-sections-{id_base}")))
                    .w_full()
                    .underline()
                    .small()
                    .px_4()
                    .selected_index(selected_ix)
                    .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                        if let Some(sec) = sections.get(*ix) {
                            this.active_request_section = *sec;
                            cx.notify();
                        }
                    }))
                    .child(Tab::new().label(if param_count > 0 {
                        format!("{} ({param_count})", crate::i18n::t("params"))
                    } else {
                        crate::i18n::t("params").to_string()
                    }))
                    .child(Tab::new().label(if header_count > 0 {
                        format!("{} ({header_count})", crate::i18n::t("headers"))
                    } else {
                        crate::i18n::t("headers").to_string()
                    }))
                    .child(Tab::new().label(format!(
                        "{} ({})",
                        crate::i18n::t("body"),
                        self.body_mode.label()
                    )))
                    .child(Tab::new().label(crate::i18n::t("auth")))
                    .child(Tab::new().label(if has_post_script {
                        format!("{} •", crate::i18n::t("post_script"))
                    } else {
                        crate::i18n::t("post_script").to_string()
                    }))
                    .child(Tab::new().label(crate::i18n::t("yaml_spec")))
                    .child(Tab::new().label(crate::i18n::t("settings")))
                    .child(Tab::new().label(crate::i18n::t("code"))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p_3()
                    .when(self.active_request_section == RequestSection::Params, |this| {
                        this.child(self.params_table.clone())
                    })
                    .when(self.active_request_section == RequestSection::Headers, |this| {
                        this.child(
                            v_flex()
                                .size_full()
                                .min_h_0()
                                .gap_2()
                                .when(!self.project_snapshot.doc.default_headers.is_empty(), |col| {
                                    let count = self
                                        .project_snapshot
                                        .doc
                                        .default_headers
                                        .iter()
                                        .filter(|h| h.enabled)
                                        .count();
                                    col.child(
                                        div()
                                            .px_2()
                                            .py_1()
                                            .rounded(cx.theme().radius)
                                            .bg(cx.theme().muted)
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!(
                                                "{count} default header(s) inherited from project \"{}\". Request headers override matching names.",
                                                self.project_snapshot.doc.name
                                            )),
                                    )
                                })
                                .child(div().flex_1().min_h_0().child(self.headers_table.clone())),
                        )
                    })
                    .when(self.active_request_section == RequestSection::Body, |this| {
                        this.child(self.render_body_editor(cx))
                    })
                    .when(self.active_request_section == RequestSection::Auth, |this| {
                        this.child(
                            div()
                                .size_full()
                                .overflow_y_scrollbar()
                                .child(self.auth_editor.clone()),
                        )
                    })
                    .when(self.active_request_section == RequestSection::PostScript, |this| {
                        this.child(self.render_post_script_pane(cx))
                    })
                    .when(self.active_request_section == RequestSection::YamlSpec, |this| {
                        this.child(self.render_yaml_spec_pane(cx))
                    })
                    .when(self.active_request_section == RequestSection::Settings, |this| {
                        this.child(self.render_settings_pane(cx))
                    })
                    .when(self.active_request_section == RequestSection::Code, |this| {
                        this.child(self.render_code_pane(cx))
                    }),
            )
    }

    fn render_body_editor(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let id_base = self.request_id.clone();
        let mode_ix = BodyMode::ALL
            .iter()
            .position(|m| *m == self.body_mode)
            .unwrap_or(0);

        v_flex()
            .size_full()
            .min_h_0()
            .gap_2()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_2()
                    .child(
                        TabBar::new(SharedString::from(format!("req-body-mode-{id_base}")))
                            .segmented()
                            .small()
                            .selected_index(mode_ix)
                            .on_click(cx.listener(|this, ix: &usize, window, cx| {
                                if let Some(mode) = BodyMode::ALL.get(*ix) {
                                    this.body_mode = *mode;
                                    this.on_edited(window, cx);
                                }
                            }))
                            .children(BodyMode::ALL.iter().map(|m| Tab::new().label(m.label()))),
                    )
                    .when(self.body_mode == BodyMode::Json, |row| {
                        row.child(
                            Button::new(SharedString::from(format!("format-json-{id_base}")))
                                .outline()
                                .xsmall()
                                .icon(Icon::new(LucideIcon::Sparkles))
                                .label(crate::i18n::t("format_json"))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.format_json_body(window, cx);
                                })),
                        )
                    })
                    .when(self.body_mode == BodyMode::Raw, |row| {
                        row.child(
                            h_flex()
                                .gap_1()
                                .children(RawBodyFormat::ALL.iter().map(|fmt| {
                                    let fmt = *fmt;
                                    Button::new(SharedString::from(format!(
                                        "raw-fmt-{id_base}-{}",
                                        fmt.label()
                                    )))
                                    .xsmall()
                                    .label(fmt.label())
                                    .when(self.raw_format == fmt, |b| b.primary())
                                    .when(self.raw_format != fmt, |b| b.outline())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.raw_format = fmt;
                                        this.raw_editor.update(cx, |ed, cx| {
                                            ed.set_highlighter(fmt.editor_language(), cx);
                                        });
                                        this.on_edited(window, cx);
                                    }))
                                }))
                                .child(
                                    Button::new(SharedString::from(format!(
                                        "raw-fmt-detect-{id_base}"
                                    )))
                                    .outline()
                                    .xsmall()
                                    .icon(Icon::new(LucideIcon::Sparkles))
                                    .label(crate::i18n::t("detect"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let text = this.raw_editor.read(cx).value().to_string();
                                        let detected = detect_body_language(&text, None);
                                        if detected == "json" {
                                            this.body_mode = BodyMode::Json;
                                            this.json_editor.update(cx, |ed, cx| {
                                                ed.set_value(text, window, cx);
                                            });
                                        } else {
                                            let next_fmt = match detected {
                                                "html" => {
                                                    let lower = text.trim().to_ascii_lowercase();
                                                    if lower.contains("<html")
                                                        || lower.starts_with("<!doctype html")
                                                    {
                                                        RawBodyFormat::Html
                                                    } else {
                                                        RawBodyFormat::Xml
                                                    }
                                                }
                                                "yaml" => RawBodyFormat::Yaml,
                                                "javascript" => RawBodyFormat::Javascript,
                                                _ => RawBodyFormat::Text,
                                            };
                                            this.raw_format = next_fmt;
                                            this.raw_editor.update(cx, |ed, cx| {
                                                ed.set_highlighter(next_fmt.editor_language(), cx);
                                            });
                                        }
                                        this.on_edited(window, cx);
                                    })),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when(self.body_mode == BodyMode::None, |this| {
                        this.child(
                            div()
                                .p_4()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("This request has no body payload."),
                        )
                    })
                    .when(self.body_mode == BodyMode::Json, |this| {
                        this.child(Editor::new(&self.json_editor).size_full())
                    })
                    .when(self.body_mode == BodyMode::Raw, |this| {
                        this.child(Editor::new(&self.raw_editor).size_full())
                    })
                    .when(self.body_mode == BodyMode::FormUrlEncoded, |this| {
                        this.child(self.form_table.clone())
                    })
                    .when(self.body_mode == BodyMode::Multipart, |this| {
                        this.child(self.multipart_table.clone())
                    })
                    .when(self.body_mode == BodyMode::Binary, |this| {
                        this.child(
                            Form::new()
                                .child(
                                    Field::new().label("Binary File Path").child(
                                        h_flex()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .child(Input::new(&self.binary_path_input).small()),
                                            )
                                            .child(
                                                Button::new(SharedString::from(format!(
                                                    "bin-browse-{id_base}"
                                                )))
                                                .outline()
                                                .small()
                                                .icon(Icon::new(LucideIcon::FolderOpen))
                                                .label("Browse…")
                                                .on_click(cx.listener(|_, _, window, cx| {
                                                    let prompt = cx.prompt_for_paths(PathPromptOptions {
                                                        files: true,
                                                        directories: false,
                                                        multiple: false,
                                                        prompt: Some("Select binary payload file".into()),
                                                    });
                                                    let entity = cx.entity();
                                                    cx.spawn_in(window, async move |_, window| {
                                                        let path = prompt.await.ok()?.ok()??.into_iter().next()?;
                                                        window
                                                            .update(|window, cx| {
                                                                entity.update(cx, |this, cx| {
                                                                    this.binary_path_input.update(cx, |input, cx| {
                                                                        input.set_value(path.to_string_lossy().to_string(), window, cx);
                                                                    });
                                                                    this.on_edited(window, cx);
                                                                });
                                                            })
                                                            .ok()
                                                    })
                                                    .detach();
                                                })),
                                            ),
                                    ),
                                )
                                .child(
                                    Field::new()
                                        .label("Content-Type")
                                        .child(Input::new(&self.binary_ct_input).small()),
                                ),
                        )
                    }),
            )
    }

    fn render_post_script_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .min_h_0()
            .gap_2()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Globals: environment, request, response (runs in gpui-shell after response)"),
                    )
                    .when_some(self.post_script_status.as_ref(), |row, status| match status {
                        Ok(msg) => row.child(
                            Tag::new()
                                .with_variant(TagVariant::Success)
                                .outline()
                                .xsmall()
                                .child(msg.clone()),
                        ),
                        Err(err) => row.child(
                            Tag::new()
                                .with_variant(TagVariant::Danger)
                                .outline()
                                .xsmall()
                                .child(err.clone()),
                        ),
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .overflow_hidden()
                    .child(Editor::new(&self.post_script_editor).size_full()),
            )
    }

    fn render_yaml_spec_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let id_base = self.request_id.clone();
        v_flex()
            .size_full()
            .min_h_0()
            .gap_2()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(format!("File: {}", self.file_path.display())),
                    )
                    .child(
                        Button::new(SharedString::from(format!("copy-yaml-{id_base}")))
                            .outline()
                            .xsmall()
                            .icon(IconName::Copy)
                            .label(crate::i18n::t("copy_yaml"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                let yaml = this.yaml_editor.read(cx).value().to_string();
                                cx.write_to_clipboard(ClipboardItem::new_string(yaml));
                                window.push_notification("YAML document copied to clipboard.", cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(Editor::new(&self.yaml_editor).size_full().readonly(true)),
            )
    }

    fn render_settings_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let id_base = self.request_id.clone();
        div().size_full().overflow_y_scrollbar().child(
            Form::new()
                .child(
                    Field::new()
                        .label("Request Name")
                        .child(Input::new(&self.name_input).small()),
                )
                .child(
                    Field::new()
                        .label("Timeout (ms)")
                        .child(Input::new(&self.timeout_input).small().w(px(160.))),
                )
                .child(
                    Field::new().label_indent(false).child(
                        Switch::new(SharedString::from(format!("follow-redir-{id_base}")))
                            .label("Follow HTTP redirects")
                            .checked(self.follow_redirects)
                            .on_change(cx.listener(|this, next, window, cx| {
                                this.follow_redirects = *next;
                                this.on_edited(window, cx);
                            })),
                    ),
                )
                .child(
                    Field::new().label_indent(false).child(
                        Switch::new(SharedString::from(format!("verify-tls-{id_base}")))
                            .label("Verify TLS certificates")
                            .checked(self.verify_tls)
                            .on_change(cx.listener(|this, next, window, cx| {
                                this.verify_tls = *next;
                                this.on_edited(window, cx);
                            })),
                    ),
                )
                .child(
                    Field::new()
                        .label("Description")
                        .child(Textarea::new(&self.description_input)),
                ),
        )
    }

    fn render_code_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let id_base = self.request_id.clone();
        v_flex()
            .size_full()
            .min_h_0()
            .gap_2()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .child(
                        h_flex().gap_1().children(CodegenTarget::ALL.iter().map(|target| {
                            let target = *target;
                            Button::new(SharedString::from(format!(
                                "code-target-{id_base}-{}",
                                target.label()
                            )))
                            .xsmall()
                            .label(target.label())
                            .when(self.code_target == target, |b| b.primary())
                            .when(self.code_target != target, |b| b.outline())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.code_target = target;
                                this.refresh_code_preview(window, cx);
                                cx.notify();
                            }))
                        })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("copy-code-{id_base}")))
                            .outline()
                            .xsmall()
                            .icon(IconName::Copy)
                            .label(crate::i18n::t("copy_code"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                let code = this.code_editor.read(cx).value().to_string();
                                cx.write_to_clipboard(ClipboardItem::new_string(code));
                                window.push_notification("Code snippet copied to clipboard.", cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(cx.theme().radius)
                    .overflow_hidden()
                    .child(Editor::new(&self.code_editor).size_full().readonly(true)),
            )
    }

    fn save_current_response_as_example(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let RequestExecutionState::Completed(snapshot) = &self.execution else {
            return;
        };
        let base_name = if snapshot.reason.trim().is_empty() {
            format!("Example {}", snapshot.status_code)
        } else {
            format!("{} {}", snapshot.status_code, snapshot.reason)
        };
        let name = if self.response_examples.iter().any(|e| e.name == base_name) {
            let mut idx = 2usize;
            loop {
                let candidate = format!("{base_name} ({idx})");
                if !self.response_examples.iter().any(|e| e.name == candidate) {
                    break candidate;
                }
                idx += 1;
            }
        } else {
            base_name
        };

        let body = if snapshot.pretty_body.is_empty() {
            snapshot.body_text.clone()
        } else {
            snapshot.pretty_body.clone()
        };
        let ex = ResponseExample::new(
            generate_id("ex"),
            name,
            snapshot.status_code,
            snapshot.reason.clone(),
            snapshot.content_type.clone(),
            snapshot.headers.clone(),
            body.clone(),
        );
        let ex_id = ex.id.clone();
        self.response_examples.push(ex);
        self.selected_example_id = Some(ex_id);

        let extracted = extract_and_merge_response_meta(
            &mut self.response_meta,
            &body,
            snapshot.content_type.as_deref(),
        );
        self.sync_selected_example_ui(window, cx);
        self.sync_meta_desc_inputs(window, cx);
        self.on_edited(window, cx);

        if extracted {
            window.push_notification("Saved response example and updated meta info.", cx);
        } else {
            window.push_notification(
                "Saved response example (skipped meta extraction: not JSON/XML).",
                cx,
            );
        }
    }

    fn extract_meta_from_example(
        &mut self,
        example_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ex) = self.response_examples.iter().find(|e| e.id == example_id) else {
            return;
        };
        let body = ex.body.clone();
        let ct = ex.content_type.clone();
        if extract_and_merge_response_meta(&mut self.response_meta, &body, ct.as_deref()) {
            self.sync_meta_desc_inputs(window, cx);
            self.on_edited(window, cx);
            window.push_notification("Extracted and merged meta info from example.", cx);
        } else {
            window.push_notification(
                "Skipped meta extraction: response is not JSON or XML.",
                cx,
            );
        }
    }

    fn merge_all_examples_into_meta(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut extracted_count = 0usize;
        let examples = self.response_examples.clone();
        for ex in &examples {
            if extract_and_merge_response_meta(
                &mut self.response_meta,
                &ex.body,
                ex.content_type.as_deref(),
            ) {
                extracted_count += 1;
            }
        }
        if extracted_count > 0 {
            self.sync_meta_desc_inputs(window, cx);
            self.on_edited(window, cx);
            window.push_notification(
                format!("Merged meta info from {extracted_count} JSON/XML example(s)."),
                cx,
            );
        } else {
            window.push_notification(
                "No JSON or XML examples found to extract meta info.",
                cx,
            );
        }
    }

    fn delete_example(
        &mut self,
        example_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.response_examples.retain(|e| e.id != example_id);
        if self.selected_example_id.as_deref() == Some(example_id) {
            self.selected_example_id = self.response_examples.first().map(|e| e.id.clone());
        }
        self.sync_selected_example_ui(window, cx);
        self.on_edited(window, cx);
    }

    fn sync_selected_example_ui(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self
            .selected_example_id
            .as_ref()
            .and_then(|id| self.response_examples.iter().find(|e| &e.id == id))
            .or_else(|| self.response_examples.first());

        self.syncing_example_ui = true;
        if let Some(ex) = selected {
            let ex_id = ex.id.clone();
            let ex_name = ex.name.clone();
            let ex_body = ex.body.clone();
            let lang = detect_body_language(&ex_body, ex.content_type.as_deref());
            self.selected_example_id = Some(ex_id);
            self.example_name_input.update(cx, |input, cx| {
                input.set_value(ex_name, window, cx);
            });
            self.example_body_editor.update(cx, |editor, cx| {
                editor.set_highlighter(lang, cx);
                editor.set_value(ex_body, window, cx);
            });
        } else {
            self.selected_example_id = None;
            self.example_name_input.update(cx, |input, cx| {
                input.set_value(String::new(), window, cx);
            });
            self.example_body_editor.update(cx, |editor, cx| {
                editor.set_value(String::new(), window, cx);
            });
        }
        self.syncing_example_ui = false;
    }

    fn on_example_name_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(selected_id) = self.selected_example_id.clone() else {
            return;
        };
        let next_name = self.example_name_input.read(cx).value().to_string();
        if let Some(ex) = self
            .response_examples
            .iter_mut()
            .find(|e| e.id == selected_id)
        {
            ex.name = next_name;
            self.on_edited(window, cx);
        }
    }

    fn sync_meta_desc_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut flat = Vec::new();
        collect_meta_paths(&self.response_meta, "", &mut flat);

        let valid_paths: HashSet<String> = flat.iter().map(|(p, _)| p.clone()).collect();
        self.meta_desc_inputs.retain(|k, _| valid_paths.contains(k));

        for (path, desc) in flat {
            if !self.meta_desc_inputs.contains_key(&path) {
                let input = cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder("Add description…")
                        .default_value(desc)
                });
                let path_clone = path.clone();
                let sub = cx.subscribe_in(
                    &input,
                    window,
                    move |this, _, event: &InputEvent, window, cx| {
                        if matches!(event, InputEvent::Change) {
                            this.on_meta_description_changed(&path_clone, window, cx);
                        }
                    },
                );
                self.meta_desc_subscriptions.push(sub);
                self.meta_desc_inputs.insert(path, input);
            }
        }
    }

    fn on_meta_description_changed(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.meta_desc_inputs.get(path) else {
            return;
        };
        let next_desc = input.read(cx).value().to_string();
        if set_meta_description_by_path(&mut self.response_meta, path, "", &next_desc) {
            self.on_edited(window, cx);
        }
    }

    fn render_response_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let id_base = self.request_id.clone();
        let sections = [
            ResponseSection::Body,
            ResponseSection::Headers,
            ResponseSection::SentRequest,
            ResponseSection::Examples,
            ResponseSection::MetaInfo,
        ];
        let selected_ix = sections
            .iter()
            .position(|s| *s == self.active_response_section)
            .unwrap_or(0);

        let headers_label = match &self.execution {
            RequestExecutionState::Completed(snapshot) => format!(
                "{} ({})",
                crate::i18n::t("headers"),
                snapshot.headers.len()
            ),
            _ => crate::i18n::t("headers").to_string(),
        };
        let examples_label = format!("Examples ({})", self.response_examples.len());
        let meta_count = count_meta_fields(&self.response_meta);
        let meta_label = if meta_count > 0 {
            format!("Meta Info ({meta_count})")
        } else {
            "Meta Info".to_string()
        };

        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .when_some(
                match &self.execution {
                    RequestExecutionState::Completed(snapshot) => Some(snapshot),
                    _ => None,
                },
                |this, snapshot| {
                    this.child(
                        h_flex()
                            .w_full()
                            .px_4()
                            .py_2()
                            .justify_between()
                            .gap_2()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(
                                h_flex()
                                    .gap_3()
                                    .child(status_badge(snapshot.status_code, &snapshot.reason))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("{} ms", snapshot.duration_ms)),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format_bytes(snapshot.size_bytes)),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(snapshot.http_version.clone()),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new(SharedString::from(format!(
                                            "save-ex-{id_base}"
                                        )))
                                        .outline()
                                        .xsmall()
                                        .icon(Icon::new(LucideIcon::Save))
                                        .label("Save as Example")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.save_current_response_as_example(window, cx);
                                        })),
                                    )
                                    .child(
                                        Button::new(SharedString::from(format!(
                                            "copy-resp-{id_base}"
                                        )))
                                        .outline()
                                        .xsmall()
                                        .icon(IconName::Copy)
                                        .label(crate::i18n::t("copy_body"))
                                        .on_click(cx.listener({
                                            let body = snapshot.pretty_body.clone();
                                            move |_, _, window, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    body.clone(),
                                                ));
                                                window.push_notification(
                                                    "Response body copied to clipboard.",
                                                    cx,
                                                );
                                            }
                                        })),
                                    ),
                            ),
                    )
                },
            )
            // Response tabs
            .child(
                TabBar::new(SharedString::from(format!("resp-tabs-{id_base}")))
                    .w_full()
                    .underline()
                    .small()
                    .px_4()
                    .selected_index(selected_ix)
                    .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                        if let Some(sec) = sections.get(*ix) {
                            this.active_response_section = *sec;
                            cx.notify();
                        }
                    }))
                    .child(Tab::new().label(crate::i18n::t("body")))
                    .child(Tab::new().label(headers_label))
                    .child(Tab::new().label("Effective Request"))
                    .child(Tab::new().label(examples_label))
                    .child(Tab::new().label(meta_label)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .when(
                        self.active_response_section == ResponseSection::Examples,
                        |this| this.child(self.render_examples_tab(cx)),
                    )
                    .when(
                        self.active_response_section == ResponseSection::MetaInfo,
                        |this| this.child(self.render_meta_info_tab(cx)),
                    )
                    .when(
                        !matches!(
                            self.active_response_section,
                            ResponseSection::Examples | ResponseSection::MetaInfo
                        ),
                        |this| {
                            this.child(match &self.execution {
                                RequestExecutionState::Idle => v_flex()
                                    .size_full()
                                    .items_center()
                                    .justify_center()
                                    .gap_2()
                                    .p_4()
                                    .child(
                                        Icon::new(LucideIcon::SendHorizontal)
                                            .large()
                                            .text_color(cx.theme().muted_foreground),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_medium()
                                            .child(crate::i18n::t("ready_to_send")),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(crate::i18n::t("ready_to_send_hint")),
                                    )
                                    .into_any_element(),
                                RequestExecutionState::Loading { .. } => v_flex()
                                    .size_full()
                                    .items_center()
                                    .justify_center()
                                    .gap_3()
                                    .p_4()
                                    .child(Spinner::new().large())
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(crate::i18n::t("executing_request")),
                                    )
                                    .child(
                                        Button::new(SharedString::from(format!(
                                            "cancel-req-{id_base}"
                                        )))
                                        .outline()
                                        .small()
                                        .label(crate::i18n::t("cancel_req"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cancel_request(cx);
                                        })),
                                    )
                                    .into_any_element(),
                                RequestExecutionState::Cancelled => div()
                                    .p_4()
                                    .child(
                                        Alert::warning(
                                            SharedString::from(format!(
                                                "resp-cancelled-{id_base}"
                                            )),
                                            "The in-flight request was cancelled.",
                                        )
                                        .title("Request Cancelled"),
                                    )
                                    .into_any_element(),
                                RequestExecutionState::Failed { message } => div()
                                    .p_4()
                                    .child(
                                        Alert::error(
                                            SharedString::from(format!("resp-error-{id_base}")),
                                            message.clone(),
                                        )
                                        .title("Request Failed"),
                                    )
                                    .into_any_element(),
                                RequestExecutionState::Completed(snapshot) => self
                                    .render_completed_response_body(snapshot, cx)
                                    .into_any_element(),
                            })
                        },
                    ),
            )
    }

    fn render_completed_response_body(
        &self,
        snapshot: &HttpResponseSnapshot,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id_base = self.request_id.clone();
        div()
            .size_full()
            .min_h_0()
            .p_3()
                    .when(
                        self.active_response_section == ResponseSection::Body,
                        |this| {
                            let snap_for_fmt = snapshot.clone();
                            this.child(
                                v_flex()
                                    .size_full()
                                    .min_h_0()
                                    .gap_2()
                                    .child(
                                        h_flex()
                                            .w_full()
                                            .justify_between()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(format!(
                                                        "Syntax: {}",
                                                        self.response_body_format
                                                            .language(snapshot)
                                                    )),
                                            )
                                            .child(
                                                h_flex().gap_1().children(
                                                    ResponseBodyFormat::ALL.iter().map(|fmt| {
                                                        let fmt = *fmt;
                                                        let snap_click = snap_for_fmt.clone();
                                                        Button::new(SharedString::from(format!(
                                                            "resp-fmt-{id_base}-{}",
                                                            fmt.label()
                                                        )))
                                                        .xsmall()
                                                        .label(fmt.label())
                                                        .when(
                                                            self.response_body_format == fmt,
                                                            |b| b.primary(),
                                                        )
                                                        .when(
                                                            self.response_body_format != fmt,
                                                            |b| b.outline(),
                                                        )
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                this.response_body_format = fmt;
                                                                let lang =
                                                                    fmt.language(&snap_click);
                                                                this.response_body_editor.update(
                                                                    cx,
                                                                    |ed, cx| {
                                                                        ed.set_highlighter(
                                                                            lang, cx,
                                                                        );
                                                                    },
                                                                );
                                                                cx.notify();
                                                            },
                                                        ))
                                                    }),
                                                ),
                                            ),
                                    )
                                    .child(
                                        div().flex_1().min_h_0().child(
                                            Editor::new(&self.response_body_editor)
                                                .size_full()
                                                .readonly(true),
                                        ),
                                    ),
                            )
                        },
                    )
                    .when(
                        self.active_response_section == ResponseSection::Headers,
                        |this| {
                            this.child(
                                v_flex()
                                    .size_full()
                                    .overflow_y_scrollbar()
                                    .gap_1()
                                    .children(snapshot.headers.iter().enumerate().map(
                                        |(ix, hdr)| {
                                            h_flex()
                                                .w_full()
                                                .px_2()
                                                .py_1p5()
                                                .gap_3()
                                                .rounded(cx.theme().radius)
                                                .when(ix % 2 == 1, |r| r.bg(cx.theme().table_even))
                                                .text_xs()
                                                .font_family(cx.theme().mono_font_family.clone())
                                                .child(
                                                    div()
                                                        .w(px(220.))
                                                        .font_semibold()
                                                        .text_color(cx.theme().foreground)
                                                        .child(hdr.name.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(hdr.value.clone()),
                                                )
                                        },
                                    )),
                            )
                        },
                    )
                    .when(
                        self.active_response_section == ResponseSection::SentRequest,
                        |this| {
                            let sent = &snapshot.sent_request;
                            this.child(
                                v_flex()
                                    .size_full()
                                    .overflow_y_scrollbar()
                                    .gap_3()
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .text_xs()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .child(
                                                Tag::primary().small().child(sent.method.as_str()),
                                            )
                                            .child(
                                                div().flex_1().min_w_0().child(sent.url.clone()),
                                            ),
                                    )
                                    .child(
                                        h_flex()
                                            .justify_between()
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .font_semibold()
                                                    .child("Transmitted Headers"),
                                            )
                                            .child(
                                                Button::new(SharedString::from(format!(
                                                    "toggle-secret-hdr-{id_base}"
                                                )))
                                                .ghost()
                                                .xsmall()
                                                .icon(if self.show_secret_headers {
                                                    IconName::EyeOff
                                                } else {
                                                    IconName::Eye
                                                })
                                                .label(if self.show_secret_headers {
                                                    "Hide Secrets"
                                                } else {
                                                    "Reveal Secrets"
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.show_secret_headers =
                                                        !this.show_secret_headers;
                                                    cx.notify();
                                                })),
                                            ),
                                    )
                                    .child(v_flex().gap_1().children(sent.headers.iter().map(
                                        |h| {
                                            let val = if h.secret && !self.show_secret_headers {
                                                "•••••••• (inherited/secret)".to_string()
                                            } else {
                                                h.value.clone()
                                            };
                                            h_flex()
                                                .px_2()
                                                .py_1()
                                                .gap_3()
                                                .text_xs()
                                                .font_family(cx.theme().mono_font_family.clone())
                                                .child(
                                                    div()
                                                        .w(px(220.))
                                                        .font_semibold()
                                                        .child(h.name.clone()),
                                                )
                                                .child(
                                                    div()
                                                        .flex_1()
                                                        .min_w_0()
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(val),
                                                )
                                        },
                                    )))
                                    .child(
                                        v_flex()
                                            .gap_1()
                                            .child(
                                                div().text_xs().font_semibold().child("Body Sent"),
                                            )
                                            .child(
                                                div()
                                                    .p_2()
                                                    .rounded(cx.theme().radius)
                                                    .border_1()
                                                    .border_color(cx.theme().border)
                                                    .text_xs()
                                                    .font_family(
                                                        cx.theme().mono_font_family.clone(),
                                                    )
                                                    .child(sent.body_preview.clone()),
                                            ),
                                    ),
                            )
                        },
                    )
    }

    fn render_examples_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let id_base = self.request_id.clone();
        if self.response_examples.is_empty() {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .font_medium()
                        .child("No Saved Response Examples"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Send a request and click \"Save as Example\" to store different response examples."),
                )
                .into_any_element();
        }

        let selected = self
            .selected_example_id
            .as_ref()
            .and_then(|id| self.response_examples.iter().find(|e| &e.id == id))
            .or_else(|| self.response_examples.first());

        v_flex()
            .size_full()
            .min_h_0()
            .p_3()
            .gap_2()
            .child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .flex_wrap()
                    .children(self.response_examples.iter().map(|ex| {
                        let is_selected = selected.is_some_and(|s| s.id == ex.id);
                        let ex_id = ex.id.clone();
                        Button::new(SharedString::from(format!("ex-pill-{id_base}-{ex_id}")))
                            .xsmall()
                            .when(is_selected, |b| b.primary())
                            .when(!is_selected, |b| b.outline())
                            .label(format!("{} · {}", ex.status_code, ex.name))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.selected_example_id = Some(ex_id.clone());
                                this.sync_selected_example_ui(window, cx);
                                cx.notify();
                            }))
                    })),
            )
            .when_some(selected, |this, ex| {
                let ex_id_extract = ex.id.clone();
                let ex_id_delete = ex.id.clone();
                let ct_label = ex
                    .content_type
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string());
                this.child(
                    h_flex()
                        .w_full()
                        .gap_2()
                        .items_center()
                        .child(status_badge(ex.status_code, &ex.reason))
                        .child(
                            div()
                                .w(px(220.))
                                .child(Input::new(&self.example_name_input).xsmall()),
                        )
                        .child(Tag::secondary().small().child(ct_label))
                        .child(div().flex_1())
                        .child(
                            Button::new(SharedString::from(format!(
                                "ex-extract-{id_base}-{ex_id_extract}"
                            )))
                            .outline()
                            .xsmall()
                            .icon(Icon::new(LucideIcon::Sparkles))
                            .label("Extract Meta")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.extract_meta_from_example(&ex_id_extract, window, cx);
                            })),
                        )
                        .child(
                            Button::new(SharedString::from(format!(
                                "ex-del-{id_base}-{ex_id_delete}"
                            )))
                            .outline()
                            .xsmall()
                            .icon(IconName::Delete)
                            .label("Delete")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.delete_example(&ex_id_delete, window, cx);
                            })),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded(cx.theme().radius)
                        .overflow_hidden()
                        .child(
                            Editor::new(&self.example_body_editor)
                                .size_full()
                                .readonly(true),
                        ),
                )
            })
            .into_any_element()
    }

    fn render_meta_info_tab(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let id_base = self.request_id.clone();
        let has_completed = matches!(self.execution, RequestExecutionState::Completed(_));
        let has_examples = !self.response_examples.is_empty();
        let has_meta = !self.response_meta.is_empty();

        let mut rows = Vec::new();
        self.collect_meta_tree_rows(&self.response_meta, "", 0, &mut rows, cx);

        v_flex()
            .size_full()
            .min_h_0()
            .p_3()
            .gap_2()
            .child(
                h_flex()
                    .w_full()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Extracted response schema tree (JSON / XML) with editable descriptions"),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .when(has_completed, |this| {
                                this.child(
                                    Button::new(SharedString::from(format!(
                                        "meta-extract-cur-{id_base}"
                                    )))
                                    .outline()
                                    .xsmall()
                                    .icon(Icon::new(LucideIcon::Sparkles))
                                    .label("Extract Current")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if let RequestExecutionState::Completed(snapshot) =
                                            &this.execution
                                        {
                                            let body = if snapshot.pretty_body.is_empty() {
                                                snapshot.body_text.clone()
                                            } else {
                                                snapshot.pretty_body.clone()
                                            };
                                            let ct = snapshot.content_type.clone();
                                            if extract_and_merge_response_meta(
                                                &mut this.response_meta,
                                                &body,
                                                ct.as_deref(),
                                            ) {
                                                this.sync_meta_desc_inputs(window, cx);
                                                this.on_edited(window, cx);
                                                window.push_notification(
                                                    "Extracted and merged response meta info.",
                                                    cx,
                                                );
                                            } else {
                                                window.push_notification(
                                                    "Skipped meta extraction: response is not JSON or XML.",
                                                    cx,
                                                );
                                            }
                                        }
                                    })),
                                )
                            })
                            .when(has_examples, |this| {
                                this.child(
                                    Button::new(SharedString::from(format!(
                                        "meta-merge-ex-{id_base}"
                                    )))
                                    .outline()
                                    .xsmall()
                                    .icon(IconName::RefreshCw)
                                    .label("Merge Examples")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.merge_all_examples_into_meta(window, cx);
                                    })),
                                )
                            })
                            .when(has_meta, |this| {
                                this.child(
                                    Button::new(SharedString::from(format!(
                                        "meta-clear-{id_base}"
                                    )))
                                    .outline()
                                    .xsmall()
                                    .icon(IconName::Delete)
                                    .label("Clear")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.response_meta.clear();
                                        this.collapsed_meta_paths.clear();
                                        this.sync_meta_desc_inputs(window, cx);
                                        this.on_edited(window, cx);
                                    })),
                                )
                            }),
                    ),
            )
            .when(rows.is_empty(), |this| {
                this.child(
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_medium()
                                .child("No Response Meta Info"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Save a JSON or XML response as an example, or click \"Extract Current\" on a JSON/XML response."),
                        ),
                )
            })
            .when(!rows.is_empty(), |this| {
                this.child(
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded(cx.theme().radius)
                        .overflow_hidden()
                        .child(
                            h_flex()
                                .w_full()
                                .px_3()
                                .py_1p5()
                                .gap_3()
                                .bg(cx.theme().table_head)
                                .border_b_1()
                                .border_color(cx.theme().border)
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child(div().w(px(240.)).child("Field"))
                                .child(div().w(px(120.)).child("Type"))
                                .child(div().w(px(180.)).child("Example"))
                                .child(div().flex_1().min_w_0().child("Description")),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scrollbar()
                                .children(rows),
                        ),
                )
            })
    }

    fn collect_meta_tree_rows(
        &self,
        fields: &[ResponseMetaField],
        prefix: &str,
        depth: usize,
        out: &mut Vec<gpui_kit::AnyElement>,
        cx: &mut Context<Self>,
    ) {
        let id_base = self.request_id.clone();
        for field in fields {
            let path = if prefix.is_empty() {
                field.name.clone()
            } else {
                format!("{prefix}.{}", field.name)
            };
            let has_children = !field.children.is_empty();
            let is_collapsed = self.collapsed_meta_paths.contains(&path);
            let row_ix = out.len();
            let toggle_path = path.clone();

            let row = h_flex()
                .w_full()
                .px_3()
                .py_1()
                .gap_3()
                .items_center()
                .border_b_1()
                .border_color(cx.theme().border)
                .when(row_ix % 2 == 1, |r| r.bg(cx.theme().table_even))
                .child(
                    h_flex()
                        .w(px(240.))
                        .items_center()
                        .gap_1()
                        .pl(px((depth as f32) * 16.0))
                        .child(if has_children {
                            Button::new(SharedString::from(format!(
                                "meta-toggle-{id_base}-{row_ix}"
                            )))
                            .ghost()
                            .xsmall()
                            .icon(if is_collapsed {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.collapsed_meta_paths.contains(&toggle_path) {
                                    this.collapsed_meta_paths.remove(&toggle_path);
                                } else {
                                    this.collapsed_meta_paths.insert(toggle_path.clone());
                                }
                                cx.notify();
                            }))
                            .into_any_element()
                        } else {
                            div().w(px(20.)).into_any_element()
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .font_semibold()
                                .font_family(cx.theme().mono_font_family.clone())
                                .child(field.name.clone()),
                        ),
                )
                .child(
                    div()
                        .w(px(120.))
                        .child(Tag::secondary().small().child(field.field_type.clone())),
                )
                .child(
                    div()
                        .w(px(180.))
                        .min_w_0()
                        .text_xs()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_color(cx.theme().muted_foreground)
                        .overflow_hidden()
                        .child(if field.example.is_empty() {
                            "—".to_string()
                        } else {
                            field.example.clone()
                        }),
                )
                .child(
                    div().flex_1().min_w_0().when_some(
                        self.meta_desc_inputs.get(&path),
                        |d, input| d.child(Input::new(input).xsmall()),
                    ),
                )
                .into_any_element();

            out.push(row);

            if has_children && !is_collapsed {
                self.collect_meta_tree_rows(&field.children, &path, depth + 1, out, cx);
            }
        }
    }
}

fn count_meta_fields(fields: &[ResponseMetaField]) -> usize {
    fields
        .iter()
        .map(|f| 1 + count_meta_fields(&f.children))
        .sum()
}

fn collect_meta_paths(
    fields: &[ResponseMetaField],
    prefix: &str,
    out: &mut Vec<(String, String)>,
) {
    for field in fields {
        let path = if prefix.is_empty() {
            field.name.clone()
        } else {
            format!("{prefix}.{}", field.name)
        };
        out.push((path.clone(), field.description.clone()));
        if !field.children.is_empty() {
            collect_meta_paths(&field.children, &path, out);
        }
    }
}

fn set_meta_description_by_path(
    fields: &mut [ResponseMetaField],
    target_path: &str,
    prefix: &str,
    next_desc: &str,
) -> bool {
    for field in fields {
        let path = if prefix.is_empty() {
            field.name.clone()
        } else {
            format!("{prefix}.{}", field.name)
        };
        if path == target_path {
            if field.description != next_desc {
                field.description = next_desc.to_string();
                return true;
            }
            return false;
        }
        if !field.children.is_empty()
            && set_meta_description_by_path(&mut field.children, target_path, &path, next_desc)
        {
            return true;
        }
    }
    false
}

impl Render for RequestTabView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let id_base = self.request_id.clone();
        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .child(self.render_header_and_url_bar(cx))
            .child(
                div().flex_1().min_h_0().child(
                    v_resizable(SharedString::from(format!("req-resp-split-{id_base}")))
                        .child(
                            resizable_panel()
                                .size(px(300.))
                                .size_range(px(180.)..px(700.))
                                .child(self.render_request_pane(cx)),
                        )
                        .child(self.render_response_pane(cx).into_any_element()),
                ),
            )
    }
}

fn extract_body_fields(
    body: &RequestBody,
) -> (
    String,
    RawBodyFormat,
    String,
    Vec<testrix_core::KeyValueRow>,
    Vec<testrix_core::MultipartField>,
    String,
    String,
) {
    match body {
        RequestBody::None => (
            "{\n  \n}".to_string(),
            RawBodyFormat::Text,
            String::new(),
            vec![],
            vec![],
            String::new(),
            String::new(),
        ),
        RequestBody::Json { content } => (
            content.clone(),
            RawBodyFormat::Text,
            String::new(),
            vec![],
            vec![],
            String::new(),
            String::new(),
        ),
        RequestBody::Raw {
            raw_format,
            content,
        } => (
            "{\n  \n}".to_string(),
            *raw_format,
            content.clone(),
            vec![],
            vec![],
            String::new(),
            String::new(),
        ),
        RequestBody::FormUrlEncoded { fields } => (
            "{\n  \n}".to_string(),
            RawBodyFormat::Text,
            String::new(),
            fields.clone(),
            vec![],
            String::new(),
            String::new(),
        ),
        RequestBody::Multipart { fields } => (
            "{\n  \n}".to_string(),
            RawBodyFormat::Text,
            String::new(),
            vec![],
            fields.clone(),
            String::new(),
            String::new(),
        ),
        RequestBody::Binary {
            file_path,
            content_type,
        } => (
            "{\n  \n}".to_string(),
            RawBodyFormat::Text,
            String::new(),
            vec![],
            vec![],
            file_path.clone(),
            content_type.clone().unwrap_or_default(),
        ),
    }
}

fn build_breadcrumb(
    project: &ProjectNode,
    folders: &[&FolderNode],
    request_name: &str,
) -> SharedString {
    let mut parts = Vec::with_capacity(folders.len() + 2);
    parts.push(project.doc.name.clone());
    for f in folders {
        parts.push(f.doc.name.clone());
    }
    parts.push(request_name.to_string());
    parts.join(" › ").into()
}

fn split_url_prefix(raw: &str) -> (String, String) {
    let trimmed = raw.trim();
    if trimmed.starts_with("{{")
        && let Some(end) = trimmed.find("}}")
    {
        let prefix = &trimmed[..end + 2];
        let rest = &trimmed[end + 2..];
        return (prefix.to_string(), rest.to_string());
    }

    for scheme in ["https://", "http://"] {
        if let Some(after_scheme) = trimmed.strip_prefix(scheme) {
            if let Some(slash_ix) = after_scheme.find('/') {
                let split_at = scheme.len() + slash_ix;
                return (
                    trimmed[..split_at].to_string(),
                    trimmed[split_at..].to_string(),
                );
            }
            return (trimmed.to_string(), String::new());
        }
    }

    (String::new(), trimmed.to_string())
}

fn combine_url_prefix(prefix: &str, path: &str) -> String {
    let p = prefix.trim();
    let rest = path.trim();
    if p.is_empty() {
        return rest.to_string();
    }
    if rest.is_empty() {
        return p.to_string();
    }
    if p.ends_with('/') && rest.starts_with('/') {
        return format!("{}{}", p.trim_end_matches('/'), rest);
    }
    if !p.ends_with('/')
        && !rest.starts_with('/')
        && !rest.starts_with('?')
        && !rest.starts_with('#')
    {
        return format!("{p}/{rest}");
    }
    format!("{p}{rest}")
}

fn build_url_prefix_options(
    project: &ProjectNode,
    active_env: Option<&EnvironmentDoc>,
) -> Vec<ComboboxOption> {
    let mut options = Vec::new();
    let base = project.doc.base_url.trim();
    if !base.is_empty() {
        options.push(ComboboxOption::new(base).with_detail("Project baseUrl"));
    }

    if let Some(env) = active_env {
        for var in &env.variables {
            if !var.enabled || var.key.trim().is_empty() {
                continue;
            }
            let token = format!("{{{{{}}}}}", var.key.trim());
            let detail = if var.secret {
                "••••••".to_string()
            } else {
                var.value.clone()
            };
            if let Some(existing) = options.iter_mut().find(|opt| opt.value == token) {
                if !detail.is_empty() {
                    existing.detail = Some(detail);
                }
            } else {
                options.push(ComboboxOption::new(token).with_detail(detail));
            }
        }
    }

    options
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_combines_url_prefix_and_path() {
        assert_eq!(
            split_url_prefix("{{base_url}}/v1/users"),
            ("{{base_url}}".to_string(), "/v1/users".to_string())
        );
        assert_eq!(
            split_url_prefix("https://api.example.com/v1/users"),
            (
                "https://api.example.com".to_string(),
                "/v1/users".to_string()
            )
        );
        assert_eq!(
            split_url_prefix("/delete"),
            (String::new(), "/delete".to_string())
        );

        assert_eq!(combine_url_prefix("", "/delete"), "/delete");
        assert_eq!(
            combine_url_prefix("{{base_url}}/", "/delete"),
            "{{base_url}}/delete"
        );
        assert_eq!(
            combine_url_prefix("{{base_url}}", "delete"),
            "{{base_url}}/delete"
        );
    }
}
