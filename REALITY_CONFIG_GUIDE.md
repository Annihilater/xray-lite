# Reality 配置完整指南

## 问题诊断

如果你看到 `Client Alert: 2/42` (bad_certificate)，最可能的原因是：

### 1. 密钥配置不匹配

**症状**：客户端一直报 `bad_certificate`

**原因**：服务器的 `privateKey` 和客户端的 `publicKey` 不匹配

**解决方案**：

```bash
# 1. 生成新的密钥对
cd /opt/xray-lite
./keygen

# 输出示例：
# Private Key: gKFN8qV7QMx8_Xqh3qvPkYrT2vN9sL1mK3jH4fG5dE8=
# Public Key: yH7fG4jK3mL1sN9vT2rYkPvq3hqX8_xMQ7Vq8NFKg=

# 2. 更新服务器配置
sudo nano /opt/xray-lite/config.json

# 将 "privateKey" 替换为生成的 Private Key
# 例如：
# "privateKey": "gKFN8qV7QMx8_Xqh3qvPkYrT2vN9sL1mK3jH4fG5dE8="

# 3. 重启服务
sudo systemctl restart xray-lite

# 4. 更新客户端配置
# 将客户端的 "publicKey" 替换为生成的 Public Key
# 例如：
# "publicKey": "yH7fG4jK3mL1sN9vT2rYkPvq3hqX8_xMQ7Vq8NFKg="
```

### 2. dest 配置错误

**症状**：客户端 SNI 与 dest 不匹配

**当前状态**：
- 客户端 SNI: `www.microsoft.com`
- 服务器 dest: `www.apple.com:443` (从 config.example.json)

**解决方案**：

```bash
# 更新服务器配置中的 dest 和 serverNames
sudo nano /opt/xray-lite/config.json

# 修改为：
{
  "dest": "www.microsoft.com:443",
  "serverNames": [
    "www.microsoft.com",
    "*.microsoft.com"
  ]
}

# 重启服务
sudo systemctl restart xray-lite
```

### 3. 完整配置示例

```json
{
  "inbounds": [
    {
      "protocol": "vless",
      "listen": "0.0.0.0",
      "port": 443,
      "settings": {
        "clients": [
          {
            "id": "b831381d-6324-4d53-ad4f-8cda48b30811",
            "flow": "",
            "email": "user@example.com"
          }
        ],
        "decryption": "none"
      },
      "streamSettings": {
        "network": "tcp",
        "security": "reality",
        "realitySettings": {
          "dest": "www.microsoft.com:443",
          "serverNames": [
            "www.microsoft.com",
            "*.microsoft.com"
          ],
          "privateKey": "gKFN8qV7QMx8_Xqh3qvPkYrT2vN9sL1mK3jH4fG5dE8=",
          "shortIds": [
            "0123456789abcdef"
          ],
          "fingerprint": "chrome"
        }
      }
    }
  ]
}
```

## 验证步骤

```bash
# 1. 检查服务状态
sudo systemctl status xray-lite

# 2. 查看实时日志
journalctl -u xray-lite -f

# 3. 验证配置
cat /opt/xray-lite/config.json | grep -A 10 "realitySettings"

# 4. 测试连接
# 使用客户端连接，观察日志中的错误信息
```

## 常见错误代码

- `Alert 2/10` (unexpected_message): 握手消息顺序错误
- `Alert 2/42` (bad_certificate): 证书验证失败或 Reality 认证失败
- `Alert 2/50` (decode_error): 消息格式错误

## 下一步

1. 运行 `./keygen` 生成新密钥对
2. 更新服务器和客户端配置
3. 确保 `dest` 与客户端 SNI 匹配
4. 重启服务并测试
