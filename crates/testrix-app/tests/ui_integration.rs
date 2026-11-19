use std::{
    io::{Read as _, Write as _},
    net::TcpListener,
    sync::mpsc,
    time::{Duration, Instant},
};

use gpui_kit::{
    AppContext as _, Bounds, Point, TestAppContext, WindowBounds, WindowOptions, px, size,
    test::{TestAppContextExt as _, TestWindowExt as _},
};
use tempfile::TempDir;
use testrix_app::{TestrixApp, init_app};
use testrix_core::{AuthConfig, EffectiveAuth, ExplorerNode, HttpMethod, resolve_request_auth};
use testrix_storage::{ensure_seeded_workspace, load_workspace, save_environment};

fn open_testrix_test_window(
    cx: &mut TestAppContext,
    workspace_dir: std::path::PathBuf,
) -> (
    gpui_kit::WindowHandle<gpui_kit::base::Root>,
    gpui_kit::Entity<TestrixApp>,
) {
    cx.update(|cx| {
        let bounds = Bounds {
            origin: Point::default(),
            size: size(px(1280.), px(820.)),
        };
        let (window, content) = gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            cx,
            |window, cx| cx.new(|cx| TestrixApp::new(workspace_dir, window, cx)),
        )
        .expect("open test window");
        (
            window
                .downcast::<gpui_kit::base::Root>()
                .expect("Base Root"),
            content,
        )
    })
}

#[gpui_kit::test]
fn layout_places_profile_top_right_and_explorer_on_right(cx: &mut TestAppContext) {
    let dir = TempDir::new().unwrap();
    cx.update(init_app);

    let (handle, app) = open_testrix_test_window(cx, dir.path().to_path_buf());

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        let profile_bar = window.find("top-right-profile-bar");
        let workbench = window.find("center-workbench-pane");
        let explorer = window.find("right-explorer-panel");
        let search = window.find("explorer-quick-search");

        assert!(profile_bar.visible());
        assert!(workbench.visible());
        assert!(explorer.visible());
        assert!(search.visible());

        // Profile + env controls must be in the right-top corner above workbench/explorer.
        assert!(profile_bar.bounds().top() < workbench.bounds().top());
        assert!(profile_bar.bounds().left() > px(640.));

        // Explorer must sit to the RIGHT of the center workbench.
        assert!(explorer.bounds().left() >= workbench.bounds().right() - px(4.));
        // Quick search must sit inside the right explorer.
        assert!(search.bounds().left() >= explorer.bounds().left());

        // Method select stays compact while the URL input fills the bar.
        let method_bounds = window.find("req-method-req-inspect-get").bounds();
        let url_bounds = window.find("req-url-req-inspect-get").bounds();
        assert!(method_bounds.size.width <= px(130.), "{method_bounds:?}");
        assert!(url_bounds.size.width > px(200.), "{url_bounds:?}");
    })
    .unwrap();

    cx.update(|cx| {
        let app_ref = app.read(cx);
        assert_eq!(app_ref.open_tab_count(), 1);
        assert!(app_ref.workspace().active_environment().is_some());
    });
}

#[gpui_kit::test]
fn quick_search_filters_explorer_and_opens_tabs_with_inherited_auth(cx: &mut TestAppContext) {
    let dir = TempDir::new().unwrap();
    cx.update(init_app);

    let (handle, app) = open_testrix_test_window(cx, dir.path().to_path_buf());

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);

        assert_eq!(
            window
                .try_find("tree-row-proj:proj-starter")
                .map(|s| s.visible()),
            Some(true),
            "starter project row should be visible before search; explorer bounds={:?}",
            window.find("right-explorer-panel").bounds()
        );

        // Quick search for "Rotate" should filter down to the nested Security request.
        window.click("explorer-quick-search", cx);
        window.input("Rotate", cx);
        window.render_frame(cx);
        assert_eq!(window.find("explorer-quick-search").value(), Some("Rotate"));
    })
    .unwrap();

    // Allow GPUI entity subscriptions (`InputEvent::Change` -> `ExplorerPanel::rebuild_tree`)
    // to flush before inspecting the re-rendered tree rows.
    cx.run_until_parked();

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(
            window
                .try_find("tree-row-req:req-security-rotate")
                .is_some(),
            "filtered rotate row missing after run_until_parked"
        );
        assert!(window.try_find("tree-row-req:req-users-list").is_none());

        // Clicking the matching request opens a new workbench tab.
        window.click("tree-row-req:req-security-rotate", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update(|cx| {
        let app_ref = app.read(cx);
        assert_eq!(app_ref.open_tab_count(), 2);
        assert_eq!(
            app_ref.active_tab_key(cx).as_deref(),
            Some("req:req-security-rotate")
        );

        let proj = app_ref.workspace().active_project().unwrap();
        let resolved = resolve_request_auth(proj, "req-security-rotate").unwrap();
        assert!(matches!(resolved.effective, EffectiveAuth::ApiKey { .. }));
    });
}

