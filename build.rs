// build.rs — Windows 图标嵌入（使用 winresource，纯 Rust，无需外部 llvm-rc）
//
// winresource 在 MSVC 下需要 Windows SDK 的 rc.exe。它通过 RC_PATH 环境变量定位
// rc.exe；若该变量缺失，它会退化成相对路径 "bin\x64\rc.exe" 而失败。
// 这里在 Windows 构建时自动探测 SDK 中的 rc.exe 并写入 RC_PATH，
// 使构建自包含、无需手动设置环境变量。
// 注：仅 CARGO_CFG_TARGET_OS == "windows" 时执行；Mac/Linux 原生构建整段跳过。
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        // 自动探测 rc.exe（仅在用户未显式指定 RC_PATH 时）
        if std::env::var_os("RC_PATH").is_none() {
            if let Some(rc) = find_rc_exe() {
                std::env::set_var("RC_PATH", rc);
            }
        }
        let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
        winresource::WindowsResource::new()
            .set_icon(&format!("{}/assets/icon.ico", manifest))
            .compile()
            .expect("Failed to compile Windows resource (icon)");
    }
}

/// 在 Windows SDK (Program Files (x86)\Windows Kits\10\bin) 中查找最新版本的 rc.exe。
/// 返回绝对路径字符串；找不到时返回 None（交回 winresource 自带的注册表回退）。
fn find_rc_exe() -> Option<String> {
    let base = r"C:\Program Files (x86)\Windows Kits\10\bin";
    let entries = std::fs::read_dir(base).ok()?;
    let mut candidates: Vec<(String, String)> = Vec::new(); // (版本号, 路径)
    for e in entries.flatten() {
        let ver = e.file_name().to_string_lossy().to_string();
        if !ver.starts_with("10.0") {
            continue;
        }
        let p = e.path().join("x64").join("rc.exe");
        if p.exists() {
            candidates.push((ver, p.to_string_lossy().to_string()));
        }
    }
    // 取版本号最大的那份（SDK 通常多版本并存）
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    candidates.last().map(|(_, p)| p.clone())
}
