#!/usr/bin/env bash
set -euo pipefail

# STS-X Build Script
# Builds single binaries for macOS, Linux, and Windows.
# Windows icon: assets/icon.ico (embedded via embed-resource on Windows targets)
#
# Prerequisites:
#   macOS (native):  nothing extra needed
#   Windows:         See build instructions below
#   Linux:           rustup target add x86_64-unknown-linux-gnu
#
# ─── Windows cross-compile from macOS ───
#   brew install mingw-w64
#   rustup target add x86_64-pc-windows-gnu
#   mkdir -p .cargo && cat > .cargo/config.toml << 'EOF'
#   [target.x86_64-pc-windows-gnu]
#   linker = "x86_64-w64-mingw32-gcc"
#   EOF
#   ./scripts/build.sh win
#
# ─── Windows native build ───
#   Install Rust from rustup.rs
#   rustup default stable
#   build.bat / build.ps1 coming soon
#
# Usage:
#   ./scripts/build.sh              # build all
#   ./scripts/build.sh mac          # macOS only
#   ./scripts/build.sh win          # Windows only
#   ./scripts/build.sh linux        # Linux only

SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$SCRIPT_DIR"

BIN_NAME="sts-x"
RELEASE_FLAGS="--release"
ASSETS="assets/"

echo "=== STS-X 多平台构建 ==="
echo "图标: assets/icon.icns (macOS) / assets/icon.ico (Windows)"

build_mac() {
    echo ""
    echo "── macOS (aarch64-apple-darwin) ──"
    cargo build $RELEASE_FLAGS 2>&1
    cp "target/release/$BIN_NAME" "target/release/${BIN_NAME}-macos"
    # Also create minimal .app bundle (optional)
    mkdir -p "target/release/${BIN_NAME}.app/Contents/MacOS"
    mkdir -p "target/release/${BIN_NAME}.app/Contents/Resources"
    cp "assets/icon.icns" "target/release/${BIN_NAME}.app/Contents/Resources/"
    cat > "target/release/${BIN_NAME}.app/Contents/Info.plist" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>$BIN_NAME</string>
    <key>CFBundleIdentifier</key>
    <string>com.xtap.sts-x</string>
    <key>CFBundleName</key>
    <string>STS-X</string>
    <key>CFBundleDisplayName</key>
    <string>STS-X 代码搜索引擎</string>
    <key>CFBundleIconFile</key>
    <string>icon</string>
    <key>CFBundleVersion</key>
    <string>0.1.0</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.0</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHumanReadableCopyright</key>
    <string>Copyright © 2026 x tap. MIT License.</string>
    <key>CFBundleGetInfoString</key>
    <string>STS-X v0.1.0 — AI 代码搜索引擎。AST 感知切块 + BM25 + MCP 服务，17MB 单二进制零依赖。</string>
</dict>
</plist>
PLIST
    # Copy binary into app bundle
    cp "target/release/$BIN_NAME" "target/release/${BIN_NAME}.app/Contents/MacOS/"
    chmod +x "target/release/${BIN_NAME}.app/Contents/MacOS/$BIN_NAME"
    echo "  macOS:  target/release/${BIN_NAME}-macos ($(ls -lh target/release/${BIN_NAME}-macos | awk '{print $5}'))"
    echo "  Bundle: target/release/${BIN_NAME}.app"
}

build_win() {
    echo ""
    echo "── Windows (x86_64-pc-windows-gnu) ──"
    TARGET="x86_64-pc-windows-gnu"

    if ! rustup target list --installed | grep -q "$TARGET"; then
        echo "  ❌ 目标 $TARGET 未安装"
        echo "  运行: rustup target add $TARGET"
        return 1
    fi
    if ! x86_64-w64-mingw32-gcc --version &>/dev/null 2>&1; then
        echo "  ❌ mingw-w64 未安装或不在 PATH 中"
        echo "  运行: brew install mingw-w64"
        return 1
    fi

    cargo build $RELEASE_FLAGS --target "$TARGET" 2>&1
    cp "target/$TARGET/release/${BIN_NAME}.exe" "target/release/${BIN_NAME}-windows.exe"
    echo "  ✅ Windows: target/release/${BIN_NAME}-windows.exe ($(ls -lh target/release/${BIN_NAME}-windows.exe | awk '{print $5}'))"
}

build_linux() {
    echo ""
    echo "── Linux (x86_64-unknown-linux-gnu) ──"
    TARGET="x86_64-unknown-linux-gnu"
    if ! rustup target list --installed | grep -q "$TARGET"; then
        echo "  ❌ 目标 $TARGET 未安装"
        echo "  运行: rustup target add $TARGET"
        return 1
    fi
    cargo build $RELEASE_FLAGS --target "$TARGET" 2>&1
    cp "target/$TARGET/release/$BIN_NAME" "target/release/${BIN_NAME}-linux"
    echo "  ✅ Linux: target/release/${BIN_NAME}-linux ($(ls -lh target/release/${BIN_NAME}-linux | awk '{print $5}'))"
}

case "${1:-all}" in
    mac|macos)
        build_mac
        ;;
    win|windows)
        build_win
        ;;
    linux)
        build_linux
        ;;
    all)
        build_mac
        build_win || echo "  ⚠  Windows 构建跳过"
        build_linux || echo "  ⚠  Linux 构建跳过"
        echo ""
        echo "=== 构建完成 ==="
        for f in target/release/${BIN_NAME}-macos target/release/${BIN_NAME}-windows.exe target/release/${BIN_NAME}-linux; do
            [ -f "$f" ] && echo "  ✅ $f ($(ls -lh "$f" | awk '{print $5}'))" || echo "  ⬜ $f (未生成)"
        done
        echo ""
        echo "macOS .app bundle: target/release/${BIN_NAME}.app"
        echo "图标: assets/ (icon.icns / icon.ico / icon.png)"
        ;;
    *)
        echo "用法: $0 [mac|win|linux|all]"
        exit 1
        ;;
esac
