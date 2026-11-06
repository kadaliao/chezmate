# Chezmate

Chezmate 是 [chezmoi](https://www.chezmoi.io/) 的桌面伴侣：

- 像系统设置一样容易用：看到哪些配置文件需要更新，一键应用或保留本地修改；
- 像现代 Git 客户端一样展示变化：每个文件的差异一目了然；
- 像标准开发工具一样能被 AI Agent 稳定操作：CLI 输出固定结构的 JSON，并提供 MCP 服务。

Chezmate 不重新实现 chezmoi。所有操作都调用你机器上真实的 `chezmoi`，读到的状态就是 chezmoi 报告的状态。

## 组成

| Crate | 作用 |
| --- | --- |
| `chezmate-core` | 调用 chezmoi 并解析输出的类型化接口，不依赖 GUI |
| `chezmate-cli` | `chezmate` 命令行，所有命令支持 `--json` |
| `chezmate-app` | 基于 [GPUI Kit](https://gpui-kit.com) 的桌面应用 |

## 环境要求

- Rust 1.88+（开发时使用 1.99）
- chezmoi 2.x（开发时使用 2.73.3），需在 `PATH` 或常见安装目录中（`/opt/homebrew/bin`、`/usr/local/bin`、`~/.local/bin` 等），也可用 `CHEZMATE_CHEZMOI` 或 `--chezmoi` 指定
- 桌面应用目前在 macOS 上开发和验证；GPUI 同样支持 Linux 和 Windows，但尚未验证

## 构建与运行

```sh
cargo build                     # 构建全部
cargo run -p chezmate-app       # 桌面应用
cargo run -p chezmate-cli -- status
cargo test                      # 测试（需要已安装 chezmoi）
```

首次构建 GPUI 需要几分钟。

## 桌面应用

- **Changes**：哪些文件和 dotfiles 不一致，每个文件会怎么变（差异视图）。`Apply` 用 dotfiles 的版本更新文件；文件在 chezmoi 之外被改过时，`Keep Local` 把你的修改存回 dotfiles。会丢失修改、运行脚本或删除文件的操作都会先确认。
- **Files**：所有托管文件、它们的源文件和属性（模板、加密、私有……），添加新文件或停止托管。
- **Repository**：dotfiles 源目录的 Git 仓库。提交、推送、拉取；拉取只更新 dotfiles，变化会出现在 Changes 里等你确认后再应用。
- **Environment**：使用的 chezmoi、源目录、配置文件，按需运行诊断。

## 命令行

```sh
chezmate info                   # chezmoi 版本和目录
chezmate status                 # 与源状态不一致的文件
chezmate managed                # 所有托管文件
chezmate diff [目标…]            # apply 会带来的变化（Git 格式）
chezmate apply [目标…] [-n] [--force]
chezmate add <路径…> [--template] [--encrypt]
chezmate re-add [目标…]          # 把本地修改写回源状态（默认：所有本地改过的文件）
chezmate forget <目标…> --yes    # 停止托管（文件本身保留）
chezmate cat <目标>
chezmate source-path <目标>
chezmate doctor
chezmate git status|log|diff|fetch|pull|push
chezmate git commit -m "说明"     # 提交源目录的全部变更
```

相对路径一律相对于目标目录（通常是 home），和 `chezmate status` 打印的路径一致，与当前工作目录无关。也可以使用绝对路径或 `~/…`。

`--json` 输出单个 JSON 对象：

```json
{"ok": true, "data": …}
{"ok": false, "error": {"kind": "conflict", "message": "…", "paths": [".zshrc"]}}
```

错误类型 `kind`：`chezmoi_not_found`、`git_not_found`、`spawn_failed`、`timeout`、`conflict`、`command_failed`、`parse_error`、`invalid_request`、`io_error`。失败时退出码为 1。

### 状态的两列含义

`status` 的每一项有两个字段：

- `local`：磁盘上的文件和 chezmoi 上次写入时相比的变化。非 `none` 表示文件在 chezmoi 之外被改过，直接 apply 会覆盖这些修改；
- `apply`：执行 apply 会做什么（`added` 新建、`modified` 更新、`deleted` 删除、`run` 运行脚本）。

本地改过的文件直接 apply 会返回 `conflict` 错误。可以用 `re-add` 保留本地版本，或用 `apply --force` 覆盖。

## 已知限制

- `re-add` 无法处理模板文件（chezmoi 不能从结果反推模板），Chezmate 会明确列出被跳过的模板，而不是静默忽略；
- `status`、`diff`、`cat` 会渲染模板、解密加密文件，可能调用 gpg/age 或密码管理器；
- `chezmoi doctor` 会对每个已知工具执行 `--version`，某些工具（如 1Password 的 `op`）可能一直等待桌面端授权，因此 doctor 有 30 秒超时；
- 实施进度见 [docs/STATUS.md](docs/STATUS.md)。

## 许可

MIT OR Apache-2.0
