//! GPUI Kit desktop application shell and workbench UI for Testrix.

mod actions;
mod app_view;
mod auth_section;
mod container_editor;
mod editable_combobox;
mod env_editor;
mod explorer;
mod kv_editor;
mod request_editor;
mod theme_helpers;

pub use app_view::{TestrixApp, init_app, open_main_window};
