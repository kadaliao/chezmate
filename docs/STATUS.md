# 实施状态

最后更新：2026-11-07

## 阶段

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| 0 | Workspace、最小 GPUI Kit 窗口 | 完成 |
| 1 | Core（真实 chezmoi 操作）+ CLI（`--json`） | 完成 |
| 2 | GUI：变更列表、差异视图、应用 / 保留本地修改、托管文件、环境 | 完成（视觉效果待人工确认） |
| 3 | Git：源仓库状态、提交、拉取、推送 | 未开始 |
| 4 | MCP 服务（`chezmate mcp`，stdio） | 未开始 |

## 已实现（Core / CLI）

| 操作 | Core | CLI | 测试 |
| --- | --- | --- | --- |
| 版本与目录 | `info` / `paths` | `info` | 沙箱集成测试 |
| 状态 | `status` | `status` | 单元 + 集成 |
| 托管列表（含属性解析） | `managed` | `managed` | 单元 + 集成 |
| 差异 | `diff`（内置 diff，Git 格式） | `diff` | 集成 |
| 应用 | `apply`（dry-run / force / 冲突识别） | `apply` | 集成 |
| 添加 | `add`（template / encrypt） | `add` | 集成 |
| 保留本地修改 | `re_add`（报告被跳过的模板） | `re-add` | 集成 |
| 停止托管 | `forget` | `forget --yes` | 集成 |
| 渲染内容 / 源路径 | `cat` / `source_path` | `cat` / `source-path` | 集成 |
| 诊断 | `doctor`（30 秒超时） | `doctor` | 单元 + 集成 |

## 已实现（GUI）

| 页面 | 功能 | 测试 |
| --- | --- | --- |
| Changes | 待应用列表（创建 / 更新 / 删除 / 运行脚本，本地修改标记）、选中项的差异（统一 / 并排）、Apply、Apply All、Keep Local（re-add）；覆盖本地修改、运行脚本、删除前确认；模板不可 Keep Local 时禁用并说明原因 | 无头窗口 UI 测试 + 纯逻辑单元测试 |
| Files | 托管文件列表与过滤、来源与属性、打开源文件 / 在 Finder 中显示、Add…（系统文件选择器）、Stop Managing（确认）、跳转到 Changes 查看待应用变化 | 无头窗口 UI 测试；文件选择器本身为原生 UI，测试直接调用同一模型命令 |
| Environment | chezmoi 版本、路径、是否 Git 仓库、打开源目录 / 配置；按需运行 doctor | 无头窗口 UI 测试 |

通用：后台线程读取，刷新时保留旧数据，新请求丢弃旧结果；同一时间只运行一个写操作；结果以通知显示；⌘R 刷新，↑↓ / j k 在变更列表中移动。

GUI 测试在沙箱里驱动真实 GPUI Kit 组件和真实 chezmoi（点击、输入、确认对话框），并检查磁盘上的文件。`cargo test -p chezmate-app real_setup -- --ignored --nocapture` 对本机真实配置做只读加载检查（只打印数量）。

## 已验证的 chezmoi 行为

- 相对路径参数按进程工作目录解析，Core 一律转换为绝对路径；
- `--no-tty` 下遇到本地修改冲突时，chezmoi 输出 `… has changed since chezmoi last wrote it` 后以 EOF 失败，Core 识别为 `conflict`；
- `forget` 不加 `--force` 会提示确认并失败；
- `re-add` 静默跳过模板；
- chezmoi 拒绝添加位于其配置目录内的文件（测试沙箱据此布局）；
- `.chezmoi.os` 使用 Go 的命名（`darwin`）。

## 限制与环境问题

- 本机 `chezmoi doctor` 会卡在 1Password CLI 的 `op --version`（等待桌面端授权），Core 30 秒后终止整个进程组并返回 `timeout`；
- 从 Finder 启动的应用 `PATH` 很短，Core 会为子进程补上 `/opt/homebrew/bin`、`/usr/local/bin`、`~/.local/bin` 等目录，以便 chezmoi 找到 gpg、git；
- 开发期间屏幕锁定，GUI 的视觉效果（布局、间距、深浅色）尚未人工确认；交互逻辑由无头窗口测试验证，应用在真实配置下启动和加载正常；
- 启用 GPUI Kit 的 `test-support` 后，`gpui-fast` 的精确版本要求与默认解析结果冲突（taffy 等），因此 Cargo.lock 以上游 gpui-kit v0.9.0 的锁文件为基础生成。升级 gpui-kit 时应同样参照上游锁文件；
- 界面文字目前只有英文。
