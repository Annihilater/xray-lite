#!/bin/bash
# wget -qO- https://raw.githubusercontent.com/undead-undead/xray-lite/main/install.sh | bash
set -e

# Color definitions / 颜色定义
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Version / 版本
VERSION="v0.4.2"
REPO="undead-undead/xray-lite"

echo -e "${BLUE}=========================================${NC}"
echo -e "${BLUE}  Xray-Lite One-Click Installation${NC}"
echo -e "${BLUE}  Xray-Lite 一键安装${NC}"
echo -e "${BLUE}  Version / 版本: ${VERSION}${NC}"
echo -e "${BLUE}=========================================${NC}"
echo ""
VERSION="v0.4.2"
# Check if running as root / 检查是否为 root
if [ "$EUID" -ne 0 ]; then 
    echo -e "${RED}Please run as root / 请使用 root 权限运行${NC}"
    echo "sudo bash install.sh"
    exit 1
fi

# Detect architecture / 检测架构
ARCH=$(uname -m)
case $ARCH in
    x86_64)
        BINARY_ARCH="x86_64"
        ;;
    aarch64|arm64)
        BINARY_ARCH="aarch64"
        ;;
    *)
        echo -e "${RED}Unsupported architecture: $ARCH / 不支持的架构: $ARCH${NC}"
        exit 1
        ;;
esac

echo -e "${GREEN}Detected architecture / 检测到架构: $ARCH${NC}"
echo ""

# Stop existing service / 停止现有服务
echo -e "${YELLOW}Checking for existing installation... / 检查现有安装...${NC}"
if systemctl is-active --quiet xray-lite; then
    echo "Stopping existing xray-lite service... / 停止现有 xray-lite 服务..."
    systemctl stop xray-lite >/dev/null 2>&1
    systemctl disable xray-lite >/dev/null 2>&1
fi

# Kill any lingering vless-server processes
pkill -f vless-server || true

echo ""

# Create installation directory / 创建安装目录
INSTALL_DIR="/opt/xray-lite"
echo -e "${YELLOW}[1/6] Creating installation directory... / 创建安装目录...${NC}"
mkdir -p $INSTALL_DIR
cd $INSTALL_DIR
echo -e "${GREEN}✓ Directory created / 目录已创建: $INSTALL_DIR${NC}"
echo ""

# Download Static Binaries / 下载静态二进制文件
echo -e "${YELLOW}[2/6] Downloading Xray-Lite binaries... / 下载 Xray-Lite 二进制文件...${NC}"

XRAY_BINARY="vless-server-linux-${BINARY_ARCH}"
KEYGEN_BINARY="keygen-linux-${BINARY_ARCH}"

DOWNLOAD_PREFIX="https://github.com/${REPO}/releases/download/${VERSION}"
FALLBACK_PREFIX="https://github.com/${REPO}/releases/download/${VERSION}"

echo "Downloading vless-server..."
if curl -fsSL "${DOWNLOAD_PREFIX}/${XRAY_BINARY}" -o "vless-server"; then
    echo -e "${GREEN}✓ vless-server downloaded${NC}"
else
    echo -e "${RED}Failed to download vless-server${NC}"
    exit 1
fi

echo "Downloading keygen..."
if curl -fsSL "${DOWNLOAD_PREFIX}/${KEYGEN_BINARY}" -o "keygen"; then
    echo -e "${GREEN}✓ keygen downloaded${NC}"
else
    echo -e "${RED}Failed to download keygen${NC}"
    exit 1
fi

chmod +x vless-server keygen
echo -e "${GREEN}✓ Files prepared / 文件已准备就绪${NC}"
echo ""

# Generate configuration / 生成配置
echo -e "${YELLOW}[3/6] Generating configuration... / 生成配置...${NC}"

# Generate keys / 生成密钥
KEYGEN_OUTPUT=$(./keygen)
PRIVATE_KEY=$(echo "$KEYGEN_OUTPUT" | grep "Private key:" | awk '{print $3}')
PUBLIC_KEY=$(echo "$KEYGEN_OUTPUT" | grep "Public key:" | awk '{print $3}')

