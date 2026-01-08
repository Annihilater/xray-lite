#!/bin/bash

# Xray-Lite GitHub 上传脚本
# 请按照提示操作

set -e

GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

echo -e "${BLUE}========================================${NC}"
echo -e "${BLUE}  Xray-Lite GitHub 上传助手${NC}"
echo -e "${BLUE}========================================${NC}"
echo ""

# 检查是否在正确的目录
if [ ! -f "Cargo.toml" ]; then
    echo -e "${YELLOW}错误: 请在项目根目录运行此脚本${NC}"
    exit 1
fi

echo -e "${YELLOW}步骤 1/4: 检查 Git 状态${NC}"
git status
echo ""

echo -e "${YELLOW}步骤 2/4: 配置 Git 用户信息 (如果需要)${NC}"
echo "当前 Git 用户配置:"
git config user.name || echo "  未设置"
git config user.email || echo "  未设置"
echo ""

read -p "是否需要设置 Git 用户信息? (y/N): " -n 1 -r
echo
if [[ $REPLY =~ ^[Yy]$ ]]; then
    read -p "请输入你的 GitHub 用户名: " GIT_NAME
    read -p "请输入你的 GitHub 邮箱: " GIT_EMAIL
    git config user.name "$GIT_NAME"
    git config user.email "$GIT_EMAIL"
    echo -e "${GREEN}✓ Git 用户信息已设置${NC}"
fi
echo ""

echo -e "${YELLOW}步骤 3/4: 准备上传${NC}"
echo "请先在 GitHub 上创建新仓库:"
echo "  1. 访问: https://github.com/new"
echo "  2. 仓库名: xray-lite"
echo "  3. 描述: Lightweight Xray implementation in Rust with Reality and XHTTP support"
echo "  4. 选择 Public"
echo "  5. 不要勾选任何初始化选项"
echo "  6. 点击 'Create repository'"
echo ""

read -p "已经创建好仓库了吗? (y/N): " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Yy]$ ]]; then
    echo -e "${YELLOW}请先创建仓库，然后重新运行此脚本${NC}"
    exit 0
fi

echo ""
echo -e "${YELLOW}步骤 4/4: 上传到 GitHub${NC}"
read -p "请输入你的 GitHub 用户名: " GITHUB_USER

echo ""
echo "选择上传方式:"
echo "  1. HTTPS (简单，但每次需要输入密码/token)"
echo "  2. SSH (推荐，需要先配置 SSH key)"
read -p "请选择 (1/2): " -n 1 -r
echo

if [[ $REPLY == "1" ]]; then
    REMOTE_URL="https://github.com/$GITHUB_USER/xray-lite.git"
else
    REMOTE_URL="git@github.com:$GITHUB_USER/xray-lite.git"
fi

echo ""
echo -e "${BLUE}执行以下命令:${NC}"
echo ""

# 添加远程仓库
echo -e "${GREEN}1. 添加远程仓库...${NC}"
if git remote | grep -q "^origin$"; then
    echo "远程仓库已存在，移除旧的..."
    git remote remove origin
fi
git remote add origin "$REMOTE_URL"
echo -e "${GREEN}✓ 远程仓库已添加: $REMOTE_URL${NC}"
echo ""

# 重命名分支
echo -e "${GREEN}2. 重命名分支为 main...${NC}"
git branch -M main
echo -e "${GREEN}✓ 分支已重命名${NC}"
echo ""

# 推送代码
echo -e "${GREEN}3. 推送代码到 GitHub...${NC}"
echo "正在推送..."
if git push -u origin main; then
    echo ""
    echo -e "${GREEN}========================================${NC}"
    echo -e "${GREEN}  ✓ 上传成功！${NC}"
    echo -e "${GREEN}========================================${NC}"
    echo ""
    echo "你的仓库地址:"
    echo "  https://github.com/$GITHUB_USER/xray-lite"
    echo ""
    echo "下一步建议:"
    echo "  1. 添加 Topics 标签: rust, xray, proxy, reality, http2"
    echo "  2. 创建 Release (v0.1.0)"
    echo "  3. 添加 GitHub Actions CI"
    echo ""
else
    echo ""
    echo -e "${YELLOW}========================================${NC}"
    echo -e "${YELLOW}  上传失败${NC}"
    echo -e "${YELLOW}========================================${NC}"
    echo ""
    echo "可能的原因:"
    echo "  1. 仓库名不是 'xray-lite'"
    echo "  2. 没有权限 (检查 SSH key 或 token)"
    echo "  3. 网络问题"
    echo ""
    echo "请检查错误信息并重试"
fi
