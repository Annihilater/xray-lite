#!/bin/bash
set -e

# Configuration
VERSION="v0.2.8"
TARGET="x86_64-unknown-linux-gnu"
OUTPUT_DIR="release_artifacts"

echo "🚀 Starting Xray-Lite Release Build ($VERSION)..."

# Ensure we are in the project root
cd "$(dirname "$0")"

# 1. Build Binaries (Release Mode)
echo "🦀 Compiling binaries..."
# Building vless-server with local rustls fork (handled by Cargo.toml patch)
cargo build --release --bin vless-server
cargo build --release --bin keygen
cargo build --release --bin genconfig

# 2. Verify binaries existence
if [[ ! -f "target/release/vless-server" ]] || [[ ! -f "target/release/keygen" ]]; then
    echo "❌ Build failed: binaries not found!"
    exit 1
fi

# 3. Create Artifact Directory
mkdir -p "$OUTPUT_DIR"

# 4. Packaging
echo "📦 Creating artifact: xray-lite-${TARGET}.tar.gz"

TMP_DIR=$(mktemp -d)
cp target/release/vless-server "$TMP_DIR/"
cp target/release/keygen "$TMP_DIR/"
cp target/release/genconfig "$TMP_DIR/"

# Create tarball
# Structure: flat (install.sh expects files in root of tar)
cd "$TMP_DIR"
tar -czvf "$OLDPWD/$OUTPUT_DIR/xray-lite-${TARGET}.tar.gz" .
cd - > /dev/null

# Clean up
rm -rf "$TMP_DIR"

echo "✅ Release Artifact Created Successfully!"
echo "--------------------------------------------------------"
echo "File: $OUTPUT_DIR/xray-lite-${TARGET}.tar.gz"
echo "--------------------------------------------------------"
echo "Next Steps:"
echo "1. Git commit & push."
echo "2. Create GitHub Release tag: $VERSION"
echo "3. Upload the .tar.gz file to the release."
echo "--------------------------------------------------------"
