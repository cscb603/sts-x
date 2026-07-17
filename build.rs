fn main() {
    // Embed Windows icon (only on Windows targets)
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        embed_resource::compile("assets/icon.rc", embed_resource::NONE);
    }
}
