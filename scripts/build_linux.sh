#!/usr/bin/env bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"

echo "======================================================"
echo "   Bifaria - Linux Build Script (.AppImage / .deb)"
echo "======================================================"

# Check and install system packages if apt is available
if command -v apt-get &> /dev/null; then
    echo "--> Installing Linux system dependencies for Tauri..."
    sudo apt-get update && sudo apt-get install -y \
        libwebkit2gtk-4.1-dev \
        build-essential \
        curl \
        wget \
        file \
        cmake \
        clang \
        pkg-config \
        libxdo-dev \
        libssl-dev \
        libayatana-appindicator3-dev \
        librsvg2-dev \
        libasound2-dev
fi

cd "$ROOT_DIR"
echo "--> Installing frontend dependencies..."
if command -v bun &> /dev/null; then
    bun install
    bun run build
    bun run tauri build
elif command -v npm &> /dev/null; then
    npm install
    npm run build
    npx tauri build
fi

echo ""
echo "======================================================"
echo "✓ Linux build completed!"
echo "Generated packages:"
ls -lh "$ROOT_DIR/src-tauri/target/release/bundle/"*/* 2>/dev/null || true
echo "======================================================"