# Generate UUID / 生成 UUID
CLIENT_UUID=$(cat /proc/sys/kernel/random/uuid)

# Get server IP / 获取服务器 IP
SERVER_IP=$(curl -s ifconfig.me 2>/dev/null || curl -s ip.sb 2>/dev/null || echo "YOUR_SERVER_IP")

# Interactive configuration / 交互式配置
echo ""
if [ -t 0 ]; then
    read -p "Server port / 服务器端口 [443]: " PORT_INPUT
    PORT=${PORT_INPUT:-443}
else
    PORT=443
    echo "Non-interactive mode detected using default port 443 / 检测到非交互模式，使用默认端口 443"
fi

if [[ ! "$PORT" =~ ^[0-9]+$ ]]; then
    echo -e "${YELLOW}Invalid port, using default 443 / 端口无效，使用默认 443${NC}"
    PORT=443
fi

if [ -t 0 ]; then
    read -p "Masquerade website / 伪装网站 [www.microsoft.com:443]: " DEST_INPUT
    DEST=${DEST_INPUT:-www.microsoft.com:443}
else
    DEST="www.microsoft.com:443"
fi

DOMAIN=$(echo $DEST | cut -d: -f1)

# Short ID configuration
if command -v openssl &> /dev/null; then
    SHORT_ID=$(openssl rand -hex 8)
else
    SHORT_ID=$(cat /proc/sys/kernel/random/uuid | tr -d '-' | head -c 16)
fi

# XHTTP configuration / XHTTP 配置
# Default to true, as it's a key feature
ENABLE_XHTTP="y" 

if [ "$ENABLE_XHTTP" = "y" ] || [ "$ENABLE_XHTTP" = "Y" ]; then
    echo -e "${GREEN}XHTTP (Anti-probing) enabled / XHTTP (防探测) 已启用${NC}"
    if [ -t 0 ]; then
        read -p "XHTTP path / XHTTP 路径 [/xhttp]: " XHTTP_PATH_INPUT
        XHTTP_PATH=${XHTTP_PATH_INPUT:-/xhttp}
    else
        XHTTP_PATH="/xhttp"
    fi
else
    XHTTP_PATH=""
    echo -e "${YELLOW}XHTTP disabled (Standard VLESS+Reality) / XHTTP 已禁用 (标准 VLESS+Reality)${NC}"
fi

# Build XHTTP settings if enabled
XHTTP_CONFIG=""
if [ -n "$XHTTP_PATH" ]; then
    XHTTP_CONFIG="\"xhttp\": {
    \"mode\": \"auto\",
    \"path\": \"$XHTTP_PATH\",
    \"extra\": {}
  },"
fi

echo -e "${YELLOW}[4/6] Creating config.json... / 创建配置文件...${NC}"

cat <<EOF > config.json
{
  "log": {
    "level": "info"
  },
  "inbounds": [
    {
      "port": $PORT,
      "protocol": "vless",
      "settings": {
        "clients": [
          {
            "id": "$CLIENT_UUID",
            "flow": "xtls-rprx-vision"
          }
        ],
        "decryption": "none"
      },
      "streamSettings": {
        "network": "tcp",
        "security": "reality",
        "realitySettings": {
          "show": false,
          "dest": "$DEST",
          "xver": 0,
          "serverNames": [
            "$DOMAIN"
          ],
          "privateKey": "$PRIVATE_KEY",
          "shortIds": [
            "$SHORT_ID"
          ]
        },
        $XHTTP_CONFIG
        "tcpSettings": {
          "acceptProxyProtocol": false,
          "header": {
            "type": "none"
          }
        }
      },
      "sniffing": {
        "enabled": true,
        "destOverride": [
          "http",
          "tls",
          "quic"
        ]
      }
    }
  ],
  "outbounds": [
    {
      "protocol": "freedom",
      "tag": "direct"
    },
    {
      "protocol": "blackhole",
      "tag": "block"
    }
  ]
}
EOF

# Create client configuration / 创建客户端配置
echo -e "${YELLOW}[5/6] Creating client configuration... / 创建客户端配置...${NC}"

# Start service to test
echo -e "${YELLOW}[6/6] Installing service... / 安装服务...${NC}"