#[gpui_kit::test]
fn saves_folder_auth_inheritance_and_yaml_documents_to_disk(cx: &mut TestAppContext) {
    let dir = TempDir::new().unwrap();
    cx.update(init_app);

    let (handle, app) = open_testrix_test_window(cx, dir.path().to_path_buf());

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        // Open the Users folder tab (which initially inherits Bearer from Project)
        app.update(cx, |app, cx| {
            app.open_node(
                ExplorerNode::Folder {
                    project_id: "proj-starter".to_string(),
                    folder_id: "fld-users".to_string(),
                },
                window,
                cx,
            );
        });
        window.render_frame(cx);
        assert!(window.try_find("save-container-fld:fld-users").is_some());
    })
    .unwrap();

    // Verify seeded YAML files on disk can be reloaded cleanly via testrix-storage.
    let reloaded = load_workspace(dir.path()).unwrap();
    assert_eq!(reloaded.projects.len(), 1);
    let proj = &reloaded.projects[0];
    assert!(matches!(proj.doc.auth, AuthConfig::Bearer { .. }));
    let (_, create_user) = proj.find_request("req-users-create").unwrap();
    assert_eq!(create_user.doc.request.method, HttpMethod::Post);
    assert!(
        create_user
            .file_path
            .to_string_lossy()
            .ends_with(".request.yaml")
    );
}

