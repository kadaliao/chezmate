# 实施状态

最后更新：2026-11-07

## 阶段

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| 0 | Workspace、最小 GPUI Kit 窗口 | 完成 |
| 1 | Core（真实 chezmoi 操作）+ CLI（`--json`） | 完成 |
| 2 | GUI：变更列表、差异视图、应用 / 保留本地修改、托管文件、环境 | 完成 |
| 3 | Git：源仓库状态、提交、拉取、推送 | 完成 |
| 4 | MCP 服务（`chezmate mcp`，stdio） | 完成 |
| 5 | 新手简化模型：每个文件一种状态 + 一个「同步」（Core、GUI 概览页、CLI、MCP），中英双语 | 完成 |
| 6 | 新手引导：没装 chezmoi、没有源目录、配置模板提问、选第一批文件、上传到 GitHub（Core、GUI；应用自带 chezmoi） | 完成 |

## 下一步

见 [HANDOFF.md](HANDOFF.md)。

## 已实现（新手引导）

2026-11-07 用户要求：没用过 chezmoi、没有源目录的人打开应用也要顺利开始。与用户确认：应用自带 chezmoi；共享位置用 `gh` 一键建私有仓库，或粘贴空仓库地址。

| 情况 | 应用里看到的 | Core |
| --- | --- | --- |
| 找不到 chezmoi | 全屏说明（`brew install chezmoi`、复制按钮、官网链接、「重新检查」）。发布的应用在 `Contents/Helpers/chezmoi` 自带一份，`PATH` 里找不到时用它，所以正常不会出现 | `ChezmoiOptions::fallback_binary` |
| 没有源目录（也没有配置文件；有配置文件说明源目录被移动或在没接上的硬盘里，显示说明而不是欢迎页，取回也会拒绝，以免覆盖配置） | 欢迎页两张卡片：「这是我的第一台电脑」→ `chezmoi init`；「我已经在别的电脑上用过」→ 输入 GitHub 用户名或地址 → `chezmoi init <仓库>`（不 apply）。失败时就地说明原因（找不到 / 没权限 / 连不上），不留下空目录 | `start_new`、`start_from`（简写先按 `gh` 设置的协议试，「找不到 / 拒绝」时换另一种）、`remote_problem` |
| 配置模板有问题要问（有模板、没有配置文件、有问题） | 表单：文字、数字、是 / 否开关、单选、多选；「继续」= 带答案再跑一次 `chezmoi init`；chezmoi 问了模板里看不出来的问题（文字由模板计算）时，按它打印的提示加进表单；「跳过」写一个空配置。模板里有 `stdinIsATTY` 时不预先列问题：没有终端时它本来就用默认值，只问 chezmoi 真正问到的 | `parse_questions`、`config_questions`、`init_config`、`unanswered_question`（先读调用本身，读不出时用 stdout 里的提示）、`skip_config`。答案参数：同类答案合成一个 CSV 记录，必要时补一个占位键 `__chezmate=1`，因为只有一个 `=` 时 chezmoi 的参数解析器只去掉两端引号、不按 CSV 读；数字参数不支持引号，问题里有 `,` / `=` 时直接拒绝 |
| 源目录里还没有文件 | 概览页换成「选择要同步的文件」：家目录里常见配置文件的勾选列表（按 Shell / Git / 编辑器 / 终端 / SSH 分组），也可以用文件选择器选其他文件 | `suggestions`（跳过符号链接、空文件、超过 1 MB 的文件、超过 200 个文件或 4 MB 的文件夹、git 仓库；`.npmrc`、`.netrc`、密钥不在候选里） |
| 有文件但没有任何远端（有远端但没设跟踪分支的不算，不提示） | 概览页左上提示「只在这台电脑上」+「上传到 GitHub…」对话框：`gh` 已登录时一键新建私有仓库（名字默认 `dotfiles`）并上传；否则打开 GitHub 新建页面，粘贴空仓库地址上传 | `connect_shared`（不是仓库时先 `git init`；已有指向别处的 `origin` 时拒绝；提交，加 `origin`，`push --set-upstream`；上传失败时去掉刚加的 `origin`）、`GitHub`（`gh api user`、`gh repo create --private`、`git_protocol`） |