cat <<EOF > /etc/systemd/system/xray-lite.service
[Unit]
Description=Xray-Lite Service
Documentation=https://github.com/$REPO
After=network.target nss-lookup.target

[Service]
User=root
WorkingDirectory=$INSTALL_DIR
ExecStart=$INSTALL_DIR/vless-server -c config.json
Restart=on-failure
RestartSec=10
LimitNOFILE=infinity

[Install]
WantedBy=multi-user.target
EOF

# Set permissions
chmod 644 /etc/systemd/system/xray-lite.service
chmod 755 $INSTALL_DIR/vless-server
chmod 755 $INSTALL_DIR/keygen

systemctl daemon-reload
systemctl enable xray-lite

# Configure firewall / 配置防火墙
if command -v ufw &> /dev/null; then
    if ufw status | grep -q "Status: active"; then
        echo "Opening port $PORT in UFW..."
        ufw allow $PORT/tcp
    fi
fi

# Ensure PORT is numeric again just in case
if [[ ! "$PORT" =~ ^[0-9]+$ ]]; then
    PORT=443
fi

# Check port availability / 检查端口占用
if lsof -i:$PORT -t >/dev/null 2>&1; then
    echo -e "${RED}Warning: Port $PORT is in use. Service might fail to start.${NC}"
    echo -e "${RED}警告: 端口 $PORT 已被占用，服务可能启动失败。${NC}"
else
    # Start service
    systemctl start xray-lite
    if systemctl is-active --quiet xray-lite; then
        echo -e "${GREEN}✓ Service started successfully / 服务启动成功${NC}"
    else
        echo -e "${RED}Service failed to start. Check logs with: journalctl -u xray-lite -f${NC}"
        echo -e "${RED}服务启动失败。请使用 journalctl -u xray-lite -f 查看日志${NC}"
    fi
fi

echo ""
echo -e "${BLUE}=========================================${NC}"
echo -e "${GREEN}Installation Completed / 安装完成${NC}"
echo -e "${BLUE}=========================================${NC}"
echo ""
echo -e "Server IP    : ${YELLOW}$SERVER_IP${NC}"
echo -e "Port         : ${YELLOW}$PORT${NC}"
echo -e "UUID         : ${YELLOW}$CLIENT_UUID${NC}"
echo -e "Flow         : ${YELLOW}xtls-rprx-vision${NC}"
echo -e "Encryption   : ${YELLOW}none${NC}"
echo -e "Network      : ${YELLOW}tcp${NC}"
echo -e "Security     : ${YELLOW}reality${NC}"
echo -e "Public Key   : ${YELLOW}$PUBLIC_KEY${NC}"
echo -e "Short ID     : ${YELLOW}$SHORT_ID${NC}"
echo -e "Sni          : ${YELLOW}$DOMAIN${NC}"
if [ -n "$XHTTP_PATH" ]; then
    echo -e "XHTTP Path   : ${YELLOW}$XHTTP_PATH${NC}"
    echo -e "XHTTP Mode   : ${YELLOW}auto${NC}"
    
    # Generate VLESS Link with XHTTP
    VLESS_LINK="vless://${CLIENT_UUID}@${SERVER_IP}:${PORT}?security=reality&encryption=none&pbk=${PUBLIC_KEY}&fp=chrome&type=tcp&sni=${DOMAIN}&sid=${SHORT_ID}&spx=${XHTTP_PATH}#Xray-Lite-XHTTP"
else
    # Standard VLESS Link
    VLESS_LINK="vless://${CLIENT_UUID}@${SERVER_IP}:${PORT}?security=reality&encryption=none&pbk=${PUBLIC_KEY}&fp=chrome&type=tcp&sni=${DOMAIN}&sid=${SHORT_ID}#Xray-Lite-Reality"
fi

echo ""
echo -e "${BLUE}--- VLESS Link / VLESS 链接 ---${NC}"
echo -e "${GREEN}$VLESS_LINK${NC}"
echo -e "${BLUE}-------------------------------${NC}"
echo ""
echo -e "Config file: $INSTALL_DIR/config.json"
echo -e "Service: systemctl status xray-lite"
echo ""
