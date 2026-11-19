fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/build_info.json");

    if let Some(branch) = run_cmd("git", &["rev-parse", "--abbrev-ref", "HEAD"]) {
        println!("cargo:rustc-env=TESTRIX_LOCAL_BRANCH={branch}");
    }
    if let Some(commit) = run_cmd("git", &["rev-parse", "--short=12", "HEAD"]) {
        println!("cargo:rustc-env=TESTRIX_LOCAL_COMMIT={commit}");
    }
    if let Some(build_time) = run_cmd("date", &["+%Y-%m-%d %H:%M:%S %Z (%z)"]) {
        println!("cargo:rustc-env=TESTRIX_LOCAL_BUILD_TIME={build_time}");
    }

    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        // Windows defaults to a 1 MB main-thread stack, which is too small for unoptimized
        // GPUI layout/render frames in dev builds; match macOS/Linux's 8 MB stack.
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
            println!("cargo:rustc-link-arg-bins=-Wl,--stack,8388608");
        } else {
            println!("cargo:rustc-link-arg-bins=/STACK:8388608");
        }

        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "Testrix");
        res.set("FileDescription", "Testrix — YAML API Workbench");
        res.compile().expect("failed to compile Windows icon resource");
    }
}

fn run_cmd(bin: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(bin).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}
