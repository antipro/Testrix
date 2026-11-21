use std::{
    collections::{BTreeMap, HashSet},
    sync::{Mutex, OnceLock},
};

use gpui_kit::{
    App, Global, KeyBinding, Keystroke, actions,
    component::input::{Replace, Search},
};

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

#[derive(Clone, Copy, Debug)]
pub(crate) struct ShortcutActionSpec {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) description: &'static str,
    pub(crate) default_shortcut: &'static str,
}

#[cfg(target_os = "macos")]
pub(crate) const CUSTOMIZABLE_SHORTCUTS: &[ShortcutActionSpec] = &[
    ShortcutActionSpec {
        id: "save_active",
        label: "Save",
        description: "Save the active request or document",
        default_shortcut: "cmd-s",
    },
    ShortcutActionSpec {
        id: "send_request",
        label: "Send Request",
        description: "Execute the active HTTP request",
        default_shortcut: "cmd-enter",
    },
    ShortcutActionSpec {
        id: "focus_quick_search",
        label: "Quick Search",
        description: "Focus explorer quick search",
        default_shortcut: "cmd-k",
    },
    ShortcutActionSpec {
        id: "new_request",
        label: "New Request",
        description: "Create a new HTTP request",
        default_shortcut: "cmd-n",
    },
    ShortcutActionSpec {
        id: "close_active_tab",
        label: "Close Tab",
        description: "Close the active workbench tab",
        default_shortcut: "cmd-w",
    },
    ShortcutActionSpec {
        id: "toggle_env_sheet",
        label: "Environment Variables",
        description: "Open the environment variables dialog",
        default_shortcut: "cmd-e",
    },
    ShortcutActionSpec {
        id: "find",
        label: "Find",
        description: "Find in Request Body, Response Body, or Post-Script",
        default_shortcut: "cmd-f",
    },
    ShortcutActionSpec {
        id: "replace",
        label: "Find & Replace",
        description: "Find & Replace in Request Body or Post-Script",
        default_shortcut: "cmd-h",
    },
];

#[cfg(not(target_os = "macos"))]
pub(crate) const CUSTOMIZABLE_SHORTCUTS: &[ShortcutActionSpec] = &[
    ShortcutActionSpec {
        id: "save_active",
        label: "Save",
        description: "Save the active request or document",
        default_shortcut: "ctrl-s",
    },
    ShortcutActionSpec {
        id: "send_request",
        label: "Send Request",
        description: "Execute the active HTTP request",
        default_shortcut: "ctrl-enter",
    },
    ShortcutActionSpec {
        id: "focus_quick_search",
        label: "Quick Search",
        description: "Focus explorer quick search",
        default_shortcut: "ctrl-k",
    },
    ShortcutActionSpec {
        id: "new_request",
        label: "New Request",
        description: "Create a new HTTP request",
        default_shortcut: "ctrl-n",
    },
    ShortcutActionSpec {
        id: "close_active_tab",
        label: "Close Tab",
        description: "Close the active workbench tab",
        default_shortcut: "ctrl-w",
    },
    ShortcutActionSpec {
        id: "toggle_env_sheet",
        label: "Environment Variables",
        description: "Open the environment variables dialog",
        default_shortcut: "ctrl-e",
    },
    ShortcutActionSpec {
        id: "find",
        label: "Find",
        description: "Find in Request Body, Response Body, or Post-Script",
        default_shortcut: "ctrl-f",
    },
    ShortcutActionSpec {
        id: "replace",
        label: "Find & Replace",
        description: "Find & Replace in Request Body or Post-Script",
        default_shortcut: "ctrl-h",
    },
];

struct BaseKeyBindings(Vec<KeyBinding>);
impl Global for BaseKeyBindings {}

fn intern_shortcut(s: &str) -> &'static str {
    static INTERNED: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let set = INTERNED.get_or_init(|| Mutex::new(HashSet::new()));
    let mut guard = set.lock().expect("shortcut interner lock poisoned");
    if let Some(&existing) = guard.get(s) {
        return existing;
    }
    let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
    guard.insert(leaked);
    leaked
}