#[gpui_kit::test]
async fn env_sheet_creates_profile_and_persists_variables(cx: &mut TestAppContext) {
    let dir = TempDir::new().unwrap();
    cx.update(init_app);

    let (handle, app) = open_testrix_test_window(cx, dir.path().to_path_buf());

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("env-vars-button", cx);
    })
    .unwrap();

    cx.wait_for(handle.into(), Duration::from_secs(1), |window, _| {
        window.try_find("env-save-profile").is_some()
    })
    .await;
    // Allow the Sheet 150ms slide-in animation to complete before clicking inside it.
    cx.executor().advance_clock(Duration::from_millis(160));
    cx.run_until_parked();

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("env-new-profile-input", cx);
        window.input("Local QA", cx);
        assert_eq!(
            window.find("env-new-profile-input").value(),
            Some("Local QA")
        );
        window.click("env-create-profile", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(handle.into(), |_, window, cx| {
        window.click("env-add-variable", cx);
        window.input("custom_flag", cx);
        window.click("env-save-profile", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update(|cx| {
        let app_ref = app.read(cx);
        let active_env = app_ref
            .workspace()
            .active_environment()
            .expect("active env");
        assert_eq!(active_env.doc.name, "Local QA");
        assert!(
            active_env
                .doc
                .variables
                .iter()
                .any(|v| v.key == "custom_flag"),
            "saved variables: {:?}",
            active_env.doc.variables
        );
    });

    let reloaded = load_workspace(dir.path()).unwrap();
    let qa_env = reloaded
        .environments
        .iter()
        .find(|e| e.doc.name == "Local QA")
        .expect("Local QA profile persisted as YAML");
    assert!(qa_env.doc.variables.iter().any(|v| v.key == "custom_flag"));
}

#[gpui_kit::test]
async fn executes_http_request_with_inherited_folder_auth_and_env_interpolation(
    cx: &mut TestAppContext,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel::<String>();

    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept http connection");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        let mut buf = Vec::new();
        let mut chunk = [0_u8; 1024];
        let header_end = loop {
            let n = stream.read(&mut chunk).expect("read request headers");
            assert!(n > 0, "unexpected EOF waiting for headers");
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
        };

        let header_str = String::from_utf8_lossy(&buf[..header_end]).to_string();
        let content_length: usize = header_str
            .lines()
            .find_map(|line| {
                let (k, v) = line.split_once(':')?;
                if k.eq_ignore_ascii_case("content-length") {
                    v.trim().parse().ok()
                } else {
                    None
                }
            })
            .unwrap_or(0);

        while buf.len() < header_end + content_length {
            let n = stream.read(&mut chunk).expect("read request body");
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }

        let captured = String::from_utf8_lossy(&buf).to_string();
        tx.send(captured).unwrap();

        let body = r#"{"rotated":true,"inherited":"admin-key"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
    });

    let dir = TempDir::new().unwrap();
    let mut tree = ensure_seeded_workspace(dir.path()).unwrap();
    {
        let dev_env = &mut tree.environments[0];
        let base_var = dev_env
            .doc
            .variables
            .iter_mut()
            .find(|v| v.key == "base_url")
            .expect("base_url variable");
        base_var.value = format!("http://127.0.0.1:{port}");
        save_environment(dev_env).unwrap();
    }

    cx.update(init_app);
    let (handle, _) = open_testrix_test_window(cx, dir.path().to_path_buf());

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("explorer-quick-search", cx);
        window.input("Rotate", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("tree-row-req:req-security-rotate", cx);
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("req-send-req-security-rotate", cx);
        window.render_frame(cx);
    })
    .unwrap();

    // Poll until the multi-thread Tokio HTTP runner delivers its response into GPUI.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut completed = false;
    while Instant::now() < deadline {
        cx.run_until_parked();
        let ready = cx
            .update_window(handle.into(), |_, window, cx| {
                window.render_frame(cx);
                window.try_find("copy-resp-req-security-rotate").is_some()
            })
            .unwrap();
        if ready {
            completed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        completed,
        "HTTP response view did not render before timeout"
    );

    let raw_request = rx
        .recv_timeout(Duration::from_secs(2))
        .expect("captured HTTP request");
    let lower = raw_request.to_ascii_lowercase();
    assert!(
        raw_request.starts_with("DELETE /delete HTTP/1.1"),
        "unexpected request line: {raw_request}"
    );
    assert!(
        lower.contains("x-admin-key: dev-admin-key-001"),
        "expected inherited Admin folder X-Admin-Key header in:\n{raw_request}"
    );
    assert!(
        lower.contains("x-client-app: testrix/0.1"),
        "expected inherited Project default header in:\n{raw_request}"
    );
    assert!(
        raw_request.contains("reason: scheduled-rotation"),
        "expected YAML body payload in:\n{raw_request}"
    );

    server.join().unwrap();
}

#[gpui_kit::test]
fn url_prefix_and_header_editable_comboboxes_select_and_edit(cx: &mut TestAppContext) {
    let dir = TempDir::new().unwrap();
    cx.update(init_app);

    let (handle, app) = open_testrix_test_window(cx, dir.path().to_path_buf());

    // 1. Test URL prefix editable combobox: open dropdown, pick {{base_url}}, or type custom var.
    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find("req-url-prefix-req-inspect-get").is_some());
        window.click("req-url-prefix-req-inspect-get-toggle", cx);
        window.render_frame(cx);
        assert!(
            window
                .try_find("req-url-prefix-req-inspect-get-menu")
                .is_some(),
            "URL prefix dropdown menu should open on toggle click"
        );
        window.click("req-url-prefix-req-inspect-get", cx);
        window.input("{{base_url}}", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("req-url-prefix-req-inspect-get").value(),
            Some("{{base_url}}")
        );
    })
    .unwrap();
    cx.run_until_parked();

    // 2. Test Header editable combobox: open Create User (has X-Idempotency-Key), switch to Headers tab,
    //    open common header dropdown, and edit/select a common header.
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |app, cx| {
            app.open_node(
                ExplorerNode::Request {
                    project_id: "proj-starter".to_string(),
                    request_id: "req-users-create".to_string(),
                },
                window,
                cx,
            );
        });
        window.render_frame(cx);
        window
            .within("req-sections-req-users-create")
            .click(1usize, cx);
        window.render_frame(cx);

        assert!(
            window
                .try_find("req-headers-req-users-create-name-1")
                .is_some(),
            "header editable combobox should be rendered on Headers tab"
        );
        window.click("req-headers-req-users-create-name-1-toggle", cx);
        window.render_frame(cx);
        assert!(
            window
                .try_find("req-headers-req-users-create-name-1-menu")
                .is_some(),
            "common headers dropdown should open on toggle click"
        );

        window.click("req-headers-req-users-create-add", cx);
        window.render_frame(cx);
        window.click("req-headers-req-users-create-name-2", cx);
        window.input("Content-Type", cx);
        window.render_frame(cx);
        assert_eq!(
            window.find("req-headers-req-users-create-name-2").value(),
            Some("Content-Type")
        );
    })
    .unwrap();
}

