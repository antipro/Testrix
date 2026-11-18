fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
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
