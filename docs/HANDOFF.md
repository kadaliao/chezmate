# 交接：给新手的简化模型（新架构）

写于 2026-11-07，供新会话接手。先读本文，再读 `AGENTS.md`、`docs/STATUS.md`、`README.md`。

## 1. 为什么要做

用户试用后的反馈：

> 我觉得这一套体系还是有点复杂，一个小白用户可能看不太懂，有没有办法再简化一点？……可不可以有一个更抽象、更简化的模型呢？

现在的界面把 chezmoi 的概念直接暴露给了用户：apply / re-add（Keep Local）、源文件与目标文件、模板与属性、`status` 的两列状态。会用 chezmoi 的人看得懂，新手看不懂。

产品原则（来自最初的需求）：像系统设置一样简单；像现代 Git 客户端一样清楚地展示变化；像标准开发工具一样能被 AI Agent 稳定操作。

用户两次提出"能不能脱离 chezmoi，自己实现"。这是新会话要先和用户确认的方向问题（见第 4 节）。

## 2. 现状（已完成、已提交）

| 部分 | 内容 |
| --- | --- |
| `chezmate-core` | 调用真实 chezmoi / git：status、managed（解析源文件名属性）、diff、apply（dry-run / force / 冲突识别）、add、re-add（报告跳过的模板）、forget、cat、source-path、doctor（30 秒超时、杀进程组）、git status / log / diff / commit / fetch / pull（冲突自动 abort）/ push |
| `chezmate-cli` | `chezmate …` 全部命令，`--json` 固定结构；`chezmate mcp`：双时代 MCP 服务（2026-07-28 无状态 + 2025-11-25 及更早），15 个工具，`--read-only` |
| `chezmate-app` | GPUI Kit 0.9：Changes / Files / Repository / Environment 四页 |
| 测试 | 60 个，全部在临时沙箱里跑真实 chezmoi 和 git；UI 测试用无头窗口 |

最后一次提交：`97ef7d6`（每次 apply 都运行的脚本不再算待处理）。工作区干净，没有远端，没有推送。

## 3. 已验证的事实（设计时直接用）

### chezmoi 行为

- `status` 第一列 = 磁盘相对 chezmoi 上次写入的变化，第二列 = apply 会做什么。
- **`MM` 有歧义**：「只有本机改过」和「本机改过且源也变了」都显示 `MM`。可以区分（2026-11-07 在沙箱里验证）：
  - `chezmoi state dump --format json` 的 `entryState[<绝对路径>].contentsSHA256` 是上次写入时的哈希；
  - `chezmoi cat <目标>` 的 sha256 是现在源状态渲染出来的哈希；
  - 两者相同 → 源没变，只有本机改过；不同 → 两边都改了。
  - state dump 里只有哈希、模式、类型，不含文件内容（buckets：configState、entryState、scriptState、gitHub*State、gitRepoExternalState）。
- `run_` 且没有 `once_` / `onchange_` 的脚本每次 apply 都运行，`status` 永远列出它。Core 已用 `StatusEntry::runs_every_apply` 标出。
- `re-add` 静默跳过模板（无法从结果反推模板）；Core 会报告被跳过的项。
- `--no-tty` 下，本地修改冲突会让 apply 以 EOF 失败；`forget` 必须加 `--force`。
- 相对路径参数按进程工作目录解析，Core 一律传绝对路径。
- 本机 `chezmoi doctor` 会卡在 1Password 的 `op --version`。

### 用户本机的实际配置（只读统计）

- 2 台 Mac（MacBook Pro `mbp`、Mac mini `mm`），通过 Git 远端 `origin/main` 同步源目录。
- 326 个托管条目：204 个文件、114 个目录（其中 107 个是没有属性的普通目录）、8 个脚本。
- 属性：private 20、template 11、encrypted 9（GPG，`.asc`）、executable 6、onchange 6、once 1、empty 1、create 1。
- 8 个脚本：7 个 `run_onchange_` / `run_once_`，1 个每次都运行（`run_after_98-ensure-brewfile.sh`）；其中 4 个是模板。
- 源目录根部有 `.chezmoi.toml.tmpl`、`.chezmoiignore`、`.chezmoiignore.tmpl`、`.chezmoi-scripts/`。
- 模板数据键：`email`、`frp_machine`、`frp_secrets`、`gitname`、`is_linux`、`is_macos`（只看过键名；值可能含密钥，不要打印）。

### GPUI Kit 0.9 的坑

- 文档和实际 API 有出入，以 `~/.cargo/registry/src/*/gpui-component-0.9.0` 源码为准：`Sidebar::new(id)` 需要 ID；`AsyncApp` 上用 `cx.background_executor().spawn(...)`；`ListItem` 没有 `secondary_selected`。
- 开启 `test-support` 后依赖解析会冲突，`Cargo.lock` 以上游 gpui-kit v0.9.0 的锁文件为基础。
- UI 测试里，输入后要先离开 `update_window`（让订阅回调执行），再点依赖输入状态的按钮。
- 真机检查：AppleScript `click at` 是无障碍按压，列表行不响应。要用 CGEvent 发真实鼠标事件（见 `AGENTS.md`）。屏幕会锁，`caffeinate` 只防闲置锁屏。

## 4. 待决定：要不要脱离 chezmoi

新会话**先和用户确认这一点**。我的建议是 A。

**A. 简化模型建在 chezmoi 之上（建议先做）**

