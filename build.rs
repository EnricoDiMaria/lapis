fn main() {
    println!("cargo:rerun-if-changed=assets/lapis.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        if std::path::Path::new("assets/lapis.ico").exists() {
            res.set_icon("assets/lapis.ico");
        }
        res.set("ProductName", "Lapis");
        res.set("FileDescription", "Lapis - Advanced PDF study suite");
        res.set("OriginalFilename", "lapis.exe");
        if let Err(e) = res.compile() {
            panic!("winresource: {e}");
        }
    }
}
