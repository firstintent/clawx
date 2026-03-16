# ClawX AI Agent — Telegram 部署与使用指南

## 目录

- [架构概览](#架构概览)
- [环境要求](#环境要求)
- [第一步：创建 Telegram Bot](#第一步创建-telegram-bot)
- [第二步：获取 LLM API Key](#第二步获取-llm-api-key)
- [第三步：编译项目](#第三步编译项目)
- [第四步：配置与启动](#第四步配置与启动)
- [运行模式](#运行模式)
- [配置参考](#配置参考)
- [生产部署](#生产部署)
- [安全建议](#安全建议)
- [故障排查](#故障排查)

---

## 架构概览

```
Telegram 用户
    │
    ▼
Telegram Bot API  ◄── Long Polling (30s 超时)
    │
    ▼
┌─────────────────────────────────┐
│  ClawX CLI (telegram 子命令)      │
│  ┌───────────────────────────┐  │
│  │ TelegramChannel           │  │
│  │  · 消息接收 & 用户过滤     │  │
│  │  · Markdown → HTML 转换   │  │
│  │  · 4096 字符智能分块       │  │
│  └────────────┬──────────────┘  │
│               ▼                  │
│  ┌───────────────────────────┐  │
│  │ Agent Loop                │  │
│  │  · LoopDelegate 控制流    │  │
│  │  · 工具调用 & 执行         │  │
│  │  · 上下文压缩 (3 级策略)   │  │
│  └────────────┬──────────────┘  │
│               ▼                  │
│  ┌───────────────────────────┐  │
│  │ LLM Provider 链            │  │
│  │  Raw → Retry → CircuitBreaker │
│  └───────────────────────────┘  │
│               ▼                  │
│  ┌───────────────────────────┐  │
│  │ Memory (SQLite + FTS5)    │  │
│  └───────────────────────────┘  │
└─────────────────────────────────┘
```

**核心特性：**
- 基于 long polling 的 Telegram 消息接收，支持指数退避重连
- 每个 chat 独立的会话历史（多用户/多群组并发）
- API Key 使用 `Zeroizing<String>` 安全存储，内存释放后自动清零
- 凭据泄露检测：工具输出中的 API key/token/password 自动脱敏
- 支持 Anthropic / OpenAI / OpenRouter / Ollama 等多 LLM 后端

---

## 环境要求

| 依赖 | 版本要求 | 说明 |
|------|---------|------|
| Rust | 1.85+ | `rustup update stable` |
| 系统 | Linux / macOS / WSL2 | Windows 需通过 WSL |
| 网络 | 可访问 Telegram API | 中国大陆需代理 |
| LLM API | 至少一个 API Key | 见下文 |

---

## 第一步：创建 Telegram Bot

1. 打开 Telegram，搜索 **@BotFather**
2. 发送 `/newbot`
3. 按提示输入 bot 名称和用户名
4. BotFather 会返回一个 **Bot Token**，格式类似：
   ```
   123456789:ABCDefGHIjklMNOpqrsTUVwxyz
   ```
5. **保存这个 Token**，后续配置需要

**可选设置（推荐）：**
```
/setdescription    — 设置 bot 描述
/setcommands       — 设置命令菜单（如 /start, /help）
/setprivacy        — 群组中设为 Disable 可接收所有消息
```

**获取你的 Telegram User ID：**
- 搜索 **@userinfobot**，发送任意消息即可获取你的数字 ID
- 用于 `--allowed-users` 安全限制

---

## 第二步：获取 LLM API Key

根据你选择的 LLM 提供商：

### Anthropic (推荐)
1. 访问 https://console.anthropic.com/
2. 创建 API Key
3. 设置环境变量：`export ANTHROPIC_API_KEY=sk-ant-...`

### OpenAI
1. 访问 https://platform.openai.com/api-keys
2. 创建 API Key
3. 设置环境变量：`export OPENAI_API_KEY=sk-...`

### OpenRouter (多模型网关)
1. 访问 https://openrouter.ai/keys
2. 创建 API Key
3. 设置环境变量：`export OPENROUTER_API_KEY=sk-or-...`

### Ollama (本地模型，免费)
1. 安装 Ollama: `curl -fsSL https://ollama.ai/install.sh | sh`
2. 拉取模型: `ollama pull llama3.1`
3. 无需 API Key

---

## 第三步：编译项目

```bash
cd /path/to/clawx

# 编译 release 版本（推荐，体积更小、性能更好）
cargo build --release

# 二进制产物位置
ls -lh target/release/clawx
```

编译完成后可将二进制拷贝到目标服务器：
```bash
# 单文件部署，无外部依赖
scp target/release/clawx user@server:/usr/local/bin/
```

---

## 第四步：配置与启动

### 方式一：环境变量（推荐生产使用）

创建 `.env` 文件：
```bash
# .env
ANTHROPIC_API_KEY=sk-ant-api03-xxxxx
TELEGRAM_BOT_TOKEN=123456789:ABCDefGHIjklMNOpqrsTUVwxyz
RUST_LOG=clawx=info
```

启动：
```bash
# 加载环境变量并启动
source .env && ./target/release/clawx telegram \
    --allowed-users "你的UserID" \
    --mention-only true
```

### 方式二：命令行参数

```bash
./target/release/clawx telegram \
    --bot-token "123456789:ABCDefGHIjklMNOpqrsTUVwxyz" \
    --allowed-users "12345678,87654321" \
    --mention-only true \
    --provider anthropic \
    --model claude-sonnet-4-20250514
```

### 方式三：使用 Ollama 本地模型（无需 API Key）

```bash
# 先确保 Ollama 运行中
ollama serve &

./target/release/clawx telegram \
    --bot-token "$TELEGRAM_BOT_TOKEN" \
    --provider ollama \
    --model llama3.1 \
    --allowed-users "你的UserID"
```

### 方式四：使用 OpenRouter 访问多种模型

```bash
./target/release/clawx telegram \
    --bot-token "$TELEGRAM_BOT_TOKEN" \
    --provider openrouter \
    --model anthropic/claude-sonnet-4-20250514 \
    --allowed-users "你的UserID"
```

---

## 运行模式

Claw 提供三种运行模式：

### 1. Telegram Bot 模式（本文重点）
```bash
clawx telegram [选项]
```

### 2. 交互式 REPL 模式
```bash
clawx
# 进入交互式对话
# you> 你好
# clawx> 你好！有什么可以帮你的？
```

### 3. 单次执行模式
```bash
clawx run "列出当前目录的文件"
```

---

## 配置参考

### Telegram 相关参数

| 参数 | 环境变量 | 默认值 | 说明 |
|------|---------|--------|------|
| `--bot-token` | `TELEGRAM_BOT_TOKEN` | (必填) | Telegram Bot Token |
| `--allowed-users` | — | `""` (允许所有) | 允许的用户 ID，逗号分隔 |
| `--mention-only` | — | `true` | 群组中仅响应 @提及 |
| `--telegram-api-base` | — | `api.telegram.org` | 自定义 API 地址 |
| `--draft-interval-ms` | — | `750` | 流式编辑最小间隔 (ms) |

### LLM 相关参数

| 参数 | 环境变量 | 默认值 | 说明 |
|------|---------|--------|------|
| `--provider` | — | `anthropic` | LLM 提供商 |
| `--model` | — | `claude-sonnet-4-20250514` | 模型名称 |
| `--api-key` | `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` | — | API Key |
| `--base-url` | — | (按提供商) | API 基础 URL |
| `--max-iterations` | — | `50` | Agent 循环最大迭代数 |

### 通用参数

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `--system` / `-s` | 内置提示 | 自定义系统提示词 |
| `--memory-db` | `clawx_memory.db` | SQLite 记忆数据库路径 |

### 提供商与模型速查

| 提供商 | `--provider` | `--model` 示例 | 环境变量 |
|--------|-------------|----------------|---------|
| Anthropic | `anthropic` | `claude-sonnet-4-20250514` | `ANTHROPIC_API_KEY` |
| OpenAI | `openai` | `gpt-4o` | `OPENAI_API_KEY` |
| OpenRouter | `openrouter` | `anthropic/claude-sonnet-4-20250514` | `OPENROUTER_API_KEY` |
| Ollama | `ollama` | `llama3.1`, `qwen2.5` | (无需) |

---

## 生产部署

### 使用 systemd (Linux)

创建服务文件 `/etc/systemd/system/clawx-telegram.service`：

```ini
[Unit]
Description=ClawX AI Telegram Bot
After=network.target

[Service]
Type=simple
User=clawx
Group=clawx
WorkingDirectory=/opt/clawx
ExecStart=/opt/clawx/clawx telegram --allowed-users "你的UserID"
Restart=always
RestartSec=5

# 环境变量
EnvironmentFile=/opt/clawx/.env

# 安全加固
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/opt/clawx
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

部署步骤：
```bash
# 1. 创建系统用户
sudo useradd -r -s /bin/false -d /opt/clawx clawx
sudo mkdir -p /opt/clawx
sudo cp target/release/clawx /opt/clawx/

# 2. 创建环境变量文件
sudo tee /opt/clawx/.env << 'EOF'
ANTHROPIC_API_KEY=sk-ant-api03-xxxxx
TELEGRAM_BOT_TOKEN=123456789:ABCDefGHIjklMNOpqrsTUVwxyz
RUST_LOG=clawx=info
EOF
sudo chmod 600 /opt/clawx/.env
sudo chown -R clawx:clawx /opt/clawx

# 3. 启动服务
sudo systemctl daemon-reload
sudo systemctl enable clawx-telegram
sudo systemctl start clawx-telegram

# 4. 查看日志
sudo journalctl -u clawx-telegram -f
```

### 使用 Docker

创建 `Dockerfile`：

```dockerfile
FROM rust:1.85 AS builder
WORKDIR /build
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/clawx /usr/local/bin/clawx
RUN useradd -r -s /bin/false clawx
USER clawx
WORKDIR /data
ENTRYPOINT ["clawx"]
```

运行：
```bash
# 构建镜像
docker build -t clawx .

# 运行
docker run -d \
    --name clawx-telegram \
    --restart unless-stopped \
    -e ANTHROPIC_API_KEY=sk-ant-api03-xxxxx \
    -e TELEGRAM_BOT_TOKEN=123456789:ABCDefGHIjklMNOpqrsTUVwxyz \
    -e RUST_LOG=clawx=info \
    -v clawx-data:/data \
    clawx telegram --allowed-users "你的UserID"

# 查看日志
docker logs -f clawx-telegram
```

### 使用 Docker Compose

```yaml
# docker-compose.yml
services:
  clawx-telegram:
    build: .
    restart: unless-stopped
    command: telegram --allowed-users "${ALLOWED_USERS}"
    environment:
      - ANTHROPIC_API_KEY=${ANTHROPIC_API_KEY}
      - TELEGRAM_BOT_TOKEN=${TELEGRAM_BOT_TOKEN}
      - RUST_LOG=clawx=info
    volumes:
      - clawx-data:/data

volumes:
  clawx-data:
```

```bash
# .env (Docker Compose 自动加载)
ANTHROPIC_API_KEY=sk-ant-api03-xxxxx
TELEGRAM_BOT_TOKEN=123456789:ABCDefGHIjklMNOpqrsTUVwxyz
ALLOWED_USERS=12345678

# 启动
docker compose up -d
```

---

## 安全建议

### 必须做

1. **设置 `--allowed-users`**：限制只有授权用户能与 bot 交互。不设置则任何人都可以使用
2. **不要在命令行传 API Key**：使用环境变量或 `.env` 文件，避免进程列表泄露
3. **.env 文件权限设为 600**：`chmod 600 .env`

### 建议做

4. **使用非 root 用户运行**：创建专用 `clawx` 用户
5. **启用 systemd 安全选项**：`ProtectSystem=strict`、`NoNewPrivileges=true`
6. **定期轮换 API Key 和 Bot Token**
7. **限制 Shell 工具使用**：生产环境谨慎启用 `shell` 工具，它允许执行系统命令

### 内置安全机制

- **凭据自动脱敏**：工具输出中检测到的 API key、token、password 会被自动替换为 `[REDACTED]`
- **API Key 内存安全**：LLM API key 使用 `Zeroizing<String>` 存储，释放后内存自动清零
- **Shell 工具沙箱**：Shell 执行前清空环境变量（仅保留 PATH/HOME），超时 120 秒自动终止
- **LLM 弹性**：3 次自动重试 + 熔断器（5 次失败断开，30 秒后半开恢复）

### 中国大陆用户

如果无法直接访问 Telegram API，需配置代理：

```bash
# HTTP 代理
export HTTPS_PROXY=http://127.0.0.1:7890

# 或使用自建 Telegram Bot API 服务器
# https://github.com/tdlib/telegram-bot-api
./target/release/clawx telegram \
    --telegram-api-base "http://localhost:8081" \
    --bot-token "$TELEGRAM_BOT_TOKEN"
```

---

## 故障排查

### Bot 无响应

```bash
# 1. 检查日志
RUST_LOG=clawx=debug ./target/release/clawx telegram --bot-token "..."

# 2. 验证 Bot Token 是否有效
curl "https://api.telegram.org/bot<你的TOKEN>/getMe"

# 3. 确认网络连通性
curl -I "https://api.telegram.org"

# 4. 检查是否有其他实例占用 (Telegram 不允许多个 polling 实例)
# 如果看到 409 Conflict 错误，停止其他运行中的实例
```

### LLM 调用失败

```bash
# 检查 API Key 是否正确
# Anthropic
curl https://api.anthropic.com/v1/messages \
    -H "x-api-key: $ANTHROPIC_API_KEY" \
    -H "anthropic-version: 2023-06-01" \
    -H "content-type: application/json" \
    -d '{"model":"claude-sonnet-4-20250514","max_tokens":10,"messages":[{"role":"user","content":"hi"}]}'

# 查看详细错误日志
RUST_LOG=clawx=debug,reqwest=debug ./target/release/clawx telegram ...
```

### 常见错误

| 错误 | 原因 | 解决方案 |
|------|------|---------|
| `telegram api getUpdates: 409` | 另一个实例在轮询 | 停止其他 bot 实例 |
| `telegram api getUpdates: 401` | Bot Token 无效 | 检查 Token 或联系 @BotFather 重新生成 |
| `circuit breaker open` | LLM 连续 5 次失败 | 检查 API Key 和网络，等待 30 秒自动恢复 |
| `rate limited, retry after Xs` | API 调用频率过高 | 自动重试，无需干预 |
| `max iterations reached` | Agent 循环超过 50 次 | 简化任务或增加 `--max-iterations` |
| `context window exceeded` | 对话过长 | 自动触发上下文压缩，或开始新对话 |

### 日志级别

```bash
# 仅关键信息
RUST_LOG=clawx=info

# 调试模式（含 LLM 调用详情）
RUST_LOG=clawx=debug

# 全量日志（含 HTTP 请求/响应）
RUST_LOG=clawx=trace,reqwest=debug
```

---

## 可用工具

Telegram Bot 默认注册以下工具，LLM 会根据用户请求自动选择使用：

| 工具 | 说明 | 需要审批 |
|------|------|---------|
| `echo` | 回显消息（测试用） | 否 |
| `shell` | 执行 Shell 命令 | 是* |
| `read_file` | 读取文件内容 | 否 |
| `write_file` | 写入文件 | 是* |
| `list_dir` | 列出目录 | 否 |

> *标注"是"的工具在 Telegram 模式下自动执行（无交互审批界面）。**生产环境请评估安全风险。**

---

## 使用示例

在 Telegram 中与 bot 对话：

```
你: 你好，你能做什么？

Claw: 你好！我是 ClawX AI 助手。我可以：
• 执行 Shell 命令
• 读写文件
• 列出目录内容
• 回答各种问题
有什么需要帮忙的？

你: 查看服务器的磁盘使用情况

Claw: [调用 shell 工具: df -h]
文件系统      容量  已用  可用  使用%  挂载点
/dev/sda1     50G   32G   16G    67%  /
tmpfs         8G    0     8G     0%   /dev/shm
...

你: /opt 下有哪些文件？

Claw: [调用 list_dir 工具: /opt]
clawx/
nginx/
...
```
