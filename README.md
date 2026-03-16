# ClawX

Rust 实现的 AI Agent 框架，支持多 LLM 后端、工具调用、对话记忆和多渠道部署。

## 特性

- **多 LLM 后端** — Anthropic / OpenAI / OpenRouter / Ollama，支持自动重试 + 熔断器 + Failover
- **工具系统** — 内置 Shell、文件读写、目录列表等工具，可扩展注册
- **对话记忆** — SQLite + FTS5 全文检索，持久化存储对话历史
- **上下文压缩** — 三级策略自动管理上下文窗口
- **多渠道** — Telegram Bot（long polling）、交互式 REPL、单次执行
- **安全** — API Key 内存清零（zeroize）、凭据泄露检测与脱敏、Shell 沙箱

## 架构

```
clawx-cli          CLI 入口（REPL / 单次 / Telegram）
├── clawx-agent    Agent 循环、上下文压缩、子代理
├── clawx-llm      LLM Provider 抽象 + Retry/CircuitBreaker 装饰器
├── clawx-tools    工具注册表 + 内置工具 + 安全策略
├── clawx-memory   SQLite 持久化记忆
├── clawx-channels 消息渠道（Telegram）
└── clawx-core     核心类型（Message、ToolCall、Config、Error）
```

## 快速开始

### 环境要求

- Rust 1.85+
- 至少一个 LLM API Key（或本地 Ollama）

### 编译

```bash
cd clawx
cargo build --release
```

### 运行

```bash
# 交互式 REPL
export ANTHROPIC_API_KEY=sk-ant-...
./target/release/clawx

# 单次执行
./target/release/clawx run "列出当前目录的文件"

# Telegram Bot
export TELEGRAM_BOT_TOKEN=123456:ABC...
./target/release/clawx telegram --allowed-users "你的UserID"

# 使用 OpenRouter
./target/release/clawx --provider openrouter --model anthropic/claude-sonnet-4-20250514

# 使用本地 Ollama
./target/release/clawx --provider ollama --model llama3.1
```

### 主要参数

| 参数 | 默认值 | 说明 |
|------|--------|------|
| `--provider` | `anthropic` | LLM 后端 |
| `--model` | `claude-sonnet-4-20250514` | 模型名称 |
| `--api-key` | 环境变量 | API Key |
| `--max-iterations` | `50` | Agent 最大迭代次数 |
| `--system` / `-s` | 内置提示 | 自定义系统提示词 |
| `--memory-db` | `clawx_memory.db` | 记忆数据库路径 |

## 部署

详见 [docs/deploy-telegram.md](docs/deploy-telegram.md)，包含 systemd、Docker、Docker Compose 部署方案。

## License

MIT OR Apache-2.0
