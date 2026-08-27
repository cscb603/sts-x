#!/usr/bin/env python3
# pack-default.py — sts-x 默认版（零依赖）发布打包
# 产物（dist/）：
#   STS-X-<V>-Mac版(给AI搜代码).zip
#   STS-X-<V>-Win版(给AI搜代码).zip   （Win exe 存在时才打）
#   STS-X-<V>-源码.zip
# 铁律：R1 说明四段式 / R2 Python zipfile（中文名 UTF-8）/ 单文件零依赖
import os, re, zipfile, shutil, subprocess, sys

ROOT = "/Users/xtap/Documents/AI/sts-x"
DIST = os.path.join(ROOT, "dist")
TMP = "/tmp/stsx_pack"
os.makedirs(DIST, exist_ok=True)
os.makedirs(TMP, exist_ok=True)

with open(os.path.join(ROOT, "Cargo.toml"), encoding="utf-8") as f:
    ver = re.search(r'^version\s*=\s*"([^"]+)"', f.read(), re.M).group(1)
print(f"=== sts-x {ver} 默认版打包 ===")

def note(plat: str) -> str:
    if plat == "win":
        install = ("解压到任意目录（如 C:\\tools\\sts-x），然后把该目录加入系统 PATH：\n"
                   "  设置 → 系统 → 关于 → 高级系统设置 → 环境变量 → Path → 新建\n"
                   "  填 C:\\tools\\sts-x（或你解压的目录）→ 确定。之后新开命令行窗口直接 sts-x。")
        blocked = ("Windows：双击 exe 被 SmartScreen 拦 → 点「更多信息 → 仍要运行」；\n"
                   "  或右键 exe → 属性 → 底部「解除锁定」→ 确定。")
        mcp = ('{"mcpServers": {"sts-x": {"command": "C:\\\\tools\\\\sts-x\\\\sts-x.exe", '
               '"args": ["mcp", "-p", "C:\\\\你的项目根"], "env": {}}}}')
        binline = "sts-x.exe"
    else:
        install = ("解压到任意目录，然后：\n  sudo mv sts-x /usr/local/bin/    # 装成全局命令\n之后任何终端直接 sts-x。")
        blocked = ("macOS：终端跑二进制被拦（无法验证开发者）→ 先执行一次：\n  xattr -dr com.apple.quarantine <解压目录>\n再运行即可。")
        mcp = ('{"mcpServers": {"sts-x": {"command": "/usr/local/bin/sts-x", '
               '"args": ["mcp", "-p", "/你的项目根"], "env": {}}}}')
        binline = "sts-x"
    return (
        f"首次打开必看 - sts-x {ver}（{plat} 默认版）\n"
        "==================================================\n\n"
        "1. 先别慌：这是免费分享版，没做付费签名，第一次运行系统可能拦你，不是病毒，不会自动删。\n\n"
        "2. 标准安装（CLI，一次装好全局可用）：\n"
        f"    {install}\n\n"
        "3. 怎么用（给 AI 搜代码 / 给自己搜都行）：\n"
        f"     {binline} --version                              # 应显示 {ver}\n"
        f"     {binline} ai \"缓存\" -p <你的项目路径>            # 中文查询代码（0 命中自动换英文词重试）\n"
        f"     {binline} search \"McpServer\" -p <项目> --locate   # 符号精确定位\n"
        f"     {binline} file \"Cargo.toml\" -p <目录>            # 任意目录零索引文件搜索\n"
        f"     {binline} glob \"**/*.rs\" -p <项目> --top-k 20     # 按模式列举文件（AI 友好排序+截断+诊断）\n\n"
        "4. MCP 接入（AI 客户端，推荐方式 A 原生 stdio）：\n"
        "   在 AI 客户端的 MCP 配置里加（如 ~/.workbuddy/mcp.json）：\n"
        f"   {mcp}\n"
        "   客户端将看到工具：search（代码搜索）/ file（文件搜索）/ glob（按模式列文件）。\n\n"
        "5. 第一次被拦怎么办：\n"
        f"    {blocked}\n\n"
        "6. 支持系统：macOS 12+（Apple Silicon/Intel）/ Windows 10/11 64 位。默认版零依赖，单文件即用。\n\n"
        "祝你用得开心\n"
    )

