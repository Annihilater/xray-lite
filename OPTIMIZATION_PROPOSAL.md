# Xray-Lite v0.5.0 优化方案

**目标**: 轻量级 + 高性能 + 兼容官方客户端  
**预计提升**: 吞吐量 +20-30%, 内存占用 -15%

---

## 📊 当前性能基线 (v0.4.2)

| 指标 | 当前值 |
|------|--------|
| 二进制大小 | 4.8MB |
| 内存占用 | ~10MB (空闲) |
| 最大连接数 | 4096 |
| Buffer 大小 | 16KB (handler), 64KB (xhttp) |

---

## 🚀 性能优化方案

### 1. 零拷贝优化 (Priority: HIGH)

**当前问题**: `handler.rs` 中数据转发使用 `copy_bidirectional`，内部会分配临时 buffer。

**优化方案**: 使用 `splice()` 系统调用 (Linux 特有) 实现内核级零拷贝。

```rust
// 新文件: src/network/splice.rs
#[cfg(target_os = "linux")]
pub async fn splice_bidirectional<A, B>(a: &mut A, b: &mut B) -> io::Result<(u64, u64)>
where
    A: AsRawFd + AsyncRead + AsyncWrite + Unpin,
    B: AsRawFd + AsyncRead + AsyncWrite + Unpin,
{
    use std::os::unix::io::AsRawFd;
    // 使用 pipe + splice 实现零拷贝
    // 数据直接在内核态从 socket a 传到 socket b，不经过用户态
}
```

**预期效果**: CPU 使用降低 10-15%, 大文件传输速度提升 20%+

---

### 2. 目标连接池 (Priority: HIGH)

**当前问题**: 每个请求都新建到目标服务器的 TCP 连接。

**优化方案**: 为热点目标维护连接池。

```rust
// 新文件: src/network/pool.rs
use dashmap::DashMap;
use tokio::net::TcpStream;

pub struct ConnectionPool {
    pools: DashMap<String, Vec<TcpStream>>,  // 按目标地址分组
    max_idle_per_host: usize,                 // 每个目标最多空闲连接数
    idle_timeout: Duration,                   // 空闲超时
}

impl ConnectionPool {
    pub async fn get_or_connect(&self, addr: &str) -> Result<TcpStream> {
        // 优先从池中取，否则新建
    }
    
    pub fn return_connection(&self, addr: &str, conn: TcpStream) {
        // 连接用完后归还池
    }
}
```

**预期效果**: 减少 TCP 握手延迟，高频访问目标响应时间降低 30-50ms

---

### 3. Buffer 池化 (Priority: MEDIUM)

**当前问题**: 每个连接都分配新的 16KB/64KB buffer。

**优化方案**: 全局 buffer 池复用。

```rust
// 新文件: src/utils/buffer_pool.rs
use std::sync::Arc;
use crossbeam::queue::ArrayQueue;

pub struct BufferPool {
    pool_16k: Arc<ArrayQueue<BytesMut>>,
    pool_64k: Arc<ArrayQueue<BytesMut>>,
}

impl BufferPool {
    pub fn get_16k(&self) -> PooledBuffer {
        self.pool_16k.pop().unwrap_or_else(|| BytesMut::with_capacity(16384))
    }
    
    // Drop 时自动归还
}
```

**预期效果**: 减少内存分配/释放开销，GC 压力降低（虽然 Rust 没 GC，但 jemalloc 也有开销）

---

### 4. 智能 TCP_CORK (Priority: MEDIUM)

**当前问题**: XHTTP 的 Traffic Shaping 发送很多小包，可能触发 Nagle 算法延迟。

**优化方案**: 批量数据时先 cork，发完再 uncork。

```rust
// 在 h2.rs 的 send_split_data 中
fn send_split_data_optimized(/* ... */) -> Result<()> {
    // 设置 TCP_CORK，暂时不发送
    set_tcp_cork(fd, true);
    
    // 发送所有分片
    while src.has_remaining() {
        let chunk = src.split_to(chunk_size).freeze();
        send_stream.send_data(chunk, false)?;
    }
    
    // 取消 cork，一次性发送
    set_tcp_cork(fd, false);
}
```

**预期效果**: 减少小包数量，网络效率提升

---

### 5. VLESS 请求解析优化 (Priority: LOW)

**当前问题**: `VlessRequest::decode` 每次都检查 UUID 是否在列表中 (O(n) 查找)。

**优化方案**: 使用 HashSet 替代 Vec。

```rust
// src/protocol/vless/codec.rs
pub struct VlessCodec {
    allowed_uuids: HashSet<Uuid>,  // 改用 HashSet
}
```

**预期效果**: 当用户数多时 (>10)，UUID 验证速度提升

---

## ✨ 新特性方案 (不影响客户端兼容)

### 1. 实时流量统计 (Priority: HIGH)

**API**: Unix Socket 或 HTTP 端点