其他：git 推断不出提交身份时（新电脑上常见），`Git::commit_all` 只为这一次提交加 `-c user.name / user.email`（`$USER@主机名`），不写进任何配置，之后设置的身份或按远端地址生效的 `includeIf` 身份照常起作用；chezmoi 启动的 git 设置 `GIT_TERMINAL_PROMPT=0`，克隆私有仓库时不会卡在终端提问；设置过程中的错误在进入概览后作为通知显示，不会随页面消失。独立审查（2026-11-07）后据此修正了：上传对话框会替换已有远端、计算出的问题问不到、答案引号编码、身份写进仓库配置、源目录丢失时出现欢迎页。

测试：`chezmate-core/tests/setup.rs` 用两台沙箱电脑和本地裸仓库，覆盖新建 → 添加 → 连接 → 另一台取回（提交身份不写入配置）、模板提问（逗号和引号、漏答、文字由模板计算的问题）、`stdinIsATTY` 模板、已有配置时拒绝取回、跳过、失败的取回不留目录、远端已有内容被拒且不留 `origin`、不替换已有远端、源目录不是仓库、文件建议；单元测试覆盖模板解析、错误识别、`gh` 输出解析、自带 chezmoi 的查找顺序。GUI：无头窗口测试走完「第一台电脑」（开始 → 取消一个勾选 → 添加 → 粘贴地址上传 → 对话框关闭、远端有文件）和「第二台电脑」（错误地址的提示 → 正确地址 → 回答问题 → 概览显示待同步文件）以及缺少 chezmoi、源目录丢失时不显示欢迎页、有远端时不提示上传；`--test screenshots` 离屏截图检查了各个画面（中文、浅色，欢迎页另看了深色）。

没有验证的：`gh repo create` 一键建库没有端到端测试（会在真实 GitHub 账号里建仓库），只测了参数和输出解析；没装 Xcode 命令行工具的 Mac 上 `/usr/bin/git` 只是一个会弹安装框的占位程序，Chezmate 目前不单独处理；2026-11-07 用 v0.1.4 的发布包验证过自带 chezmoi：解压发布的 zip，用 `sandbox-exec` 挡住 Homebrew 装的 chezmoi，在空的临时 HOME 里启动，欢迎页 → 开始 → 添加文件都正常，环境页显示用的是 `Chezmate.app/Contents/Helpers/chezmoi`；签名（Developer ID、hardened runtime）、公证和 Gatekeeper 检查都通过。注意：终端里如果设置了 `XDG_CONFIG_DIRS`，chezmoi 也会到那里找配置文件（从 Finder 启动没有这个问题）。

## 已实现（简化模型）

2026-11-07 与用户确认：建在 chezmoi 之上（不自建引擎）；界面中英双语、跟随系统；同步前显示摘要，要保存的文件可逐个取消；概览页用左列表 + 右详情。

| 部分 | 内容 | 测试 |
| --- | --- | --- |
| Core `sync` | `Overview`（文件状态、设置步骤、每次都运行的脚本、共享仓库状态）、`SyncPlan`、`execute_sync`、`fetch_shared` / `pull_shared`、`sync`（CLI / MCP 用） | 单元测试；`tests/sync.rs` 用两个沙箱 HOME 共享一个本地裸仓库，覆盖：改动在两台电脑间传递、两边都改、新电脑已有文件、跳过保存、本机删除、模板和设置步骤、新目录、无仓库、计划过期、离线、共享版本冲突 |
| GUI 概览 | 首页；分组列表、状态说明、按状态给出操作（存到共享版本 / 放弃修改 / 保留这台电脑的 / 用共享版本 / 恢复 / 不再同步 / 现在更新 / 现在运行 / 打开模板），差异方向随状态调整并标注颜色含义；同步 = 获取共享版本 → 确认对话框（可取消勾选保存项，离线时提示）→ 执行 | 无头窗口 UI 测试（含两台电脑同步、取消勾选、离线）；`--test screenshots` 离屏截图目测（中英文、浅色和深色） |
| CLI | `chezmate overview`、`chezmate sync [--dry-run] [--skip T]` | 沙箱集成测试 |
| MCP | `sync_overview`（只读）、`sync`（默认 dry run）；原有 15 个工具名称和顺序不变 | 端到端测试（新旧协议、只读模式） |

状态如何得出（chezmoi `status` 两列 + 持久状态）：

