#!/usr/bin/env python3
"""sts-x 3.3.3 四包打包：Mac / Windows / Linux / 源码。

用 Python zipfile 而非 ditto/zip：强制 UTF-8 文件名，避免中文名乱码与 ._ 垃圾文件。

用法：python3 scripts/pack_all_333.py
"""
import os
import subprocess
import sys
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DIST = os.path.join(ROOT, "dist", "3.3.3")
OUT = os.path.join(ROOT, "dist")
VER = "3.3.3"

DOC_READ = "README.md"
DOC_TXT = os.path.join(DIST, "首次打开必看.txt")
DOC_AR = os.path.join(DIST, "AR-部署使用说明.md")


def add_file(z, src, arcname):
    if not os.path.exists(src):
        print(f"  ⚠️ 缺少 {src}，跳过")
        return
    z.write(src, arcname)
    print(f"  + {arcname}  ({os.path.getsize(src)/1e6:.1f} MB)")


def make_zip(name, items):
    path = os.path.join(OUT, name)
    if os.path.exists(path):
        os.remove(path)
    print(f"\n=== {name} ===")
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for src, arc in items:
            add_file(z, src, arc)
    print(f"  → {os.path.getsize(path)/1e6:.1f} MB")
    return path


def main():
    readme = os.path.join(ROOT, DOC_READ)

    # 1) macOS：给两个二进制（零依赖 + 语义），语义版在本机有 onnxruntime 时自动启用
    make_zip(
        f"sts-x-{VER}-macos-arm64.zip",
        [
            (os.path.join(DIST, "sts-x-3.3.3-macos-arm64"), "sts-x"),
            (os.path.join(DIST, "sts-x-3.3.3-macos-arm64-semantic"), "sts-x-semantic"),
            (DOC_TXT, "首次打开必看.txt"),
            (readme, "README.md"),
            (DOC_AR, "docs/AR-部署使用说明.md"),
        ],
    )

    # 2) Windows：扁平布局 —— BM25 版 + 语义版 同包。语义依赖（onnxruntime.dll + models/）
    #    放在 exe 同级；exe 启动时用 SetDllDirectoryW 把 dll 目录加入搜索路径，消除 1114。
    #    注意：【不】随包 VC++ 运行库。实测发现从 Python venv 提取的 VC++ 2022 运行库
    #    （vcruntime140/msvcp140 等）在部分 Win10 上会导致 onnxruntime 的 DllMain 初始化
    #    失败（ERROR_DLL_INIT_FAILED 1114）；系统自带的 VC++ 2015-2022 再发行件才正确。
    #    故依赖系统运行库，目标机需安装「Microsoft Visual C++ 2015-2022 Redistributable (x64)」。
    win_dll = os.path.join(ROOT, "_scratch", "win-dll", "onnxruntime.dll")
    model_dir = os.path.join(ROOT, "models", "bge-small-zh-v1.5")
    win_items = [
        (os.path.join(DIST, "sts-x-3.3.3-win64.exe"), "sts-x.exe"),
        (os.path.join(DIST, "sts-x-3.3.3-win64-semantic.exe"), "sts-x-semantic.exe"),
        (DOC_TXT, "首次打开必看.txt"),
        (DOC_AR, "AR-部署使用说明.md"),
        (readme, "README.md"),
    ]
    if os.path.exists(win_dll):
        win_items.append((win_dll, "onnxruntime.dll"))
    else:
        print("  ⚠️ 缺 _scratch/win-dll/onnxruntime.dll")
    if os.path.exists(model_dir):
        for fn in ("model.onnx", "tokenizer.json"):
            fp = os.path.join(model_dir, fn)
            if os.path.exists(fp):
                win_items.append((fp, f"models/bge-small-zh-v1.5/{fn}"))
    make_zip(f"sts-x-{VER}-win-x64.zip", win_items)

    # 3) Linux：musl 静态二进制
    make_zip(
        f"sts-x-{VER}-linux-x86_64.zip",
        [
            (os.path.join(DIST, "sts-x-3.3.3-linux-x86_64-musl"), "sts-x"),
            (DOC_TXT, "首次打开必看.txt"),
            (readme, "README.md"),
        ],
    )

    # 4) 源码包：git 已跟踪文件 + core_lib（相对 path 依赖，必须同带）
    print(f"\n=== sts-x-{VER}-src.zip ===")
    src_zip = os.path.join(OUT, f"sts-x-{VER}-src.zip")
    if os.path.exists(src_zip):
        os.remove(src_zip)
    tracked = subprocess.run(
        ["git", "ls-files"], cwd=ROOT, capture_output=True, text=True
    ).stdout.split()
    core_lib = os.path.join(os.path.dirname(ROOT), "rust_master_workspace", "libs", "core_lib")
    n = 0
    with zipfile.ZipFile(src_zip, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for f in tracked:
            p = os.path.join(ROOT, f)
            if os.path.exists(p):
                z.write(p, os.path.join("sts-x", f))
                n += 1
        # core_lib：Cargo.toml 里是 ../rust_master_workspace/libs/core_lib
        if os.path.isdir(core_lib):
            for dirpath, dirnames, filenames in os.walk(core_lib):
                dirnames[:] = [d for d in dirnames if d not in ("target", ".git")]
                for fn in filenames:
                    fp = os.path.join(dirpath, fn)
                    rel = os.path.relpath(fp, os.path.dirname(ROOT))
                    z.write(fp, rel)
                    n += 1
        else:
            print(f"  ⚠️ 缺 core_lib: {core_lib}")
        # 构建说明
        z.write(DOC_AR, os.path.join("sts-x", "AR-部署使用说明.md"))
    print(f"  + {n} 个文件")
    print(f"  → {os.path.getsize(src_zip)/1e6:.1f} MB")

    print("\n✅ 打包完成：", OUT)


if __name__ == "__main__":
    sys.exit(main())
