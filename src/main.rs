/*
 * main.rs
 * Project: sts-x
 * Description: CLI binary entry point
 */

use clap::Parser;
use sts_x::cli::{self, Cli};

/// v3.3.1: onnxruntime 动态库自动探测（语义检索开箱即用，零配置）。
///
/// `ort` load-dynamic 的默认查找并不覆盖我们发布包的布局：
/// - macOS dyld 不搜 `/usr/local/lib`（Homebrew 路径）→ dlopen 失败 panic；
/// - Windows LoadLibrary 搜 exe 根目录，但语义包的 dll 放在 `lib/` 子目录。
///
/// 探测顺序（命中即 setenv `ORT_DYLIB_PATH`）：
///   1. 用户显式 `ORT_DYLIB_PATH` → 直接尊重，跳过探测；
///   2. exe 同目录 `lib/` 或同目录（跨平台：覆盖发布包布局）；
///   3. macOS 系统常见路径（`/usr/local/lib`、`/opt/homebrew/lib`）。
fn bootstrap_ort_dylib() {
    if std::env::var_os("ORT_DYLIB_PATH").is_some() {
        return; // 显式指定优先
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for rel in [
                "lib/libonnxruntime.1.28.0.dylib",
                "lib/libonnxruntime.dylib",
                "lib/onnxruntime.dll",
                "libonnxruntime.dylib",
                "onnxruntime.dll",
            ] {
                let p = dir.join(rel);
                if p.exists() {
                    let s = p.display().to_string();
                    std::env::set_var("ORT_DYLIB_PATH", &s);
                    tracing::info!("Auto-set ORT_DYLIB_PATH={s} (semantic search ready)");
                    return;
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        for cand in [
            "/usr/local/lib/libonnxruntime.1.28.0.dylib",
            "/usr/local/lib/libonnxruntime.dylib",
            "/opt/homebrew/lib/libonnxruntime.1.28.0.dylib",
            "/opt/homebrew/lib/libonnxruntime.dylib",
        ] {
            if std::path::Path::new(cand).exists() {
                std::env::set_var("ORT_DYLIB_PATH", cand);
                tracing::info!("Auto-set ORT_DYLIB_PATH={cand} (semantic search ready)");
                return;
            }
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    bootstrap_ort_dylib();

    // Initialize tracing with sensible defaults
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "sts_x=info,tantivy=warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    // Start the background idle GC scanner (reclaims stale index versions
    // during quiet gaps; one-shot CLI modes also get a final sweep on exit).
    sts_x::gc::spawn_index_gc();

    let cli = Cli::parse();
    cli::run(&cli).await?;

    // One-shot CLI modes: reclaim stale index versions on the way out.
    // (The MCP server keeps the background idle GC thread alive instead.)
    sts_x::gc::run_final_gc();

    Ok(())
}
