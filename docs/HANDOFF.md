# 交接：简化模型已完成

写于 2026-11-07，供新会话接手。先读本文，再读 `AGENTS.md`、`docs/STATUS.md`、`README.md`。

## 1. 背景与已定方向

用户试用后觉得 chezmoi 的概念（apply / re-add、源与目标、模板属性、`status` 两列）对新手太复杂。2026-11-07 和用户确认：

- **建在 chezmoi 之上**，不自建引擎。chezmoi 是看不见的引擎，用户看到的是「这台电脑」和「共享版本」；
- 简化界面**中英双语、跟随系统**（macOS 读 `AppleLanguages`，终端里的 `LANG` 常常和系统语言不一致）；
- 同步前**显示摘要，要保存的文件可以逐个取消**（有些文件会被程序自己改写，如用户的 `openlist/data/config.json`）；
- 概览页**左列表 + 右详情**，原来四个页面放进「高级（chezmoi）」。

## 2. 现状（已完成、已提交）

本次会话的提交（从 `b669634` 之后）：Core 同步模型 → CLI/MCP → GUI 概览页 → 离屏截图与文案修正 → 离线同步 → 拉取冲突修复 → 独立审查后的修复（目录递归 apply 会覆盖同目录文件、本机改过权限的文件夹被隐藏、`exact_` 目录删除本机文件）。工作区干净，没有推送。

| 部分 | 内容 |
| --- | --- |
| `chezmate-core/src/sync.rs` | `Overview`（每个文件一种 `FileState`：`changed_here` / `update_available` / `both_changed` / `deleted_here`，外加 `cannot_save`、`never_synced`）、`SetupStep`、`SharedStatus`、`SyncPlan`、`execute_sync`、`fetch_shared` / `pull_shared`、`sync` |
| `chezmate-core/src/git.rs` | `pull` 现在会撤销 autostash 冲突；`is_pull_conflict` |
| `chezmate-cli` | `chezmate overview`、`chezmate sync [--dry-run] [--skip T]`；MCP `sync_overview`、`sync`（共 17 个工具，原有 15 个不变） |
| `chezmate-app` | `overview.rs`（概览页 + 同步确认 `SyncReview`）、`i18n.rs`（`tr(en, zh)`）、`model.rs`（`prepare_sync` / `execute_sync` / `keep_here` / `take_shared` / `stop_syncing` / `run_step`）、`tests/screenshots.rs` |
| 测试 | 90 个，全部在沙箱里跑真实 chezmoi 和 git；核心场景用「两台电脑」（两个沙箱 HOME + 一个本地裸仓库） |

设计细节和状态推导规则见 `docs/STATUS.md`「已实现（简化模型）」。

### 之后：文件页（2026-11-07 用户要求）

用户要求：文件页按目标在系统上的路径排成可展开的目录树；单击显示元信息，双击在右侧内置编辑器里显示内容或差异，默认只读，体验要好看。已完成：`files.rs`（目录树、信息面板）、`viewer.rs`（查看器）、`language.rs`（高亮语言识别）、`assets.rs`（额外图标）。可编辑，保存写回家目录或源目录（等同于用户自己改文件；保存后概览页显示相应状态，同步照常处理）。

## 3. 这次验证到的事实

- `MM` 用哈希区分：`chezmoi state dump --format json` 的 `entryState[<绝对路径>].contentsSHA256` vs `chezmoi cat` 输出的 sha256。符号链接的记录不含 `cat` 末尾的换行。
- chezmoi 没写过、但已存在的不同文件：`status` 显示 ` M`，`apply` 直接覆盖不提示。Core 把它算作 `both_changed`（`never_synced`）。
- 只 apply 新目录里的文件会失败，必须带上待创建的父目录（`Overview::apply_targets`）；而 `chezmoi apply <目录>` 默认递归，会覆盖目录里的所有文件，所以一律加 `--recursive=false`（`ApplyOptions::non_recursive`、`Chezmoi::take_shared`）。新电脑上 `~/.config` 是 0700 这类情况很常见。
- 指定目标 apply 时脚本按名字顺序运行，忽略 `before_` / `after_`。同步分三轮 apply。
- `DA`（本机删除）恢复必须 `--force`。
- `git pull --rebase --autostash` 放不回暂存改动时返回 0 并留下冲突标记。
- GPUI Kit：`AlertDialog` 可以放子视图（`.child(entity)`），默认按钮 ID 是 `ok` / `cancel`；`gpui_kit::component::set_locale("zh-CN")` 让组件自带文字变中文；测试工具只登记可交互元素的 ID（普通 `div().id()` 用 `try_find` 找不到）。
- GPUI Kit 的 `Tree`：单击条目会选中并展开 / 折叠文件夹，不发选中事件，用 `observe` 跟踪键盘选择；条目内容的 `on_click` 先于树自己的处理运行，`ClickEvent::click_count()` 区分双击。测试里同一次 `update_window` 内连续点击时，实体事件（`cx.emit`）要等闭包结束才派发，需要分开点击。
- GPUI Kit 默认 `Assets` 只含组件图标；其他 Lucide 图标要用 `gpui_kit::assets::icon_assets!` 另外打包，否则不显示也不报错。
- 离屏截图：`VisualTestAppContext` + `render_to_image` 需要 `test-support` 且必须在主线程创建平台，所以 `tests/screenshots.rs` 是 `harness = false` 并用 `#[path]` 引入 app 模块；窗口要包一层 `gpui_kit::base::Root`，截图前推进时钟让对话框动画结束。锁屏也能用。

## 4. 建议的下一步

1. **真机目测**（屏幕解锁时）：`cargo run -p chezmate-app` 看用户真实配置的概览页和文件页。只看不点：绝不点「同步」「存到共享版本」「用共享版本」「恢复」「不再同步」「编辑 / 保存」「停止管理」等任何写操作（等同于原来禁止的 Apply / Keep Local / Pull / Push）。真实配置目前全部已同步，概览页应显示「已同步」和 204 个文件。文件页截图时不要打开加密文件（「这台电脑」标签会显示解密后的内容）。
2. **请用户自己试用同步**，收集反馈。重点：文案是否易懂、确认对话框是否清楚、「需要你决定」里的选择是否够用。
3. 可能的改进（等用户反馈再定，不要先做）：
   - 「只在这台电脑上保留」：把被程序改写的文件转为忽略（要处理 `.chezmoiignore.tmpl`）；
   - 共享版本层面的冲突（两台电脑改了同一个源文件且本机未上传）目前只报错，可以在界面里让用户选一边；
   - 定时后台 fetch，让「另一台电脑有更新」及时出现；
   - 高级页面的中文翻译。

## 5. 约束（沿用）

- 遵守 `AGENTS.md`：调用真实 chezmoi，不用 mock；测试只在沙箱里跑；在用户的真实配置上只做只读检查。
- 不打印凭据或模板数据的值。
- 用户偏好：中文沟通；改完并验证后直接提交，不用问；推送要等用户明确说；提交身份 `Kada Liao <kadaliao@gmail.com>`（已配在仓库里）。
- 不要过度设计：只有一个引擎时不加 trait；模型先做成具体类型。
