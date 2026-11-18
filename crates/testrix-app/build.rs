fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "Testrix");
        res.set("FileDescription", "Testrix — YAML API Workbench");
        res.compile().expect("failed to compile Windows icon resource");
    }
}
