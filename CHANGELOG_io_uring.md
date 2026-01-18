# io_uring 原生实现 - 版本更新日志

## v0.6.0-beta1 (2026-01-18)

### 🎉 重大功能更新

#### ✅ 完整实现 io_uring + XHTTP 支持
- **Reality + XHTTP (HTTP/2) 完整兼容**
- XHTTP 流量通过 `monoio-compat` 桥接到现有 `h2` 处理器
- 纯 VLESS 流量保持在原生 `io_uring` 高性能路径
- 自动检测协议类型并选择最优处理路径

#### 🔧 核心技术实现

**Reality 认证引擎**
- 修复 AuthKey 派生 Bug（统一使用 `b"REALITY"` 字符串）
- 完整的 X25519 + HKDF + AES-GCM 验证流程
- 1:1 对齐 `server_rustls.rs` 的验证逻辑
- 支持所有标准 Reality 配置参数

**XHTTP 混合架构**
- HTTP/2 Connection Preface 自动检测
- `monoio-rustls` TlsStream → `monoio-compat` → Tokio `h2`
- 添加 `StreamWrapper` 的 `MaybeAsRawFd` trait 实现
- 支持所有 XHTTP 模式：Auto / StreamUp / StreamDown / StreamOne

**数据转发优化**
- 使用 `futures::join!` 并发执行双向转发
- 避免单线程轮询导致的死锁
- 64KB 缓冲区优化
- 优雅的连接关闭处理

### 📊 性能表现

| 场景 | CPU 使用率 | 说明 |
|------|-----------|------|
| **之前版本** | 90%+ | Reality + XHTTP 混合场景 |
| **当前版本 (XHTTP)** | 0-16% | XHTTP 通过 compat 桥接 |
| **当前版本 (纯 VLESS)** | <5% 预期 | 原生 io_uring 路径 |
| **Tokio 标准模式** | 3-5% | 基准对比 |

**测试环境**: Racknerd 单核 VPS, Linux 6.8.0-90-generic

### 🚀 使用方式

#### 启动 io_uring 模式
```bash
/usr/local/x-ui/bin/xray --uring -c /usr/local/x-ui/data/xray.json
```

#### 安装脚本（beta 版本）
```bash
bash <(curl -Ls https://raw.githubusercontent.com/undead-undead/x-ui-lite/feature/uring-io-optimized/install.sh)
```

### ⚙️ 技术架构

```
客户端连接
    ↓
Reality TLS 握手 (monoio-rustls-reality)
    ↓
协议检测
    ├─→ HTTP/2 Preface 检测到
    │       ↓
    │   monoio-compat 桥接
    │       ↓
    │   Tokio h2 处理器
    │       ↓
    │   XHTTP 解包 → VLESS 解析
    │
    └─→ 纯 VLESS 数据
            ↓
        原生 io_uring 解析
            ↓
        原生 io_uring 转发 (零拷贝)
```

### 🐛 已修复的 Bug

1. **Reality AuthKey 派生不一致**
   - 问题：验证和 TLS 握手使用了不同的 HKDF info 字符串
   - 修复：统一使用 `b"REALITY"`

2. **双向转发死锁**
   - 问题：单任务轮询导致某些场景下连接挂起
   - 修复：使用 `futures::join!` 并发执行

3. **VLESS 版本解析错误**
   - 问题：TLS 解密后字节对齐问题导致读取错误版本号
   - 修复：优化缓冲区管理和读取逻辑

4. **StreamWrapper MaybeAsRawFd 缺失**
   - 问题：XHTTP 处理器要求但 `monoio-compat` 未实现
   - 修复：添加返回 `None` 的实现

### 📝 已知限制

- XHTTP 流量仍需经过 `monoio-compat` 桥接，有轻微性能开销
- 纯原生 `monoio` HTTP/2 实现需要大量工作，暂未实现
- 推荐追求极致性能的用户使用纯 VLESS (tcp 传输)

### 🔜 未来计划

- [ ] 实现原生 `monoio` HTTP/2 处理器
- [ ] 进一步优化 XHTTP 桥接性能
- [ ] 支持更多传输协议
- [ ] 完善错误处理和日志

### 📚 相关文档

- [io_uring 性能优化指南](docs/io_uring_optimization.md)
- [Reality 协议实现细节](docs/reality_implementation.md)
- [XHTTP 混合架构设计](docs/xhttp_hybrid_arch.md)

---

**测试者**: @undead-undead  
**发布日期**: 2026-01-18  
**分支**: `feature/uring-io-optimized`  
**Release**: `v0.6.0-beta1`