| 状态 | 条件 |
| --- | --- |
| 这台电脑上改过 `changed_here` | 第一列为 M/A，且 `state dump` 里上次写入的哈希 == `chezmoi cat` 的哈希 |
| 两边都改了 `both_changed` | 第一列为 M/A 且哈希不同；或第一列为空、chezmoi 没有写入记录，且第二列为 M（文件或链接）或 D（`exact_` 目录会删掉它；`remove_` 除外） |
| 有新版本 `update_available` | 第一列为空，第二列为 A/M/D（除上一行） |
| 这台电脑上删除了 `deleted_here` | 第一列为 D；或位于本机已删除的文件夹里的新文件 |
| 不能自动保存 `cannot_save` | 模板、`modify_` / `remove_`、目录；本机改过时归入「需要你决定」。本机改过权限的文件夹作为单独一行显示 |

同步的执行顺序：保存（re-add）→ 更新与设置步骤（分 `before_` / 普通 / `after_` 三轮 apply，`--recursive=false`，自动带上本机还不存在的父目录；有更新时才运行每次都运行的脚本）→ 有改动就提交 → 有远端就推送（失败只作为 `push_error` 报告）。每一项执行前都按当前状态重新检查，已经变化的跳过。

## 已实现（Core / CLI）

| 操作 | Core | CLI | 测试 |
| --- | --- | --- | --- |
| 版本与目录 | `info`（含配置模板 `config_template`）/ `paths` | `info` | 沙箱集成测试 |
| 状态 | `status` | `status` | 单元 + 集成 |
| 托管列表（含属性解析） | `managed` | `managed` | 单元 + 集成 |
| 差异 | `diff`（内置 diff，Git 格式） | `diff` | 集成 |
| 应用 | `apply`（dry-run / force / 冲突识别） | `apply` | 集成 |
| 添加 | `add`（template / encrypt） | `add` | 集成 |
| 保留本地修改 | `re_add`（报告被跳过的模板） | `re-add` | 集成 |
| 停止托管 | `forget` | `forget --yes` | 集成 |
| 渲染内容 / 源路径 | `cat` / `source_path` | `cat` / `source-path` | 集成 |
| 诊断 | `doctor`（30 秒超时） | `doctor` | 单元 + 集成 |

## 已实现（Git）

| 操作 | Core（`Git`） | CLI（`chezmate git …`） | 说明 |
| --- | --- | --- | --- |
| 状态 | `status`（porcelain v2：分支、上游、领先 / 落后、变更） | `status` | 不受系统语言影响 |
| 历史 | `log` | `log -n` | 空仓库返回空列表 |
| 未提交差异 | `diff`（含未跟踪文件，最多 100 个） | `diff` | 首次提交前对比空树 |
| 提交 | `commit_all`（`add --all` + `commit`） | `commit -m` | 空信息、无变更时报 `invalid_request` |
| 获取 / 拉取 / 推送 | `fetch` / `pull` / `push` | `fetch` / `pull` / `push` | pull 用 `--rebase --autostash`，冲突时自动 `rebase --abort`；pull 只更新源目录，不 apply |

网络操作设置 `GIT_TERMINAL_PROMPT=0`（需要凭据时直接失败，不会挂起），并有 180 秒超时。SSH 私钥需要由 ssh-agent / 钥匙串提供。

测试：解析单元测试；沙箱集成测试使用本地裸仓库作为远端，另一个克隆模拟另一台机器（推送、获取、拉取、冲突回滚、空仓库）；git 全局配置隔离（`GIT_CONFIG_GLOBAL`、`GIT_CONFIG_NOSYSTEM`）。

## 已实现（MCP）

`chezmate mcp [--read-only]`，stdio，换行分隔的 JSON-RPC 2.0，手写实现（只依赖 serde_json）。

- 双时代：`initialize` 进入旧协议（协商 2025-11-25 / 2025-06-18 / 2025-03-26 / 2024-11-05）；带 `_meta["io.modelcontextprotocol/protocolVersion"]` 的请求按 2026-07-28 无状态处理（结果带 `resultType`、`_meta` 中的 serverInfo，`tools/list` 带 `ttlMs` / `cacheScope`）；不支持的版本返回 `-32022` 并列出支持的版本；实现 `server/discover`；兼容 JSON-RPC 批量请求；
- 15 个工具，顺序固定；参数用 `deny_unknown_fields` 严格校验，错误作为工具结果（`isError`）返回；
- 安全默认：apply 默认 dry run，diff 默认排除加密文件，`--read-only` 隐藏所有写操作；chezmoi 不存在时服务仍启动，工具返回 `chezmoi_not_found`。

测试：协议单元测试；端到端测试启动真实 `chezmate mcp` 进程，分别用旧协议和新协议完成 状态 → diff → dry run → apply → 冲突 → re-add 流程，并验证只读模式无法写入。已对本机真实配置做只读冒烟测试。