#[gpui_kit::test]
async fn workspace_button_in_titlebar_opens_workspace_dialog(cx: &mut TestAppContext) {
    let dir = TempDir::new().unwrap();
    cx.update(init_app);

    let (handle, _) = open_testrix_test_window(cx, dir.path().to_path_buf());

    cx.update_window(handle.into(), |_, window, cx| {
        window.render_frame(cx);
        window.click("open-workspace-btn", cx);
    })
    .unwrap();

    cx.wait_for(handle.into(), Duration::from_secs(1), |window, _| {
        window.try_find("workspace-dir-input").is_some()
            && window.try_find("workspace-browse-btn").is_some()
            && window.try_find("workspace-apply-btn").is_some()
    })
    .await;
}

#[gpui_kit::test]
async fn auto_sync_triggers_on_ctrl_s_after_editing_meta_info(cx: &mut TestAppContext) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel::<String>();

    std::thread::spawn(move || {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            loop {
                let n = socket.read(&mut tmp).unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                let s = String::from_utf8_lossy(&buf);
                if let Some(hdr_end) = s.find("\r\n\r\n") {
                    let headers = &s[..hdr_end];
                    let content_len = headers
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            if k.eq_ignore_ascii_case("content-length") {
                                v.trim().parse::<usize>().ok()
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0);
                    if buf.len() >= hdr_end + 4 + content_len {
                        let body = String::from_utf8_lossy(&buf[hdr_end + 4..]).to_string();
                        let _ = tx.send(body);
                        let resp_body = format!(
                            r#"{{"ok":true,"filesWritten":1,"shareUrl":"http://{addr}/s/starter"}}"#
                        );
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            resp_body.len(),
                            resp_body
                        );
                        socket.write_all(resp.as_bytes()).unwrap();
                        break;
                    }
                }
            }
        }
    });

    let dir = TempDir::new().unwrap();
    ensure_seeded_workspace(dir.path()).unwrap();
    cx.update(init_app);

    let (handle, app) = open_testrix_test_window(cx, dir.path().to_path_buf());

    // Configure mock share server and click Upload YAML on the project
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |this, cx| {
            this.set_share_settings_for_test(
                testrix_storage::ShareSettings {
                    share_server_url: format!("http://{addr}"),
                    share_email: "test@example.com".to_string(),
                    share_token: "tok-123".to_string(),
                    cloudflare_account_id: String::new(),
                    cloudflare_r2_bucket: String::new(),
                    cloudflare_api_token: String::new(),
                    cloudflare_pages_url: String::new(),
                    share_access_token: String::new(),
                },
                window,
                cx,
            );
        });
        app.update(cx, |this, cx| {
            this.open_node(
                ExplorerNode::Project {
                    project_id: "proj-starter".to_string(),
                },
                window,
                cx,
            );
        });
        window.render_frame(cx);
        window
            .within("container-sections-proj:proj-starter")
            .click(4usize, cx);
        window.render_frame(cx);
        window.click("share-upload-btn", cx);
    })
    .unwrap();

    // Wait for initial share upload
    let first_upload = rx.recv_timeout(Duration::from_secs(5)).expect("initial share upload");
    assert!(first_upload.contains("Starter Platform API"));

    cx.wait_for(handle.into(), Duration::from_secs(2), |_, cx| {
        app.read(cx).active_share_count() >= 1
    })
    .await;

    // Now open a request, edit URL, and press ctrl-s
    cx.update_window(handle.into(), |_, window, cx| {
        app.update(cx, |this, cx| {
            this.open_node(
                ExplorerNode::Request {
                    project_id: "proj-starter".to_string(),
                    request_id: "req-inspect-get".to_string(),
                },
                window,
                cx,
            );
        });
        window.render_frame(cx);
        window.click("req-url-req-inspect-get", cx);
        window.input("/v2", cx);
        window.render_frame(cx);
        window.press("ctrl-s", cx);
    })
    .unwrap();

    let second_upload = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("auto-sync upload after ctrl-s");
    assert!(
        second_upload.contains("/v2"),
        "expected updated request YAML in auto-sync upload"
    );
}
