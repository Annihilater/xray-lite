# Xray-Lite

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)
[![Buy Me A Coffee](https://img.shields.io/badge/Buy%20Me%20A%20Coffee-Support-orange?style=flat&logo=buy-me-a-coffee)](https://buymeacoffee.com/undeadundead)

**English** | **中文** (Bilingual / 双语文档)

> **⚠️ Warning: This project is prohibited for commercial use and is intended for learning and exchange purposes only.**
>
> **⚠️ 警告：本项目禁止商业用途，仅供学习交流使用。**

A lightweight, high-performance VLESS + Reality + XHTTP proxy server implemented in pure Rust. Fully compatible with all Xray/V2Ray clients.

一个轻量级、高性能的纯 Rust 实现的 VLESS + Reality + XHTTP 代理服务器。完全兼容所有 Xray/V2Ray 客户端。

---

## 🚀 Quick Start / 快速开始

### 1. One-Click Installation (Stable) / 一键安装（稳定版）

> **Current Version: v0.4.6**
>
> 适用于生产环境，稳定可靠，兼容所有 Linux 系统。

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/undead-undead/xray-lite/main/install.sh)
```

> **Note**: This is a **static compilation version** that works perfectly on **any Linux system** (Debian, Ubuntu, CentOS, Alpine, etc.) without dependency issues.
>
> **注意**：此为**静态编译版本**，完美适配**任何 Linux 系统** (Debian, Ubuntu, CentOS, Alpine 等)，无需担心依赖问题。

### 2. XDP Installation (Performance Enhanced) / XDP 版安装（性能增强版）

> **Current Version: v0.6.0-xdp (Rate Limit)**
> 
> **Requirements**: Linux Kernel ≥ 5.4 (AMD64 only), Root privileges.

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/undead-undead/xray-lite/feature/dynamic-xdp/install.sh)
```

**New XDP Features / XDP 新特性:**
*   🛡️ **XDP Firewall**: Kernel-level protection against **UDP Floods**, **TCP SYN Floods (Rate Limiting)** & **Illegal Flags**. / 基于 eBPF 技术的内核级 UDP 洪水、TCP SYN 洪水（限流）和非法标志防御。
*   🛑 **Anti-Probe**: Instantly drops **UDP Floods** & **Illegal TCP Packets** (e.g., Null Scan, SYN+FIN). / 在网卡驱动层直接丢弃 UDP 洪水和非法 TCP 包。（支持 TCP 限流）。
*   👻 **Ultimate Stealth**: XDP drops malicious probing packets silently (DROP), while standard Web traffic is allowed (PASS). / XDP 静默丢弃探测包（无法抓包），正常 Web 流量无感放行。
*   🎭 **Smart Protection**: Only protects configured VLESS ports (e.g., 443), allowing other services (SSH) to work normally. / 仅保护配置的 VLESS 端口（如 443），不影响其他服务（如 SSH）。

The script will: / 脚本将自动：
1. Detect Kernel & Architecture / 检测内核与架构
2. Download optimized XDP binary / 下载 XDP 优化版二进制
3. Auto-attach XDP program to NIC / 自动挂载 XDP 程序到网卡
4. Generate keys and start service / 生成密钥并启动服务

./target/release/vless-server --config config.json
```

### 3. io_uring High-Performance Mode / io_uring 高性能模式

> **Current Version: v0.6.0-beta1 (io_uring)**
> 
> **Requirements**: Linux Kernel ≥ 5.10 (AMD64 only).

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/undead-undead/xray-lite/feature/uring-io-optimized/install.sh)
```

> **⚠️ CRITICAL WARNING / 严重警告**
> 
> When using io_uring mode, you **MUST USE VLESS + Reality**.
> **DO NOT USE XHTTP**.
> 
> The io_uring optimization uses a specialized kernel fast path incompatible with XHTTP.
>
> 当使用 io_uring 模式时，**必须使用 VLESS + Reality**。
> **请勿使用 XHTTP**。
> io_uring 优化源于内核快速路径，与 XHTTP 不兼容。

### 4. Build from Source / 从源码构建

```bash
# Clone the repository / 克隆仓库
git clone https://github.com/undead-undead/xray-lite.git
cd xray-lite

# Build / 构建
cargo build --release

# Run / 运行
./target/release/vless-server --config config.json
```

---

## ✨ Features / 特性

| Feature / 特性 | Status / 状态 | Description / 描述 |
|---------------|---------------|---------------------|
| **VLESS Protocol** | ✅ Stable | Full VLESS protocol support / 完整 VLESS 协议支持 |
| **VLESS UDP** | ✅ Stable | UDP over TCP support / UDP over TCP 支持 |
| **Reality** | ✅ Stable | TLS 1.3 with dynamic certificate / TLS 1.3 动态证书 |
| **SNI Sniffing** | ✅ Stable | Auto-detect target domain / 自动嗅探目标域名 |
| **XHTTP (NEW)** | ✅ Universal | Integrates: **<span style="color:red">H2 Ping-Pong</span>**, **<span style="color:red">Traffic Shaping</span>**, **<span style="color:red">Chameleon Headers</span>** |

### Why Xray-Lite? / 为什么选择 Xray-Lite？

- 🚀 **High Performance / 高性能**: Built on Tokio async runtime / 基于 Tokio 异步运行时
- 🪶 **Lightweight / 轻量级**: ~1.5MB binary, ~10MB RAM / 约 1.5MB 二进制文件，约 10MB 内存
- 🛡️ **Intelligent Adaptive / 智能自适应**: Auto-pairing XHTTP Split-Streams for mobile clients / 自动焊接移动端分离流
- 🔒 **Secure / 安全**: Reality protocol resists active probing / Reality 协议抵抗主动探测
- ✅ **Compatible / 兼容**: Works with v2rayN, Shadowrocket, Xray-core, etc. / 兼容所有主流客户端

---



## 🏗️ Technical Details / 技术细节

Xray-Lite features a **Universal Adaptive Engine (UAE)** for XHTTP:
*   **PC/Desktop**: Higher performance H2-Pipe mode.
*   **iOS/Mobile**: Intelligent Session Pairing for XHTTP Split-Streams.
*   **Auto-Sync**: Standard gRPC Framing automatically detected.
*   **Anti-Censorship**: **<span style="color:red">H2 Ping-Pong</span>**, **<span style="color:red">Traffic Shaping</span>**, and **<span style="color:red">Chameleon Headers</span>** combine to defeat deep packet inspection and traffic analysis.

Xray-Lite 拥有针对 XHTTP 的 **全协议自适应引擎 (UAE)**:
*   **电脑端**: 采用极速 H2-Pipe 直传。
*   **移动端**: 自动识别并焊接 XHTTP 分离流（解决小火箭掉线问题）。
*   **自动同步**: 自动识别标准 gRPC 分帧逻辑。
*   **抗封锁**: **<span style="color:red">H2 Ping-Pong</span>** (随机心跳), **<span style="color:red">Traffic Shaping</span>** (流量整形), **<span style="color:red">Chameleon Headers</span>** (拟态伪装) 三位一体，对抗深度包检测与流量分析。

---

## 🐛 Troubleshooting / 故障排除

### Connection Failed / 连接失败

1. **Check firewall / 检查防火墙**
   ```bash
   sudo ufw allow 443/tcp
   ```

2. **Check logs / 查看日志**
   ```bash
   sudo journalctl -u xray-lite -f
   ```

3. **Verify client config / 验证客户端配置**
   - Flow must be empty / Flow 必须为空
   - Public key must match / 公钥必须匹配
   - Short ID must match / 短 ID 必须匹配

---

## 📄 License / 许可证

MIT License - See [LICENSE](LICENSE) for details.

---

## 🙏 Acknowledgments / 致谢

- [Xray-core](https://github.com/XTLS/Xray-core) - Reality & XHTTP design / 协议设计
- [Tokio](https://tokio.rs/) - Async runtime / 异步运行时
- [rustls](https://github.com/rustls/rustls) - TLS implementation / TLS 实现

---

## ☕ Support / 支持

If this project is helpful to you, please consider buying me a coffee!

如果你觉得好用，请我喝杯咖啡吧！
sol:GJu2g8nd5pQMCdPj1uBJ2bdDguSTMXU6uqXmUbYPS9x
base:0xBC14Ef78a454b4D52A1b0605b707b85Eb9A6b9A1
btc:162vtnicREByPgxh6KLbp2tknXuFCQDHMC
sui:0xd6d896a0ab9ec220c32b17ebc3f641a3a1d7fa140c3c03d9307797704132dc78
**[☕ Buy Me a Coffee / 请我喝咖啡](https://buymeacoffee.com/undeadundead)**

---

**Note / 注意**: This project is for learning and research purposes only. Please comply with local laws and regulations. / 本项目仅供学习和研究使用，请遵守当地法律法规。
