// build.rs — Windows 图标嵌入（使用 winresource，纯 Rust，无需外部 llvm-rc）
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
        winresource::WindowsResource::new()
            .set_icon(&format!("{}/assets/icon.ico", manifest))
            .compile()
            .expect("Failed to compile Windows resource (icon)");
    }
}
