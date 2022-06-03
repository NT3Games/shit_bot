# Shit Bot on Telegram - 屎官

## 简介

本 Telegram Bot 可以帮助记载指定群组的“屎书”，即将含“屎”及相关字符的对话转发到指定的聊天（频道、群组）中。

## 功能

- 自定义“屎书”规则
- 自定义自动转发用户

  Bot 可以自动转发指定用户的符合规则的对话，对其他用户的对话仍可以通过命令手动转发。
- 在群组中查看最后一条“屎”
- 更多命令

## 命令

Bot 支持如下群组内命令：

- `/help`

  发送帮助文字。

  **用法**：`/help`
- `/pull`

  “拉”出最后的屎。

  **用法**：`/pull`
- `/shit`

  转发到屎书。

  **用法**：对任意消息回复 `/shit`
- `/source`

  查看源代码。

  **用法**：`/source`

## 使用

### 快速使用

以 Linux 为例。克隆本仓库：

```bash
git clone https://github.com/NT3Games/shit_bot.git
cd shit_bot
```

确保已安装 Rust 编译器和 Cargo。安装教程见 Rust [官方文档](https://doc.rust-lang.org/book/ch01-01-installation.html)。

复制 `config.toml.example` 为 `config.toml`，并根据需要修改自定义转发用户、群组和“屎书”所在聊天的 Telegram ID。

再在仓库根目录下，运行如下命令：

```bash
cargo run --release
```

## 许可证

[AGPL-3.0](/LICENSE)
