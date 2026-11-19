# Changelog

All notable changes to Testrix are documented in this file.

## [Unreleased]

### Added
- Support drag-and-drop in the Explorer panel to move HTTP requests across folders and projects, updating disk files and open tabs.

### Fixed
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