def zip_one(bin_src: str, zip_name: str, bin_arcname: str) -> None:
    pkg = os.path.join(TMP, zip_name.replace(".zip", ""))
    shutil.rmtree(pkg, ignore_errors=True)
    os.makedirs(pkg, exist_ok=True)
    shutil.copy(bin_src, os.path.join(pkg, bin_arcname))
    with open(os.path.join(pkg, "首次打开必看-sts-x.txt"), "w", encoding="utf-8") as f:
        f.write(note("win" if bin_arcname.endswith(".exe") else "mac"))
    out = os.path.join(DIST, zip_name)
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for fn in sorted(os.listdir(pkg)):
            p = os.path.join(pkg, fn)
            zi = zipfile.ZipInfo.from_file(p, fn)
            zi.compress_type = zipfile.ZIP_DEFLATED
            if fn == bin_arcname:
                zi.external_attr = (0o755 & 0xFFFF) << 16
            with open(p, "rb") as fh:
                z.writestr(zi, fh.read())
    print(f"OK {zip_name} ({os.path.getsize(out)/1048576:.1f}MB)")

mac_bin = os.path.join(ROOT, "target/release/sts-x")
win_bin = os.path.join(ROOT, "target/x86_64-pc-windows-msvc/release/sts-x.exe")

if os.path.exists(mac_bin):
    zip_one(mac_bin, f"STS-X-{ver}-Mac版(给AI搜代码).zip", "sts-x")
else:
    print("SKIP Mac 默认版：二进制缺失，请先 cargo build --release")

if os.path.exists(win_bin):
    zip_one(win_bin, f"STS-X-{ver}-Win版(给AI搜代码).zip", "sts-x.exe")
else:
    print("SKIP Win 默认版：exe 尚未构建（后台 cargo xwin 完成后重跑本脚本）")

# ── 源码包（git ls-files + 未跟踪新源文件；排除模型/二进制）──
print("-- 源码包 --")
src_tmp = os.path.join(TMP, "src")
shutil.rmtree(src_tmp, ignore_errors=True)
os.makedirs(src_tmp, exist_ok=True)
# 已跟踪
files = subprocess.run(["git", "-C", ROOT, "ls-files", "-z"],
                       capture_output=True).stdout.split(b"\0")
for f in files:
    f = f.decode("utf-8")
    if not f:
        continue
    dst = os.path.join(src_tmp, f)
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    shutil.copy(os.path.join(ROOT, f), dst)
# 未跟踪新源文件（排除 target/dist/models/_scratch）
out = subprocess.run(["git", "-C", ROOT, "status", "--porcelain"],
                    capture_output=True, text=True).stdout
for line in out.splitlines():
    if line.startswith("??"):
        f = line[3:].strip()
        if any(f.startswith(p) for p in ("target/", "dist/", "models/", "_scratch/")):
            continue
        if os.path.isfile(os.path.join(ROOT, f)):
            dst = os.path.join(src_tmp, f)
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            shutil.copy(os.path.join(ROOT, f), dst)
# .cargo/config.toml 若未跟踪则带
cc = os.path.join(ROOT, ".cargo/config.toml")
if os.path.isfile(cc) and not subprocess.run(["git", "-C", ROOT, "ls-files", "--error-unmatch", ".cargo/config.toml"],
                                              capture_output=True).returncode == 0:
    d = os.path.join(src_tmp, ".cargo")
    os.makedirs(d, exist_ok=True)
    shutil.copy(cc, os.path.join(d, "config.toml"))
src_out = os.path.join(DIST, f"STS-X-{ver}-源码.zip")
with zipfile.ZipFile(src_out, "w", zipfile.ZIP_DEFLATED) as z:
    for root, _, fs in os.walk(src_tmp):
        for fn in fs:
            p = os.path.join(root, fn)
            z.write(p, os.path.relpath(p, src_tmp))
print(f"OK STS-X-{ver}-源码.zip ({os.path.getsize(src_out)/1048576:.1f}MB)")
print("\n=== dist/ ===")
for n in sorted(os.listdir(DIST)):
    if n.startswith(f"STS-X-{ver}"):
        print(" ", n, f"{os.path.getsize(os.path.join(DIST,n))/1048576:.1f}MB")