```rust
// 新文件: src/stats/mod.rs
pub struct Stats {
    pub connections_active: AtomicU64,
    pub connections_total: AtomicU64,
    pub bytes_up: AtomicU64,
    pub bytes_down: AtomicU64,
    pub per_user: DashMap<Uuid, UserStats>,
}

// 查询接口
GET /api/stats
{
    "connections_active": 42,
    "connections_total": 12345,
    "bytes_up": 1073741824,
    "bytes_down": 5368709120,
    "uptime_seconds": 86400
}
```

**实现成本**: 低，只需在关键路径加计数器

---

### 2. 配置热重载 (Priority: MEDIUM)

**触发方式**: 监听 SIGHUP 信号

```rust
// src/main.rs
tokio::spawn(async move {
    let mut signals = signal(SignalKind::hangup())?;
    while signals.recv().await.is_some() {
        info!("收到 SIGHUP，重新加载配置...");
        let new_config = Config::load(&config_path)?;
        // 更新 UUID 列表等，无需重启
    }
});
```

**限制**: 端口和 Reality 密钥不支持热重载（需要重启）

**支持热重载的配置**:
- 客户端 UUID 列表
- Sniffing 开关
- 日志级别

---

### 3. 速率限制 (Priority: MEDIUM)

**场景**: 防止单用户滥用带宽

```rust
// 新配置字段
{
  "clients": [{
    "id": "uuid-here",
    "rateLimit": "100Mbps"  // 可选
  }]
}

// 实现: 令牌桶算法
pub struct RateLimiter {
    tokens: AtomicU64,
    rate: u64,  // bytes per second
    last_refill: AtomicU64,
}
```

---

### 4. Prometheus 指标导出 (Priority: LOW)

**端点**: `/metrics`

```
# HELP xray_lite_connections_active Current active connections
# TYPE xray_lite_connections_active gauge
xray_lite_connections_active 42

# HELP xray_lite_bytes_total Total bytes transferred
# TYPE xray_lite_bytes_total counter
xray_lite_bytes_total{direction="up"} 1073741824
xray_lite_bytes_total{direction="down"} 5368709120
```

**好处**: 可以接入 Grafana 做可视化监控

---

### 5. 多入站端口 (Priority: LOW)

**当前**: 一个配置只能一个端口  
**优化**: 支持多端口，不同端口不同配置

```json
{
  "inbounds": [
    { "port": 443, "protocol": "vless", ... },
    { "port": 8443, "protocol": "vless", ... }
  ]
}
```

**注意**: 你当前代码已经支持这个！只是没有充分利用。

---

## 🎯 推荐实施顺序

| 阶段 | 优化项 | 预计工时 | 收益 |
|------|--------|----------|------|
| **v0.5.0** | 目标连接池 | 4h | 高 |
| **v0.5.0** | 流量统计 | 3h | 高 (运维必备) |
| **v0.5.1** | Buffer 池化 | 2h | 中 |
| **v0.5.1** | UUID HashSet | 30min | 低 |
| **v0.6.0** | 零拷贝 splice | 6h | 高 (Linux only) |
| **v0.6.0** | 配置热重载 | 3h | 中 |
| **v0.7.0** | 速率限制 | 4h | 中 |
| **v0.7.0** | Prometheus | 2h | 低 |

---

## 💡 快速起步：流量统计实现

这是最容易实现且最有价值的特性，我可以直接帮你实现：

```rust
// src/stats.rs - 完整实现
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Default, Clone)]
pub struct Stats {
    pub connections_active: Arc<AtomicU64>,
    pub connections_total: Arc<AtomicU64>,
    pub bytes_up: Arc<AtomicU64>,
    pub bytes_down: Arc<AtomicU64>,
}

impl Stats {
    pub fn new() -> Self {
        Self::default()
    }
    
    pub fn connection_opened(&self) {
        self.connections_active.fetch_add(1, Ordering::Relaxed);
        self.connections_total.fetch_add(1, Ordering::Relaxed);
    }
    
    pub fn connection_closed(&self) {
        self.connections_active.fetch_sub(1, Ordering::Relaxed);
    }
    
    pub fn add_bytes(&self, up: u64, down: u64) {
        self.bytes_up.fetch_add(up, Ordering::Relaxed);
        self.bytes_down.fetch_add(down, Ordering::Relaxed);
    }
    
    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            connections_active: self.connections_active.load(Ordering::Relaxed),
            connections_total: self.connections_total.load(Ordering::Relaxed),
            bytes_up: self.bytes_up.load(Ordering::Relaxed),
            bytes_down: self.bytes_down.load(Ordering::Relaxed),
        }
    }
}

#[derive(serde::Serialize)]
pub struct StatsSnapshot {
    pub connections_active: u64,
    pub connections_total: u64,
    pub bytes_up: u64,
    pub bytes_down: u64,
}
```

---

## ❓ 你想先实现哪个？

1. **流量统计** - 最实用，方便监控
2. **目标连接池** - 性能提升最明显
3. **配置热重载** - 运维友好
4. **其他** - 告诉我你的想法

我可以直接帮你实现！