## 已实现（GUI）

| 页面 | 功能 | 测试 |
| --- | --- | --- |
| Changes | 待应用列表（创建 / 更新 / 删除 / 运行脚本，本地修改标记）、选中项的差异（统一 / 并排）、Apply、Apply All、Keep Local（re-add）；覆盖本地修改、运行脚本、删除前确认；模板不可 Keep Local 时禁用并说明原因 | 无头窗口 UI 测试 + 纯逻辑单元测试 |
| Files（文件） | 按目标在家目录里的路径排成目录树（文件夹在前，可展开、全部折叠、过滤时自动展开；脚本单独一组；有变化的文件和包含它们的文件夹带颜色圆点）；单击后右侧上方显示基本信息（状态、路径、源文件、大小、修改时间、权限、属性，有变化时一条说明），下方直接显示内容：「差异 / 这台电脑 / 源文件」三个标签（有变化时默认差异），只读代码编辑器（行号、语法高亮、⌘F 搜索），图片预览，二进制 plist 用 `plutil` 转成 XML 只读显示，其他二进制、过大、缺失、加密源文件给出说明；加密文件「这台电脑」标签直接显示解密后的内容，上方标注「同步时以加密形式保存到共享版本」，编辑保存后同步会重新加密；点「编辑」才能改，保存写回家目录或源目录（模板只能改源文件），未保存时不能切换文件；Add…、停止管理（确认）、跳转到 Changes；中英双语 | 纯逻辑单元测试（目录树、语言识别、内容分类含真实 plist 转换、标签页）；无头窗口 UI 测试（展开、单击显示内容、编辑保存后概览状态变化、未保存时切换被拦、有变化默认显示差异、模板只能改源文件、加密文件直接显示且编辑后保存回源目录仍是密文、保存失败显示磁盘实际内容）；文件选择器本身为原生 UI，测试直接调用同一模型命令；离屏截图目测 |
| Repository | 分支与上游、领先 / 落后、Fetch / Pull / Push、未提交变更列表与差异、提交信息 + Commit All、最近 30 条历史；非 Git 仓库时给出说明 | 无头窗口 UI 测试（提交 → 推送 → 另一台机器推送 → Fetch → Pull → Changes 出现待应用变化） |
| Environment（环境） | chezmoi 版本、路径、是否 Git 仓库、显示源目录；按需运行 doctor，运行后（成功或失败）可点「复制给 Agent」：剪贴板里是一段请求、chezmoi 版本和目录、需要注意的项、完整的 doctor 表格（失败时是错误信息），家目录路径写成 `~`；页面下方直接显示 chezmoi 配置文件（和文件页同一个只读代码编辑器，点「编辑」修改，保存后立即生效；文件不存在时显示空编辑器，保存即创建）；源目录里有 `.chezmoi.<格式>.tmpl` 时多一个「模板」标签，并说明 `chezmoi init` 会按模板重新生成配置；刷新失败（例如配置写坏）时页面顶部显示错误，可以就地改回；中英双语 | 无头窗口 UI 测试（诊断并复制、读剪贴板；配置不存在时编辑保存即创建、模板标签编辑保存）；复制内容的单元测试；离屏截图目测 |

演示模式：`chezmate-app --demo`（或 `CHEZMATE_DEMO=1`）在 `$TMPDIR/chezmate-demo` 重新生成一套两台电脑的假配置（`src/demo.rs`，离屏截图也用它），标题栏显示「演示模式」；所有操作都只作用于这个目录，演示脚本只写临时目录里的日志。测试：无头窗口 UI 测试检查各种状态都出现、运行设置步骤只写日志。

通用：后台线程读取，刷新时保留旧数据，新请求丢弃旧结果；同一时间只运行一个写操作；结果以通知显示；⌘R 刷新，↑↓ / j k 在变更列表中移动。

GUI 测试在沙箱里驱动真实 GPUI Kit 组件和真实 chezmoi（点击、输入、确认对话框），并检查磁盘上的文件。`cargo test -p chezmate-app real_setup -- --ignored --nocapture` 对本机真实配置做只读加载检查（只打印数量）。

## 已验证的 chezmoi 行为

