use std::{
    io::{Read as _, Write as _},
    net::TcpListener,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui_kit::{
    AppContext as _, Entity, VisualTestAppContext, assets::AllAssets, base::Root, px, size,
    test::TestWindowExt as _,
};
use tempfile::TempDir;
use testrix_app::{TestrixApp, init_app};
use testrix_core::ExplorerNode;
use testrix_storage::{ensure_seeded_workspace, save_environment};

fn spawn_demo_http_server() -> (u16, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind demo server");
    let port = listener.local_addr().expect("local addr").port();

    let handle = std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut buf = [0_u8; 4096];
        let _ = stream.read(&mut buf);

        let body = r#"{
  "status": "rotated",
  "service": "starter-platform-api",
  "environment": "development",
  "inheritedAuth": {
    "source": "Folder: Admin -> Security -> Request",
    "header": "X-Admin-Key"
  },
  "keyVersion": 42
}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Testrix-Demo: true\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.flush();
    });

    (port, handle)
}

fn main() {
    let out_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("testrix-screenshot.png"));

    let (port, server) = spawn_demo_http_server();
    let workspace_dir = TempDir::new().expect("create temp workspace");
    let mut tree = ensure_seeded_workspace(workspace_dir.path()).expect("seed workspace");
    if let Some(dev_env) = tree.projects[0]
        .environments
        .iter_mut()
        .find(|e| e.doc.id == "env-development")
    {
        if let Some(var) = dev_env
            .doc
            .variables
            .iter_mut()
            .find(|v| v.key == "base_url")
        {
            var.value = format!("http://127.0.0.1:{port}");
        }
        save_environment(dev_env).expect("save development env");
    }

    let mut cx = VisualTestAppContext::with_asset_source(
        gpui_kit::platform::current_platform(false),
        Arc::new(AllAssets),
    );
    cx.update(init_app);

    let ws_path = workspace_dir.path().to_path_buf();
    let mut app_entity: Option<Entity<TestrixApp>> = None;
    let window = cx
        .open_offscreen_window(size(px(1360.), px(860.)), |window, cx| {
            let view = cx.new(|cx| TestrixApp::new(ws_path, window, cx));
            app_entity = Some(view.clone());
            cx.new(|cx| Root::new(view, window, cx))
        })
        .expect("open offscreen Metal window");
    let app = app_entity.expect("app entity built");

    // Open additional tabs and execute Rotate Signing Keys against our local server.
    cx.update_window(window.into(), |_, window, cx| {
        window.render_frame(cx);
        app.update(cx, |app, cx| {
            app.open_node(
                ExplorerNode::Request {
                    project_id: "proj-starter".to_string(),
                    request_id: "req-users-create".to_string(),
                },
                window,
                cx,
            );
            app.open_node(
                ExplorerNode::Request {
                    project_id: "proj-starter".to_string(),
                    request_id: "req-security-rotate".to_string(),
                },
                window,
                cx,
            );
        });
        window.render_frame(cx);
        window.click("req-send-req-security-rotate", cx);
        window.render_frame(cx);
    })
    .expect("update window");

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        cx.run_until_parked();
        let ready = cx
            .update_window(window.into(), |_, window, cx| {
                window.render_frame(cx);
                window.try_find("copy-resp-req-security-rotate").is_some()
            })
            .expect("poll response");
        if ready {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    let image = cx
        .update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.render_to_image()
        })
        .expect("render window")
        .expect("render_to_image");

    image.save(&out_path).expect("save screenshot PNG");
    println!(
        "Saved screenshot {} ({}x{})",
        out_path.display(),
        image.width(),
        image.height()
    );

    let _ = server.join();
}
