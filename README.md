# Chezmate

Chezmate 是 [chezmoi](https://www.chezmoi.io/) 的桌面伴侣，让几台电脑上的配置文件保持一致：

- 像系统设置一样容易用：每个文件只有一种状态，一个「同步」按钮；
- 像现代 Git 客户端一样展示变化：每个文件的差异一目了然；
- 像标准开发工具一样能被 AI Agent 稳定操作：CLI 输出固定结构的 JSON，并提供 MCP 服务。

Chezmate 不重新实现 chezmoi。所有操作都调用你机器上真实的 `chezmoi` 和 `git`，读到的状态就是它们报告的状态。

## 简化模型

用户只需要理解两样东西：**这台电脑**，和保存在 Git 远端的**共享版本**（chezmoi 的源目录）。每个不一致的文件只有一种状态：

| 状态 | 含义 | 同步时 |
| --- | --- | --- |
| 这台电脑上改过 | 本机改了，共享版本没变 | 存到共享版本（chezmoi `re-add`） |
| 有新版本 | 共享版本变了（通常来自另一台电脑） | 更新这台电脑（`apply`） |
| 两边都改了 | 两边都改过；或这台电脑上原本就有一个不同的版本 | 不动，等你选择保留哪边 |
| 这台电脑上删除了 | 本机删掉了这个文件 | 不动，等你选择恢复或不再同步 |

由模板生成的文件在本机改过时不能自动存回（每台电脑的内容可能不同），也归入「需要你决定」。`run_once_` / `run_onchange_` 脚本作为「设置步骤」单独列出。

「同步」分两步：先获取共享版本（fetch + pull，只改源目录，不碰家目录），再列出计划等你确认——要保存的文件可以逐个取消勾选。确认后依次保存、更新、运行设置步骤，然后提交并推送。连不上远端时仍然可以完成本机这部分，下次同步再上传。

区分「这台电脑上改过」和「两边都改了」：`chezmoi status` 对两者都显示 `MM`。Chezmate 比较 chezmoi 上次写入时记录的哈希（`chezmoi state dump`）和共享版本现在渲染出的内容（`chezmoi cat`）的哈希；相同说明只有本机改过。chezmoi 从没写过、但本机已经存在的文件（新电脑上常见），`apply` 会不提示直接覆盖，所以算作「两边都改了」。

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

## 安装

从 [Releases](https://github.com/kadaliao/chezmate/releases) 下载，目前只有 Apple Silicon 的 macOS 版：

- `Chezmate-<版本>-macos-arm64.zip`：桌面应用，解压后拖进「应用程序」；
- `chezmate-<版本>-macos-arm64.tar.gz`：命令行（含 MCP 服务器），解压后把 `chezmate` 放进 `PATH`。

应用和命令行都用 Developer ID 签名并经过 Apple 公证，下载后可以直接打开。

自己打包：`scripts/package-macos.sh`，产物在 `dist/`。钥匙串里有「Developer ID Application」证书时自动用它签名（也可以用 `CHEZMATE_SIGN_IDENTITY` 指定），否则临时签名；设置 `CHEZMATE_NOTARY_PROFILE=<notarytool 凭据名>`（用 `xcrun notarytool store-credentials` 存一次）时同时提交公证。

## 构建与运行

```sh
cargo build                     # 构建全部
cargo run -p chezmate-app       # 桌面应用
cargo run -p chezmate-cli -- status
cargo test                      # 测试（需要已安装 chezmoi）
```

首次构建 GPUI 需要几分钟。

## 桌面应用

界面跟随系统语言显示中文或英文（`CHEZMATE_LANG=zh` / `en` 可以指定）。

- **概览**（首页）：按「需要你决定 / 这台电脑上改过 / 有新版本 / 设置步骤」分组列出文件；右侧用一句话说明状态，给出合适的选择，并显示差异（说明红绿各代表哪边）。顶部是「同步」按钮。

「高级（chezmoi）」里是直接使用 chezmoi 概念的页面，给熟悉 chezmoi 的人：

- **Changes**：哪些文件和 dotfiles 不一致，每个文件会怎么变（差异视图）。`Apply` 用 dotfiles 的版本更新文件；文件在 chezmoi 之外被改过时，`Keep Local` 把你的修改存回 dotfiles。会丢失修改、运行脚本或删除文件的操作都会先确认。
- **文件**：所有托管文件按它们在家目录里的位置排成目录树，有变化的文件带颜色圆点（红：需要你决定，黄：这台电脑上改过，蓝：有新版本）。点一下文件，右侧上方是基本信息（状态、路径、源文件、大小、修改时间、权限、模板 / 加密等属性），下方直接显示内容：「差异 / 这台电脑 / 源文件」三个标签，带行号和语法高亮，图片直接预览，二进制 plist 转成 XML 显示；加密文件要点「显示内容」才显示解密后的内容。默认只读，点「编辑」才能修改并保存（模板生成的文件只能改源文件）。也可以添加新文件或停止托管。
- **Repository**：dotfiles 源目录的 Git 仓库。提交、推送、拉取；拉取只更新 dotfiles，变化会出现在 Changes 里等你确认后再应用。
- **Environment**：使用的 chezmoi、源目录、配置文件，按需运行诊断。

## 命令行

```sh
chezmate overview               # 每个文件的状态、设置步骤、共享版本的情况
chezmate sync --dry-run         # 获取远端并列出同步计划，不做任何修改
chezmate sync [--skip 目标]…     # 拉取、保存、更新、运行设置步骤、提交并推送
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

## MCP（给 AI Agent）

`chezmate mcp` 通过 stdio 提供 [Model Context Protocol](https://modelcontextprotocol.io) 服务，同时支持 2026-07-28 的无状态协议和 2025-11-25 及更早的 `initialize` 握手协议。

```sh
cargo install --path crates/chezmate-cli          # 安装 chezmate 到 ~/.cargo/bin
claude mcp add chezmate -- chezmate mcp           # Claude Code
claude mcp add chezmate -- chezmate mcp --read-only
```

其他客户端：

```json
{ "mcpServers": { "chezmate": { "command": "chezmate", "args": ["mcp"] } } }
```

工具：`chezmoi_info`、`chezmoi_status`、`chezmoi_managed`、`chezmoi_diff`、`chezmoi_apply`、`chezmoi_add`、`chezmoi_re_add`、`chezmoi_forget`、`git_status`、`git_log`、`git_diff`、`git_commit`、`git_fetch`、`git_pull`、`git_push`、`sync_overview`、`sync`。

- 最简单的用法：`sync_overview` 看每个文件的状态，`sync` 先 dry run 看计划，再传 `"dry_run": false` 执行。`sync` 从不覆盖本机改过的文件；需要决定的文件用 `chezmoi_re_add`（保留本机）或 `chezmoi_apply` + `force`（用共享版本）处理；
- `chezmoi_apply` 默认是 dry run，必须传 `"dry_run": false` 才会写入；
- `chezmoi_diff` 默认不包含加密文件（`include_encrypted: true` 才包含）。模板渲染结果仍可能含有密码管理器提供的内容；
- `--read-only` 只暴露只读工具；
- 工具失败返回 `isError: true`，`structuredContent` 里是和 CLI 相同的 `{kind, message, …}`；
- 每个工具都带 `readOnlyHint` / `destructiveHint` 等标注。

### 状态的两列含义

`status` 的每一项有两个字段：

- `local`：磁盘上的文件和 chezmoi 上次写入时相比的变化。非 `none` 表示文件在 chezmoi 之外被改过，直接 apply 会覆盖这些修改；
- `apply`：执行 apply 会做什么（`added` 新建、`modified` 更新、`deleted` 删除、`run` 运行脚本）。

本地改过的文件直接 apply 会返回 `conflict` 错误。可以用 `re-add` 保留本地版本，或用 `apply --force` 覆盖。

## 已知限制

- 指定目标 `chezmoi apply` 时，chezmoi 按名字顺序运行脚本，不管 `before_` / `after_`；同步因此分三轮 apply；
- 同步只在更新这台电脑（或运行设置步骤）时运行「每次 apply 都运行」的脚本，和 `chezmoi update` 有更新时的行为一致；
- 两台电脑改了共享版本里的同一个文件、且这台电脑的改动还没上传时，拉取会被撤销并报错，需要在「高级 › Repository」或终端里处理；
- 高级页面里 Changes、Repository、Environment 的界面文字只有英文；
- `re-add` 无法处理模板文件（chezmoi 不能从结果反推模板），Chezmate 会明确列出被跳过的模板，而不是静默忽略；
- `status`、`diff`、`cat` 会渲染模板、解密加密文件，可能调用 gpg/age 或密码管理器；
- `chezmoi doctor` 会对每个已知工具执行 `--version`，某些工具（如 1Password 的 `op`）可能一直等待桌面端授权，因此 doctor 有 30 秒超时；
- 实施进度见 [docs/STATUS.md](docs/STATUS.md)。

## 许可

双许可：[MIT](LICENSE-MIT) 或 [Apache-2.0](LICENSE-APACHE)，任选其一。
