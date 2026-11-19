use gpui_kit::{App, KeyBinding, actions};

actions!(
    testrix,
    [
        Quit,
        SaveActive,
        SendRequest,
        CancelRequest,
        NewRequest,
        NewFolder,
        NewProject,
        FocusQuickSearch,
        CloseActiveTab,
        ToggleTheme,
        ToggleEnvSheet,
        ReloadWorkspace,
    ]
);

pub(crate) const APP_KEY_CONTEXT: &str = "TestrixApp";

/// Registers keyboard shortcuts before menu construction.
pub(crate) fn init_keybindings(cx: &mut App) {
    cx.bind_keys([
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-q", Quit, None),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-q", Quit, None),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-s", SaveActive, Some(APP_KEY_CONTEXT)),
        KeyBinding::new("ctrl-s", SaveActive, Some(APP_KEY_CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-enter", SendRequest, Some(APP_KEY_CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-enter", SendRequest, Some(APP_KEY_CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-k", FocusQuickSearch, Some(APP_KEY_CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-k", FocusQuickSearch, Some(APP_KEY_CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-n", NewRequest, Some(APP_KEY_CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-n", NewRequest, Some(APP_KEY_CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-w", CloseActiveTab, Some(APP_KEY_CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-w", CloseActiveTab, Some(APP_KEY_CONTEXT)),
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-e", ToggleEnvSheet, Some(APP_KEY_CONTEXT)),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-e", ToggleEnvSheet, Some(APP_KEY_CONTEXT)),
    ]);
}