pub(crate) fn validate_shortcut(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().to_ascii_lowercase();
    if trimmed.is_empty() {
        return Err("Shortcut cannot be empty".to_string());
    }
    let static_str = intern_shortcut(&trimmed);
    Keystroke::parse(static_str)
        .map(|_| trimmed)
        .map_err(|err| format!("Invalid shortcut '{raw}': {err}"))
}

pub(crate) fn effective_shortcut(
    custom: &BTreeMap<String, String>,
    spec: &ShortcutActionSpec,
) -> String {
    custom
        .get(spec.id)
        .and_then(|s| validate_shortcut(s).ok())
        .unwrap_or_else(|| spec.default_shortcut.to_string())
}

/// Registers keyboard shortcuts before menu construction.
pub(crate) fn init_keybindings(cx: &mut App) {
    if !cx.has_global::<BaseKeyBindings>() {
        let base = cx.key_bindings().borrow().bindings().cloned().collect();
        cx.set_global(BaseKeyBindings(base));
    }
    apply_keybindings(&BTreeMap::new(), cx);
}

/// Re-applies all Testrix keybindings on top of the framework base keymap.
pub(crate) fn apply_keybindings(custom: &BTreeMap<String, String>, cx: &mut App) {
    if let Some(base) = cx.try_global::<BaseKeyBindings>() {
        let base_bindings = base.0.clone();
        cx.clear_key_bindings();
        cx.bind_keys(base_bindings);
    }

    let resolve = |id: &str| -> &'static str {
        CUSTOMIZABLE_SHORTCUTS
            .iter()
            .find(|s| s.id == id)
            .map(|s| intern_shortcut(&effective_shortcut(custom, s)))
            .unwrap_or("")
    };

    let mut bindings = vec![
        #[cfg(target_os = "macos")]
        KeyBinding::new("cmd-q", Quit, None),
        #[cfg(not(target_os = "macos"))]
        KeyBinding::new("ctrl-q", Quit, None),
        KeyBinding::new(resolve("save_active"), SaveActive, Some(APP_KEY_CONTEXT)),
        KeyBinding::new(resolve("send_request"), SendRequest, Some(APP_KEY_CONTEXT)),
        KeyBinding::new(
            resolve("focus_quick_search"),
            FocusQuickSearch,
            Some(APP_KEY_CONTEXT),
        ),
        KeyBinding::new(resolve("new_request"), NewRequest, Some(APP_KEY_CONTEXT)),
        KeyBinding::new(
            resolve("close_active_tab"),
            CloseActiveTab,
            Some(APP_KEY_CONTEXT),
        ),
        KeyBinding::new(
            resolve("toggle_env_sheet"),
            ToggleEnvSheet,
            Some(APP_KEY_CONTEXT),
        ),
    ];

    if !custom.contains_key("save_active") {
        let save_key = resolve("save_active");
        let alt = if save_key == "cmd-s" {
            "ctrl-s"
        } else {
            "cmd-s"
        };
        bindings.push(KeyBinding::new(alt, SaveActive, Some(APP_KEY_CONTEXT)));
    }

    let find_key = resolve("find");
    for ctx in ["Input", "SearchPanel", APP_KEY_CONTEXT] {
        bindings.push(KeyBinding::new(find_key, Search, Some(ctx)));
    }
    if !custom.contains_key("find") {
        let alt = if find_key == "cmd-f" {
            "ctrl-f"
        } else {
            "cmd-f"
        };
        for ctx in ["Input", "SearchPanel", APP_KEY_CONTEXT] {
            bindings.push(KeyBinding::new(alt, Search, Some(ctx)));
        }
    }

    let replace_key = resolve("replace");
    for ctx in ["Input", "SearchPanel", APP_KEY_CONTEXT] {
        bindings.push(KeyBinding::new(replace_key, Replace, Some(ctx)));
    }
    if !custom.contains_key("replace") {
        let alt = if replace_key == "cmd-h" {
            "ctrl-h"
        } else {
            "cmd-h"
        };
        for ctx in ["Input", "SearchPanel", APP_KEY_CONTEXT] {
            bindings.push(KeyBinding::new(alt, Replace, Some(ctx)));
        }
    }

    cx.bind_keys(bindings);
}
