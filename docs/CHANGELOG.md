# Changelog

All notable changes to Testrix are documented in this file.

## [Unreleased]

## [v0.9.0] - 2026-11-23

### Changed
- Replace `git` CLI subprocesses in `testrix-storage` with pure-Rust `gix` (`gitoxide`) for in-process repository initialization, commits, history, and 3-way text merge (eliminating Windows console window flashes and external `git` binary requirements).
- Make bidirectional Git sync with `node-web` incremental using `knownHeadOid` delta pulls and changed/deleted-only delta pushes, and trigger auto-sync on environment and folder saves.

## [v0.8.0] - 2026-11-23

### Added
- Add workspace Git history and bidirectional `node-web` sync, auto-starting global Git sync using the shared `node-web` account.
- Prompt to sync on workspace switch and ask whether to delete cloud data on logout.
- Add Windows single-instance system tray support.

### Fixed
- Semantically merge environment YAML variables during Git sync, keep valid YAML on 3-way merge conflicts instead of writing conflict markers, and preserve local files when syncing with an empty remote Git repository.
- Prevent Sign Out dialog footer buttons from overflowing and keep them in a single row.
- Auto-start the `optional/node-web` server when required by Passenger/LiteSpeed.
- Fix compiler warnings in `tray` and `app_view`.

## [v0.7.0] - 2026-11-21

### Added
- Add Find (`Ctrl/Cmd+F`) in Request Body (JSON and Raw), Response Body, and Post-Script, plus Replace (`Ctrl/Cmd+H`) in Request Body and Post-Script.
- Add a left navigation list (`General`, `Appearance`, `Keybindings`, `Share & Sync`) to the Preferences window.
- Add UI Font Size (`Small` / `Large`) and Editor Font Size (`10`–`24 px` for Request Body, Response Body, and Post-Script) controls in Preferences.
- Add customizable Key Map settings in Preferences with per-action Apply/Reset and Reset All.

### Changed
- Move the Light/Dark theme toggle from the titlebar into the Preferences **Appearance** section and persist theme, font sizes, and keybindings in `config.yaml`.

### Fixed
- Hide the Windows taskbar button when minimizing to the system tray and disambiguate `HasWindowHandle::window_handle`.

## [v0.6.0] - 2026-11-20

### Added
- Add GitHub Releases self-upgrade check, download, and installer launch.
- Add CLI share management commands and document them in `SKILL.md`.
- Prompt to replace variable references with their real values when deleting environment variables.
- Add a **Path** tab next to **Params** for editing URL path variables (`:param` and `{param}`).

### Changed
- Skip auto-update checks in dev mode and enforce using the installed CLI in `SKILL.md`.

### Fixed
- Close the variable removal confirmation dialog after completing an action.
- Treat `{{...}}` strictly as environment variables while keeping `:param` and `{param}` for URL path variables, and prevent URL variable hint clipping.

## [v0.5.0] - 2026-11-20

### Added
- Support drag-and-drop in the Explorer panel to move HTTP requests across folders and projects, updating disk files and open tabs.
- Add History & Package sidebar views with vertical split, cross-view drag-and-drop, and Windows 11 icon sizing.
- Add request body meta info tree table, web share support, and Windows MSI install directory / desktop shortcut options.
- Add MySQL database support alongside SQLite in `optional/node-web` (configured via `DB_TYPE` and `MYSQL_*` environment variables).
- Add a landing index page in `optional/node-web` and restrict shared project viewing to direct `/s/<slug>` share links.
- Add a configurable End User License Agreement (`REQUIRE_EULA`, enabled by default) to the `node-web` registration page.

### Changed
- Use a normal text field instead of a masked password field for Share Access Token in Preferences, and sync active remote shares when saving the access token.

### Fixed
- Load `.env` on `optional/node-web` server startup so environment settings such as `REQUIRE_EMAIL_VERIFICATION` and `DB_TYPE` take effect.
- Remove unused `project_id` field from `ApplyPostScriptEnv` to eliminate compiler warnings.

## [v0.4.0] - 2026-11-19

### Added
- Embed build metadata (`version`, `build_time` with timezone, `branch`, `commit`) into `build_info.json` during GitHub Actions releases.
- Add an **About** dialog displaying version, build time, branch, last commit, creator email (`support@bitifyware.com`), and official website (`https://bitifyware.com`).

### Fixed
- Install GTK3, Ayatana AppIndicator, and `libxdo` development libraries in the Linux CI job for `tray-icon` builds.

## [v0.3.0] - 2026-11-19

### Added
- System tray support with close-to-tray behavior, first-close confirmation prompt, and a close-behavior setting in the Preferences window.
- Sort environment profile variables by special symbol, number, and then alphabet.
- Persist Explorer folder/project expanded state without auto-expanding everything on startup.
- Share environment profiles across projects as standalone workspace YAML files.
- Import response examples and schema meta-info from OpenAPI, Postman, and Apifox; add OpenAPI export from the project context menu.
- Response examples/meta tree, web codegen snippets, share auto-sync on save, profile merge, script import, and CLI CRUD API.
- Build universal macOS `.dmg` (`arm64` + `x86_64`).

### Changed
- Move toast notifications to the bottom-right corner.
- Use the Upload icon for the import menu.

### Fixed
- Keep focus inside the environment sheet after saving so the close button always works.
- Bind `Ctrl+S` to `SaveActive` on macOS as well as `Cmd+S`.
- Restore persisted shares using `ShareChannel::from_label` and persist remote share records for auto-sync with request-level URL routing in web viewers.
- Decode Windows OEM console encoding for outbound IP adapter names.

## [v0.2.0] - 2026-11-18

### Fixed
- Strip macOS binary and set explicit DMG size for `hdiutil`.
- Add `package.description` to `testrix-app` for `cargo-generate-rpm`.
- Pin WiX Toolset to `5.0.2` in Windows CI to avoid v7 OSMF EULA errors.
- Specify Rust toolchain `1.100.0` for `dtolnay/rust-toolchain`.

## [v0.1.0] - 2026-11-18

### Added
- Initial cross-platform release workflow and packaging for macOS (`.dmg`), Windows (`.msi`, `.exe`), and Linux (`.deb`, `.rpm`).
- Core YAML-backed HTTP request workspace, environment profiles, Explorer tree, request editor, response viewer, and post-response scripting.
