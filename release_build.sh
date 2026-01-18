#!/bin/bash
set -e

echo "=== Starting Xray-Lite Release Build ==="

# 1. 编译 eBPF 字节码 (Nightly)
# 这是 XDP 功能的基础必须先编译
echo "[1/4] Building eBPF bytecode..."
cd xray-lite-ebpf
cargo +nightly build --release --target bpfel-unknown-none -Z build-std=core
cd ..

# 2. 编译标准版 (Musl Static)
# 不带 --features xdp
echo "[2/4] Building Standard (Musl) Version..."
cross_build=false
# 如果没有 musl-gcc，有时需要用 cross 或者专门的 linker，但 rustup target 通常自带
cargo build --release --target x86_64-unknown-linux-musl

# 3. 编译 XDP 版 (GNU Dynamic)
# 带 --features xdp
# 注意: aya 依赖 libbpf/libc，推荐用 gnu
echo "[3/4] Building XDP (GNU) Version..."
cargo build --release --target x86_64-unknown-linux-gnu --features xdp

# 4. 编译 Keygen (Musl)
echo "[4/4] Building Keygen..."
cargo build --release --target x86_64-unknown-linux-musl --bin keygen

# 5. 整理产物
echo "Collecting artifacts..."
cp target/x86_64-unknown-linux-musl/release/vless-server vless-server-linux-x86_64
cp target/x86_64-unknown-linux-gnu/release/vless-server vless-server-linux-x86_64-xdp
cp target/x86_64-unknown-linux-musl/release/keygen keygen-linux-x86_64

# Strip symbols to reduce size
strip vless-server-linux-x86_64
strip vless-server-linux-x86_64-xdp
strip keygen-linux-x86_64

echo "=== Build Success! ==="
ls -lh vless-server-linux-x86_64 vless-server-linux-x86_64-xdp keygen-linux-x86_64
