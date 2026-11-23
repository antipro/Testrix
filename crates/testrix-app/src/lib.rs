//! GPUI Kit desktop application shell and workbench UI for Testrix.

mod actions;
mod app_view;
mod auth_section;
pub mod cli_api;
mod container_editor;
mod editable_combobox;
mod env_editor;
mod explorer;
mod history_panel;
mod i18n;
mod kv_editor;
mod post_script;
mod request_editor;
mod sets_panel;
mod sidebar_pane;
mod theme_helpers;
mod tray;

pub use app_view::{TestrixApp, init_app, open_main_window};
pub use tray::{SingleInstanceGuard, acquire_single_instance_or_activate_existing, init_system_tray};
