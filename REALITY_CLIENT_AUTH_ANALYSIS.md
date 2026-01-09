# Reality 客户端认证机制详解

## 发现

通过分析 Xray-core 源码，我发现 Reality 的客户端认证比预期的复杂得多。

## 客户端认证流程

### 1. SessionID 结构（32 字节）

```
Bytes 0-2:   Xray 版本号 (Version_x, Version_y, Version_z)
Byte  3:     保留字节 (0)
Bytes 4-7:   Unix 时间戳 (BigEndian uint32)
Bytes 8-15:  shortId (8 字节，配置中的 shortId)
Bytes 16-31: AEAD 加密后的数据 (16 字节)
```

### 2. AuthKey 推导

客户端使用以下步骤生成 AuthKey：

```go
// 1. ECDH 密钥交换
sharedSecret = ECDH(client_private_key, server_public_key)

// 2. HKDF 推导
AuthKey = HKDF-Expand(
    hash: SHA256,
    secret: sharedSecret,
    salt: ClientRandom[:20],  // 前 20 字节
    info: "REALITY",
    length: 32
)
```

### 3. AEAD 加密

客户端使用 AES-GCM 加密 SessionID：

```go
aead = AES-GCM(AuthKey)

// 加密 SessionID 的前 16 字节
encrypted = aead.Seal(
    dst: SessionId[:0],           // 输出到 SessionId
    nonce: ClientRandom[20:32],   // 后 12 字节作为 nonce
    plaintext: SessionId[:16],    // 前 16 字节（版本+时间+shortId）
    additionalData: ClientHello.Raw  // 整个 ClientHello 消息
)

// 结果：SessionId 被就地加密
```

### 4. 服务器验证流程

服务器需要反向执行：

```go
// 1. 从 ClientHello 获取客户端的 KeyShare
clientPublicKey = ExtractFromKeyShare(ClientHello)

// 2. ECDH 密钥交换
sharedSecret = ECDH(server_private_key, clientPublicKey)

// 3. HKDF 推导
AuthKey = HKDF-Expand(
    hash: SHA256,
    secret: sharedSecret,
    salt: ClientRandom[:20],
    info: "REALITY",
    length: 32
)

// 4. AEAD 解密
aead = AES-GCM(AuthKey)
decrypted = aead.Open(
    dst: nil,
    nonce: ClientRandom[20:32],
    ciphertext: SessionId,
    additionalData: ClientHello.Raw
)

// 5. 验证
if decrypted[8:16] in config.shortIds {
    // 认证成功
} else {
    // 认证失败，触发回落
}
```

## 实现挑战

### 挑战 1: 需要访问 ClientHello.Raw

服务器需要完整的 ClientHello 原始字节作为 AEAD 的附加数据。

**解决方案**: 在 rustls 中，需要保存 ClientHello 的原始字节。

### 挑战 2: 需要提取客户端的 KeyShare

需要从 ClientHello 的 KeyShare 扩展中提取客户端的 X25519 公钥。

**解决方案**: rustls 已经解析了 KeyShare，可以访问。

### 挑战 3: 需要 ECDH 和 HKDF

需要执行 X25519 ECDH 和 HKDF-SHA256。

**解决方案**: 使用 `x25519-dalek` 和 `hkdf` crates。

### 挑战 4: 需要 AES-GCM

需要 AEAD 解密。

**解决方案**: 使用 `aes-gcm` crate 或 `ring` 的 AEAD。

## 更新的实现计划

### 短期方案（暂时）

由于完整实现需要：
1. 访问 ClientHello.Raw
2. 提取 KeyShare
3. 执行 ECDH
4. 执行 HKDF
5. 执行 AEAD 解密

这些都需要对 rustls 进行更深入的修改。

**建议**: 暂时使用简化的验证（检查 SessionID 非空），先完成其他部分，然后再回来完善这个。

### 长期方案

1. 修改 rustls 保存 ClientHello.Raw
2. 实现完整的 ECDH + HKDF + AEAD 验证
3. 添加 shortId 配置和验证

## 参考代码

Xray-core 实现位置：
- `Xray-core/transport/internet/reality/reality.go`
- 客户端加密：第 ~140 行
- 服务器验证：需要查看 REALITY fork 的 Go TLS 库

## 下一步

1. **立即**: 使用简化验证完成基本功能
2. **阶段 4**: 实现完整的 ECDH + HKDF + AEAD 验证
3. **阶段 5**: 添加 shortId 配置和多客户端支持
