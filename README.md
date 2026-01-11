# Xray-Lite

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)
[![Buy Me A Coffee](https://img.shields.io/badge/Buy%20Me%20A%20Coffee-Support-orange?style=flat&logo=buy-me-a-coffee)](https://buymeacoffee.com/undeadundead)

**English** | **中文** (Bilingual / 双语文档)

> **⚠️ Warning: This project is prohibited for commercial use and is intended for learning and exchange purposes only.**
>
> **⚠️ 警告：本项目禁止商业用途，仅供学习交流使用。**

A lightweight, high-performance VLESS + Reality proxy server implemented in pure Rust. Fully compatible with all Xray/V2Ray clients.

一个轻量级、高性能的纯 Rust 实现的 VLESS + Reality 代理服务器。完全兼容所有 Xray/V2Ray 客户端。

---

## ✨ Features / 特性

| Feature / 特性 | Status / 状态 | Description / 描述 |
|---------------|---------------|---------------------|
| **VLESS Protocol** | ✅ Stable | Full VLESS protocol support / 完整 VLESS 协议支持 |
| **VLESS UDP** | ✅ Stable | UDP over TCP support / UDP over TCP 支持 |
| **Reality** | ✅ Stable | TLS 1.3 with dynamic certificate / TLS 1.3 动态证书 |
| **SNI Sniffing** | ✅ Stable | Auto-detect target domain / 自动嗅探目标域名 |
| **XHTTP (NEW)** | ✅ Universal | Universal compatibility (PC, iOS, Android) / 全端完美适配 (PC, iOS, 安卓) |

### Why Xray-Lite? / 为什么选择 Xray-Lite？

- 🚀 **High Performance / 高性能**: Built on Tokio async runtime / 基于 Tokio 异步运行时
- 🪶 **Lightweight / 轻量级**: ~1.5MB binary, ~10MB RAM / 约 1.5MB 二进制文件，约 10MB 内存
- 🛡️ **Intelligent Adaptive / 智能自适应**: Auto-pairing XHTTP Split-Streams for mobile clients / 自动焊接移动端分离流
- 🔒 **Secure / 安全**: Reality protocol resists active probing / Reality 协议抵抗主动探测
- ✅ **Compatible / 兼容**: Works with v2rayN, Shadowrocket, Xray-core, etc. / 兼容所有主流客户端

---

## 🚀 Quick Start / 快速开始

### One-Click Installation (Recommended) / 一键安装（推荐）

```bash
bash <(curl -fsSL https://raw.githubusercontent.com/undead-undead/xray-lite/main/install.sh)
```

The script will: / 脚本将自动：
1. Download the latest binary / 下载最新二进制文件
2. Generate keys and UUID / 生成密钥和 UUID
3. Configure systemd service / 配置 systemd 服务
4. Start the server / 启动服务器
5. Display connection info / 显示连接信息

### Build from Source / 从源码构建

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

## 📱 Client Configuration / 客户端配置

### Supported Clients / 支持的客户端

| Client / 客户端 | Platform / 平台 | Status / 状态 |
|-----------------|-----------------|---------------|
| v2rayN | Windows | ✅ Full Support |
| v2rayNG | Android | ✅ Full Support |
| Shadowrocket | iOS | ✅ Full Support (XHTTP OK) |
| Stash | iOS | ✅ Full Support |
| Passwall | OpenWrt | ✅ Full Support |
| Xray-core | CLI | ✅ Full Support |

> **Note**: For best results on mobile (iOS/Android), XHTTP is recommended. The server will automatically adapt to your client type.
> 
> **注意**：移动端建议使用 **XHTTP** 协议。服务器会自动识别并适配您的客户端模式。

### Configuration Parameters / 配置参数

After installation, you will see: / 安装后会显示：

```
Server Information / 服务器信息:
  IP: YOUR_SERVER_IP
  Port / 端口: 443
  UUID: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx
  Public Key / 公钥: xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
  Short ID / 短 ID: xxxxxxxxxxxxxxxx
```

### v2rayN Configuration / v2rayN 配置

| Field / 字段 | Value / 值 |
|--------------|------------|
| Address / 地址 | Your server IP / 服务器 IP |
| Port / 端口 | 443 |
| UUID | From installation output / 安装输出的 UUID |
| Flow / 流控 | **Leave empty / 留空** |
| Encryption / 加密 | none |
| Network / 传输协议 | **xhttp** or **tcp** |
| Security / 传输层安全 | reality |
| SNI | www.microsoft.com |
| Public Key / 公钥 | From installation output / 安装输出的公钥 |
| Short ID / 短 ID | From installation output / 安装输出的短 ID |
| Fingerprint / 指纹 | chrome |

---

## ⚙️ Service Management / 服务管理

```bash
# Start / 启动
sudo systemctl start xray-lite

# Stop / 停止
sudo systemctl stop xray-lite

# Restart / 重启
sudo systemctl restart xray-lite

# Status / 状态
sudo systemctl status xray-lite

# Logs / 日志
sudo journalctl -u xray-lite -f
```

---

## 🏗️ Technical Details / 技术细节

Xray-Lite features a **Universal Adaptive Engine (UAE)** for XHTTP:
*   **PC/Desktop**: Higher performance H2-Pipe mode.
*   **iOS/Mobile**: Intelligent Session Pairing for XHTTP Split-Streams.
*   **Auto-Sync**: Standard gRPC Framing automatically detected.

Xray-Lite 拥有针对 XHTTP 的 **全协议自适应引擎 (UAE)**:
*   **电脑端**: 采用极速 H2-Pipe 直传。
*   **移动端**: 自动识别并焊接 XHTTP 分离流（解决小火箭掉线问题）。
*   **自动同步**: 自动识别标准 gRPC 分帧逻辑。

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

**[☕ Buy Me a Coffee / 请我喝咖啡](https://buymeacoffee.com/undeadundead)**

---

**Note / 注意**: This project is for learning and research purposes only. Please comply with local laws and regulations. / 本项目仅供学习和研究使用，请遵守当地法律法规。
