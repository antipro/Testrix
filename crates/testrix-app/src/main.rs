#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use std::path::PathBuf;

use gpui_kit::assets::AllAssets;
use testrix_app::{init_app, open_main_window};
use testrix_storage::{default_workspace_dir, init_or_load_config};
use tracing_subscriber::EnvFilter;

fn parse_workspace_arg() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--workspace"
            && let Some(path) = args.next()
        {
            return Some(PathBuf::from(path));
        } else if let Some(path) = arg.strip_prefix("--workspace=") {
            return Some(PathBuf::from(path));
        }
    }
    None
}

fn resolve_startup_workspace_dir() -> PathBuf {
    let cfg = init_or_load_config();
    if let Some(cli_dir) = parse_workspace_arg() {
        return cli_dir;
    }
    match cfg {
        Ok(cfg) => cfg.workspace_dir,
        Err(err) => {
            tracing::warn!(error = %err, "failed loading ~/.testrix/config.yaml; falling back to default workspace");
            default_workspace_dir()
        }
    }
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let workspace_dir = resolve_startup_workspace_dir();

    gpui_kit::application()
        .with_assets(AllAssets)
        .run(move |cx| {
            init_app(cx);
            open_main_window(workspace_dir, cx).expect("failed to open Testrix window");
            cx.activate(true);
        });
}
