#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use gpui_kit::assets::AllAssets;
use testrix_app::{
    cli_api::{execute_cli_api, format_api_error, is_api_resource},
    init_app, init_system_tray, open_main_window,
};
use testrix_storage::{
    default_workspace_dir, init_or_load_config, load_project, load_workspace, validate_yaml_file,
};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Default, PartialEq, Eq)]
struct CliArgs {
    workspace: Option<PathBuf>,
    check: bool,
    check_paths: Vec<PathBuf>,
    api_args: Vec<String>,
    help: bool,
}

fn parse_cli_args<I>(args: I) -> CliArgs
where
    I: IntoIterator<Item = String>,
{
    let mut parsed = CliArgs::default();
    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        if arg == "--workspace" {
            if let Some(path) = iter.next() {
                parsed.workspace = Some(PathBuf::from(path));
            }
        } else if let Some(path) = arg.strip_prefix("--workspace=") {
            parsed.workspace = Some(PathBuf::from(path));
        } else if arg == "--check" {
            parsed.check = true;
        } else if let Some(path) = arg.strip_prefix("--check=") {
            parsed.check = true;
            if !path.is_empty() {
                parsed.check_paths.push(PathBuf::from(path));
            }
        } else if arg == "-h" || arg == "--help" {
            parsed.help = true;
        } else if is_api_resource(&arg) {
            parsed.api_args.push(arg);
            parsed.api_args.extend(iter);
            break;
        } else if !arg.starts_with('-') {
            parsed.check_paths.push(PathBuf::from(arg));
        }
    }
    parsed
}

fn resolve_startup_workspace_dir(cli_workspace: Option<PathBuf>) -> PathBuf {
    if let Some(cli_dir) = cli_workspace {
        return cli_dir;
    }
    match init_or_load_config() {
        Ok(cfg) => cfg.workspace_dir,
        Err(err) => {
            tracing::warn!(error = %err, "failed loading ~/.testrix/config.yaml; falling back to default workspace");
            default_workspace_dir()
        }
    }
}

fn check_target(path: &Path) -> Result<String, String> {
    if path.is_dir() {
        if path.join("testrix.project.yaml").is_file() {
            load_project(path)
                .map(|project| {
                    format!(
                        "OK project `{}` ({} requests, {} environments) {}",
                        project.doc.name,
                        project.request_count(),
                        project.environments.len(),
                        path.display()
                    )
                })
                .map_err(|err| err.to_string())
        } else {
            load_workspace(path)
                .map(|tree| {
                    format!(
                        "OK workspace ({} projects) {}",
                        tree.projects.len(),
                        path.display()
                    )
                })
                .map_err(|err| err.to_string())
        }
    } else {
        validate_yaml_file(path)
            .map(|kind| format!("OK {kind:?} {}", path.display()))
            .map_err(|err| err.to_string())
    }
}

fn run_check(cli: &CliArgs) -> ExitCode {
    let targets = if cli.check_paths.is_empty() {
        vec![resolve_startup_workspace_dir(cli.workspace.clone())]
    } else {
        cli.check_paths.clone()
    };

    let mut has_error = false;
    for target in &targets {
        match check_target(target) {
            Ok(summary) => println!("{summary}"),
            Err(err) => {
                eprintln!("ERR {}: {err}", target.display());
                has_error = true;
            }
        }
    }

    if has_error {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn print_help() {
    println!(
        "\
Testrix — YAML API Workbench

Usage:
  testrix [OPTIONS] [PATH ...]
  testrix [--workspace <DIR>] <RESOURCE> <ACTION> [FLAGS]

Options:
  --workspace <DIR>       Open or operate on the specified workspace directory
  --check [PATH ...]      Validate Testrix YAML file(s), project, or workspace in console mode
  -h, --help              Print this help message and exit

CLI API Resources (validated JSON CRUD):
  project  <list|get|create|update|delete>
  folder   <list|create|update|delete>
  item     <list|get|create|update|delete>
  profile  <list|get|create|update|delete|merge>
  var      <list|get|create|update|set|delete>
  share    <list|status|config-set|login|publish|revoke>"
    );
}

fn main() -> ExitCode {
    let cli = parse_cli_args(std::env::args().skip(1));

    if cli.help {
        print_help();
        return ExitCode::SUCCESS;
    }

    if cli.check {
        return run_check(&cli);
    }

    if !cli.api_args.is_empty() {
        let workspace_dir = resolve_startup_workspace_dir(cli.workspace);
        return match execute_cli_api(&workspace_dir, &cli.api_args) {
            Ok(json_out) => {
                println!("{json_out}");
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("{}", format_api_error(&err));
                ExitCode::FAILURE
            }
        };
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let workspace_dir = resolve_startup_workspace_dir(cli.workspace);

    gpui_kit::application()
        .with_assets(AllAssets)
        .run(move |cx| {
            init_app(cx);
            let (window_handle, app_entity) =
                open_main_window(workspace_dir, cx).expect("failed to open Testrix window");
            init_system_tray(window_handle, app_entity, cx);
            cx.activate(true);
        });

    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use testrix_storage::ensure_seeded_workspace;

    #[test]
    fn parses_check_and_workspace_cli_flags() {
        let cli = parse_cli_args([
            "--workspace".to_string(),
            "/tmp/ws".to_string(),
            "--check".to_string(),
            "a.request.yaml".to_string(),
            "b.env.yaml".to_string(),
        ]);
        assert!(cli.check);
        assert_eq!(cli.workspace, Some(PathBuf::from("/tmp/ws")));
        assert_eq!(
            cli.check_paths,
            vec![PathBuf::from("a.request.yaml"), PathBuf::from("b.env.yaml")]
        );

        let eq_cli = parse_cli_args(["--check=foo.request.yaml".to_string()]);
        assert!(eq_cli.check);
        assert_eq!(eq_cli.check_paths, vec![PathBuf::from("foo.request.yaml")]);
    }

    #[test]
    fn check_target_validates_seeded_workspace_and_rejects_invalid_yaml() {
        let dir = TempDir::new().unwrap();
        let tree = ensure_seeded_workspace(dir.path()).unwrap();
        let project = &tree.projects[0];

        assert!(check_target(dir.path()).is_ok());
        assert!(check_target(&project.root_dir).is_ok());
        assert!(check_target(&project.manifest_path).is_ok());
        assert!(check_target(&tree.environments[0].file_path).is_ok());
        assert!(check_target(&project.requests[0].file_path).is_ok());

        let bad_req = dir.path().join("bad.request.yaml");
        std::fs::write(
            &bad_req,
            "apiVersion: testrix.io/v1\nkind: Project\nid: x\nname: Bad\n",
        )
        .unwrap();
        let err = check_target(&bad_req).unwrap_err();
        assert!(err.contains("unexpected kind"));
    }
}
