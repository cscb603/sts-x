use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // Embed Windows icon (only on Windows targets)
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
        let out_dir = env::var("OUT_DIR").expect("OUT_DIR");

        let rc_src = PathBuf::from(&manifest).join("assets/icon.rc");
        let rc_content = fs::read_to_string(&rc_src).expect("read assets/icon.rc");

        // embed-resource 在 macOS 主机用 llvm-rc 编译 .rc 时，会把 .rc 复制到 OUT_DIR
        // 再运行，导致其中相对的 "assets/icon.ico" 找不到。改为基于 manifest 的绝对路径。
        let abs_icon = PathBuf::from(&manifest).join("assets/icon.ico");
        let rc_fixed = rc_content.replace("assets/icon.ico", &abs_icon.to_string_lossy());

        let rc_dst = PathBuf::from(&out_dir).join("icon_embed.rc");
        fs::write(&rc_dst, rc_fixed).expect("write icon_embed.rc");

        embed_resource::compile(&rc_dst, embed_resource::NONE);
    }
}
