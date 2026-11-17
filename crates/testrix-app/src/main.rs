use std::path::PathBuf;

use gpui_kit::assets::AllAssets;
use testrix_app::{init_app, open_main_window};
use testrix_storage::default_workspace_dir;
use tracing_subscriber::EnvFilter;

fn parse_workspace_arg() -> PathBuf {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--workspace"
            && let Some(path) = args.next()
        {
            return PathBuf::from(path);
        } else if let Some(path) = arg.strip_prefix("--workspace=") {
            return PathBuf::from(path);
        }
    }
    default_workspace_dir()
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let workspace_dir = parse_workspace_arg();

    gpui_kit::application()
        .with_assets(AllAssets)
        .run(move |cx| {
            init_app(cx);
            open_main_window(workspace_dir, cx).expect("failed to open Testrix window");
            cx.activate(true);
        });
}