- 用户看到的是新模型，chezmoi 成为看不见的引擎；
- 现有 326 个文件、GPG 加密、模板、脚本、两台机器的同步流程都不用迁移；
- 新手觉得难，是因为暴露的概念，不是因为引擎。换引擎不会让界面自动变简单，模型层才会；
- 模型层定义成与引擎无关，以后真要换引擎，替换的只是模型层下面的部分。

**B. 自建引擎替换 chezmoi**

- 可能的形态：一个普通 Git 仓库，文件按家目录里的相对路径原样存放（`files/.zshrc`），另有一个清单文件 `chezmate.toml` 记录哪些文件加密、哪些按机器区分；本机保存每个文件上次同步时的哈希，用来判断冲突；
- 必须解决：
  - 怎样替代模板（用户的 11 个模板，主要是按机器区分内容和注入密钥）；
  - 加密：用 `age` crate，还是继续调用 GPG（用户现有一把 GPG 密钥）；
  - 初始化脚本的"只运行一次"和"变化时运行"语义；
  - `.chezmoiignore` 模板；
  - 从 chezmoi 迁移，并且两台机器要分别切换；
  - 迁移失败时怎么回滚；
- 开源定位也会变：从"chezmoi 的图形界面"变成"和 chezmoi 竞争的新工具"。

折中的路线：先做 A，用户用上简化界面以后，如果仍然觉得受 chezmoi 限制，再在同一个模型下实现 B。

## 5. 简化模型草案（A、B 通用）

### 用户只需要理解的东西

- **一个对象：同步的配置文件**，用家目录里的路径称呼（`~/.zshrc`），不出现"源文件""目标"这类词。
- **每个文件只有一种状态**：

| 状态（界面用语，可中文） | 含义 | 由什么推出（A 方案） |
| --- | --- | --- |
| 已同步 | 什么都不用做 | 不在 `status` 里；或只有每次都运行的脚本 |
| 这台电脑上改过 | 本机有修改，还没存到共享版本 | `status` 第一列非空，且 state 哈希 == 目标哈希 |
| 有新版本 | 共享版本更新了（通常来自另一台电脑） | 第一列为空，第二列为 A/M/D |
| 两边都改了 | 需要用户选择 | `MM`，且 state 哈希 != 目标哈希 |
| 这台电脑上删除了 | 本机删掉了这个文件 | 第一列为 D |
| 各台电脑内容不同 | 模板，本机修改无法自动保存 | 托管条目 `attributes.template`，且本机改过 |

- **一个主按钮：同步**。先展示摘要（"保存 1 个、更新 2 个、运行 1 个设置步骤"），确认后依次执行：
  1. 把"这台电脑上改过"的文件存回共享版本（re-add；模板跳过）；
  2. 提交源目录的改动，然后 `pull --rebase --autostash`（冲突时自动 abort，Core 已实现）；
  3. 重新计算状态，把"有新版本"的文件应用到本机（只 apply 不冲突的目标）；
  4. 推送；
  5. "两边都改了"的文件单独列出，每个提供「保留这台电脑的」/「用共享版本」，并能查看差异。
- **设置步骤（脚本）**单独成区，不算"变更"：`once` / `onchange` 有待运行时显示；每次都运行的只在详情里出现。
- 现有的 Changes / Files / Repository / Environment 页面保留，放进"高级"里给熟悉 chezmoi 的人用。

### 需要新会话先想清楚的点

- 同步第 1 步会自动把本机改动存回共享版本。有些文件会被程序自己改写（用户的 `openlist/data/config.json` 就是这样），所以同步前的摘要确认是必须的；还可以考虑"这个文件只在本机保存"（转为忽略）。
- 同步时脚本怎么处理：完整 apply 会运行每次都运行的脚本；只 apply 指定目标则不会运行。
- 界面语言：现在只有英文。面向新手、用户是中文母语者，简化界面可能应该先做中文。需要问用户。
- Agent 也应该能用同一个模型：CLI `chezmate sync [--dry-run]` / `chezmate overview`，MCP `sync_overview` / `sync`（默认 dry run），错误类型沿用现有的 `kind`。

## 6. 建议的实施顺序

1. 和用户确认第 4 节的方向和第 5 节的模型（可以用界面草图让用户选）。
2. Core 新增 `sync` 模块：`Overview`（每个文件的状态 + 仓库同步状态 + 设置步骤）、`SyncPlan`、执行函数。测试搭"两台机器"：两个沙箱 HOME 共享一个本地裸仓库，参照 `crates/chezmate-core/tests/git.rs`。
3. GUI 新增"概览"首页和同步流程，现有页面移进"高级"。
4. CLI / MCP 接入同一个模型，并加端到端测试。
5. 每完成一步跑测试并提交，同时更新 `docs/STATUS.md` 和本文。

## 7. 约束（沿用）

- 遵守 `AGENTS.md`：调用真实 chezmoi，不用 mock；测试只在沙箱里跑；在用户的真实配置上只做只读检查，绝不点 Apply、Keep Local、Commit、Pull、Push。
- 不打印凭据或模板数据的值。
- 用户偏好：中文沟通；改完并验证后直接提交，不用问；推送要等用户明确说；提交身份 `Kada Liao <kadaliao@gmail.com>`（已配在仓库里）。
- 不要过度设计：只有一个引擎时不加 trait；模型先做成具体类型。