- 相对路径参数按进程工作目录解析，Core 一律转换为绝对路径；
- `--no-tty` 下遇到本地修改冲突时，chezmoi 输出 `… has changed since chezmoi last wrote it` 后以 EOF 失败，Core 识别为 `conflict`；
- `forget` 不加 `--force` 会提示确认并失败；
- `re-add` 静默跳过模板；
- 没有 `once_` / `onchange_` 的 `run_` 脚本每次 apply 都运行，`status` 永远列出它；Core 标为 `runs_every_apply`，界面、CLI、MCP 不把它算作待处理；
- `MM` 无法区分"只有本机改过"和"两边都改了"；用 `chezmoi state dump` 中 `entryState` 的 `contentsSHA256` 与 `chezmoi cat` 的哈希比较区分（符号链接的记录不含 `cat` 输出末尾的换行）；
- chezmoi 从没写过、但已存在且内容不同的文件，`status` 只显示 ` M`，`apply` 会不提示直接覆盖；
- 只 apply 新目录里的文件会失败（父目录不存在），必须连同待创建的父目录一起 apply；
- `chezmoi apply <目录>` 默认递归，会覆盖目录里所有文件（包括本机改过的）；同步和概览页的单文件操作一律用 `--recursive=false`，只带上本机不存在的父目录；
- `exact_` 目录里 chezmoi 没写过的本机文件，`status` 显示 ` D`，apply 会直接删除；
- 指定目标 apply 时，chezmoi 按名字顺序运行脚本，忽略 `before_` / `after_`；完整 apply 才遵守；
- 本机删除的文件（`DA`）不加 `--force` 无法恢复（chezmoi 认为它在外部被改过）；
- `git pull --rebase --autostash` 在暂存的改动放不回去时返回成功，但留下冲突标记；Core 检测到后撤销拉取并恢复改动；
- chezmoi 拒绝添加位于其配置目录内的文件（测试沙箱据此布局）；
- `.chezmoi.os` 使用 Go 的命名（`darwin`）。

## 限制与环境问题

- 本机 `chezmoi doctor` 会卡在 1Password CLI 的 `op --version`（等待桌面端授权），Core 30 秒后终止整个进程组并返回 `timeout`；
- 从 Finder 启动的应用 `PATH` 很短，Core 会为子进程补上 `/opt/homebrew/bin`、`/usr/local/bin`、`~/.local/bin` 等目录，以便 chezmoi 找到 gpg、git；
- GUI 已在 macOS 浅色模式下用真实配置截图检查四个页面（2026-11-07），并据此调整：普通目录不单独列出、列表行统一两行、Files 详情的操作移到信息下方、加密文件改为在 Finder 中显示源文件。深色模式、Linux、Windows 尚未检查；
- 启用 GPUI Kit 的 `test-support` 后，`gpui-fast` 的精确版本要求与默认解析结果冲突（taffy 等），因此 Cargo.lock 以上游 gpui-kit v0.9.0 的锁文件为基础生成。升级 gpui-kit 时应同样参照上游锁文件；
- 简化界面（概览、同步、侧栏、状态栏、新手引导）和「文件」「环境」页中英双语；其余高级页面（Changes、Repository）只有英文；
- 新手引导的「一键在 GitHub 上建库」依赖 GitHub CLI（`gh`）已登录；HTTPS 地址的推送需要 git 能拿到凭据（`gh auth setup-git` 或钥匙串），SSH 地址需要 ssh-agent 里有密钥；
- 配置文件写坏后，chezmoi 连路径都读不出来。应用开着时环境页保留上次的内容并显示错误，可以直接改回；但写坏后重新启动应用，环境页只能显示错误，需要在别处修好配置；
- 语法高亮用 GPUI Kit 的 `syntect` 特性（纯 Rust，bat 的语法集合，约 200 种语言）；Shell 配置、`.gitconfig`、`~/.ssh/config`、无扩展名但有 `#!` 的脚本按名字或首行识别（`src/language.rs`）；
- GPUI Kit 默认只打包组件用到的图标，文件页额外用到的 Lucide 图标在 `src/assets.rs` 里用 `icon_assets!` 加入；
- 两台电脑改了共享版本里的同一个文件、且这台电脑的改动还没上传时，同步的拉取会被撤销并报错，需要在 Repository 页或终端处理；
- 真机截图需要屏幕解锁；锁屏时用 `CHEZMATE_SCREENSHOTS=<目录> cargo test -p chezmate-app --test screenshots` 离屏渲染（2026-11-07 用它检查了概览页的中英文、浅色和深色，以及文件页的目录树、信息、文本 / 差异 / 模板源文件 / 编辑中 / 图片 / 二进制 plist）。
