/*
 * main.rs
 * Project: sts-x
 * Description: CLI binary entry point
 */

use clap::Parser;
use sts_x::cli::{self, Cli};

/// Windows 专属：把 onnxruntime.dll 所在目录加入进程 DLL 搜索路径。
///
/// 根因（Win 侧 3.3.3 实测 ERROR_DLL_INIT_FAILED 1114）：
/// onnxruntime.dll 放 `lib/` 子目录时，它依赖的 VC++ 运行库
/// (vcruntime140 / msvcp140 / ...) 按 Windows DLL 搜索顺序
/// （exe 同级 → 系统目录 → PATH）会从系统目录的旧版（如 2019）加载，
/// 版本不匹配 → DLL 初始化失败。把 dll 自身目录加进搜索路径后，
/// 依赖优先从同目录解析（与官方 vc_redist 同目录即可），1114 不再出现。
///
/// 仅语义版会触发（BM25 版不加载外部 dll，不受影响）；副作用仅限于
/// 整个进程把该目录作为 DLL 搜索首选，而语义版的所有外部依赖都在那里。
#[cfg(windows)]
fn win_add_dll_dir(dir: &std::path::Path) {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = dir
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0u16))
        .collect();
    unsafe {
        SetDllDirectoryW(wide.as_ptr());
    }
}

#[cfg(windows)]
unsafe extern "system" {
    fn SetDllDirectoryW(lpPathName: *const u16) -> i32;
}

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
    if let Some(p) = std::env::var_os("ORT_DYLIB_PATH") {
        // 显式指定优先：同样把其所在目录加入搜索路径，避免 VC++ 依赖 1114
        #[cfg(windows)]
        {
            let path = std::path::Path::new(&p);
            if let Some(parent) = path.parent() {
                win_add_dll_dir(parent);
            }
        }
        return;
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
                    // 让 onnxruntime.dll 的 VC++ 依赖从自身目录解析（防 1114）
                    #[cfg(windows)]
                    if let Some(parent) = p.parent() {
                        win_add_dll_dir(parent);
                    }
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
