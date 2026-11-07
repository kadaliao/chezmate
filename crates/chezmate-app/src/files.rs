//! Files: every target chezmoi manages, laid out in the folders it lives in
//! on this computer. Selecting one shows where it comes from; opening it
//! (double-click or Open) shows its contents, its source and its changes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

use chezmate_core::{EntryKind, ManagedEntry};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    description_list::DescriptionList,
    h_flex,
    input::{Input, InputEvent, InputState},
    list::ListItem,
    notification::NotificationType,
    resizable::{h_resizable, resizable_panel},
    scroll::ScrollableElement as _,
    tag::Tag,
    tree::{TreeEvent, TreeItem, TreeState, tree},
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, Styled as _, Subscription, Task, Window, div, prelude::FluentBuilder as _, px,
    rems,
};

use crate::changes::{error_state, loading_state};
use crate::i18n::tr;
use crate::model::{Dotfiles, Snapshot};
use crate::overview::{explanation, state_title};
use crate::viewer::{CloseViewer, FileViewer, ViewTab, file_icon, human_size, tabs_for};

/// Asks the shell to show a target on the Changes page.
pub struct ReviewChange(pub String);

pub fn kind_label(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::File => tr("File", "文件"),
        EntryKind::Directory => tr("Folder", "文件夹"),
        EntryKind::Symlink => tr("Symbolic link", "符号链接"),
        EntryKind::Script => tr("Script", "脚本"),
        EntryKind::Modify => tr("Modify script", "修改脚本"),
        EntryKind::Remove => tr("Removal", "删除标记"),
    }
}

/// Human names for the attributes encoded in the source file name.
pub fn attribute_labels(entry: &ManagedEntry) -> Vec<&'static str> {
    let a = &entry.attributes;
    [
        (a.template, tr("Template", "模板")),
        (a.encrypted, tr("Encrypted", "加密")),
        (a.private, tr("Private", "私有")),
        (a.readonly, tr("Read-only", "只读")),
        (a.executable, tr("Executable", "可执行")),
        (a.exact, tr("Exact", "精确")),
        (a.create, tr("Create only", "仅创建")),
        (a.empty, tr("Keep if empty", "保留空文件")),
        (a.once, tr("Runs once", "只运行一次")),
        (a.onchange, tr("Runs on change", "变化时运行")),
    ]
    .into_iter()
    .filter_map(|(on, label)| on.then_some(label))
    .collect()
}

/// Directories chezmoi manages only because they contain managed files.
pub fn is_plain_directory(entry: &ManagedEntry) -> bool {
    entry.kind == EntryKind::Directory && entry.attributes == Default::default()
}

fn matches(entry: &ManagedEntry, query: &str) -> bool {
    query.is_empty()
        || entry.target.to_lowercase().contains(query)
        || entry.source_relative.to_lowercase().contains(query)
}

/// Indices of entries worth counting as files that match `query`
/// (case-insensitive, against the target or the source path).
pub fn filter_entries(entries: &[ManagedEntry], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| !is_plain_directory(e) && matches(e, &query))
        .map(|(ix, _)| ix)
        .collect()
}

/// The id of the tree row that holds scripts: they have no place in the
/// home directory.
pub const SCRIPTS: &str = "scripts:";

/// A row of the Files tree before it becomes a [`TreeItem`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// The target path; scripts are prefixed with [`SCRIPTS`].
    pub id: String,
    pub name: String,
    /// Index into the managed entries; `None` for a folder chezmoi does not
    /// list itself and for the scripts row.
    pub entry: Option<usize>,
    pub children: Vec<Node>,
}

/// The managed entries matching `query`, as folders and files under the
/// home directory, folders first. Scripts go in their own row at the end.
pub fn build_tree(entries: &[ManagedEntry], query: &str) -> Vec<Node> {
    let query = query.trim().to_lowercase();
    let mut roots = Vec::new();
    let mut scripts = Vec::new();
    for (ix, entry) in entries.iter().enumerate() {
        if !matches(entry, &query) {
            continue;
        }
        if entry.kind == EntryKind::Script {
            scripts.push(Node {
                id: format!("{SCRIPTS}{}", entry.target),
                name: entry.target.clone(),
                entry: Some(ix),
                children: Vec::new(),
            });
        } else {
            insert(&mut roots, &entry.target, ix);
        }
    }
    sort(&mut roots, entries);
    if !scripts.is_empty() {
        sort(&mut scripts, entries);
        roots.push(Node {
            id: SCRIPTS.to_string(),
            name: tr("Scripts", "脚本").to_string(),
            entry: None,
            children: scripts,
        });
    }
    roots
}

fn insert(roots: &mut Vec<Node>, target: &str, ix: usize) {
    let parts: Vec<&str> = target.split('/').collect();
    let mut level = roots;
    for (depth, part) in parts.iter().enumerate() {
        let position = match level.iter().position(|n| n.name == *part) {
            Some(position) => position,
            None => {
                level.push(Node {
                    id: parts[..=depth].join("/"),
                    name: part.to_string(),
                    entry: None,
                    children: Vec::new(),
                });
                level.len() - 1
            }
        };
        let node = &mut level[position];
        if depth == parts.len() - 1 {
            node.entry = Some(ix);
        }
        level = &mut node.children;
    }
}

fn is_folder(node: &Node, entries: &[ManagedEntry]) -> bool {
    !node.children.is_empty()
        || node
            .entry
            .is_some_and(|ix| entries[ix].kind == EntryKind::Directory)
}

/// Folders first, then by name ignoring case and a leading dot.
fn sort(nodes: &mut [Node], entries: &[ManagedEntry]) {
    nodes.sort_by_cached_key(|node| {
        let name = node.name.trim_start_matches('.').to_lowercase();
        (!is_folder(node, entries), name, node.name.clone())
    });
    for node in nodes {
        sort(&mut node.children, entries);
    }
}

/// How a row's file differs from the shared version, most urgent last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Mark {
    Update,
    ChangedHere,
    Decide,
}

impl Mark {
    fn color(self, cx: &App) -> Hsla {
        match self {
            Mark::Update => cx.theme().info,
            Mark::ChangedHere => cx.theme().warning,
            Mark::Decide => cx.theme().danger,
        }
    }
}

fn mark_for(snapshot: &Snapshot, target: &str) -> Option<Mark> {
    if let Some(file) = snapshot.overview.file(target) {
        return Some(if file.needs_decision() {
            Mark::Decide
        } else if file.state == chezmate_core::FileState::ChangedHere {
            Mark::ChangedHere
        } else {
            Mark::Update
        });
    }
    snapshot
        .overview
        .setup_steps
        .iter()
        .any(|step| step.target == target)
        .then_some(Mark::Update)
}

/// What a tree row shows, by row id.
struct Row {
    entry: Option<usize>,
    folder: bool,
    /// The most urgent mark of the row and everything inside it.
    mark: Option<Mark>,
    /// Files inside a folder; 1 for a file.
    files: usize,
}

fn to_item(
    node: &Node,
    snapshot: &Snapshot,
    expanded: &HashSet<String>,
    filtering: bool,
    rows: &mut HashMap<SharedString, Row>,
) -> TreeItem {
    let entry = node.entry.map(|ix| &snapshot.managed[ix]);
    let mut mark = entry.and_then(|e| mark_for(snapshot, &e.target));
    let folder = is_folder(node, &snapshot.managed) || node.id == SCRIPTS;
    let mut files = usize::from(!folder);
    let children: Vec<TreeItem> = node
        .children
        .iter()
        .map(|child| {
            let item = to_item(child, snapshot, expanded, filtering, rows);
            if let Some(row) = rows.get(&item.id) {
                mark = mark.max(row.mark);
                files += row.files;
            }
            item
        })
        .collect();
    rows.insert(
        node.id.clone().into(),
        Row {
            entry: node.entry,
            folder,
            mark,
            files,
        },
    );
    TreeItem::new(node.id.clone(), node.name.clone())
        .children(children)
        .expanded(filtering || expanded.contains(&node.id))
}

/// A file's details on disk, read when it is selected.
struct DiskInfo {
    size: u64,
    modified: Option<SystemTime>,
    mode: u32,
}

fn disk_info(path: &Path) -> Option<DiskInfo> {
    use std::os::unix::fs::PermissionsExt as _;
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some(DiskInfo {
        size: meta.len(),
        modified: meta.modified().ok(),
        mode: meta.permissions().mode(),
    })
}

/// `rw-r--r-- (644)`.
fn permissions(mode: u32) -> String {
    let bits = "rwxrwxrwx";
    let text: String = bits
        .chars()
        .enumerate()
        .map(|(i, c)| if mode & (1 << (8 - i)) != 0 { c } else { '-' })
        .collect();
    format!("{text} ({:o})", mode & 0o777)
}

/// "3 days ago".
fn ago(time: SystemTime) -> String {
    let seconds = SystemTime::now()
        .duration_since(time)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (n, en, zh) = match seconds {
        0..60 => return tr("just now", "刚刚").to_string(),
        60..3600 => (seconds / 60, "minute", "分钟"),
        3600..86400 => (seconds / 3600, "hour", "小时"),
        86400..2_592_000 => (seconds / 86400, "day", "天"),
        2_592_000..31_536_000 => (seconds / 2_592_000, "month", "个月"),
        _ => (seconds / 31_536_000, "year", "年"),
    };
    let s = if n == 1 { "" } else { "s" };
    tr(format!("{n} {en}{s} ago"), format!("{n} {zh}前"))
}

pub struct FilesView {
    model: Entity<Dotfiles>,
    filter: Entity<InputState>,
    query: String,
    tree: Entity<TreeState>,
    rows: Rc<HashMap<SharedString, Row>>,
    expanded: HashSet<String>,
    /// The snapshot and query the tree was built for.
    built_for: Option<(usize, String)>,
    file_count: usize,
    /// The selected row's id.
    selected: Option<String>,
    disk: Option<DiskInfo>,
    viewer: Entity<FileViewer>,
    /// The viewer, rather than the details, fills the right side.
    viewing: bool,
    _pick: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ReviewChange> for FilesView {}

impl FilesView {
    pub fn new(model: Entity<Dotfiles>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr("Filter files", "筛选文件")));
        let tree = cx.new(|cx| TreeState::new(cx));
        let viewer = cx.new(|cx| FileViewer::new(model.clone(), window, cx));
        let subscriptions = vec![
            cx.observe_in(&model, window, |this, _, window, cx| {
                this.rebuild(false, cx);
                if this.viewing {
                    this.viewer
                        .update(cx, |viewer, cx| viewer.reload(window, cx));
                }
            }),
            cx.subscribe_in(&filter, window, |this, state, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.query = state.read(cx).value().to_string();
                    this.rebuild(true, cx);
                }
            }),
            cx.subscribe(&tree, |this, _, event: &TreeEvent, _| match event {
                TreeEvent::Expanded(id) => {
                    this.expanded.insert(id.to_string());
                }
                TreeEvent::Collapsed(id) => {
                    this.expanded.remove(id.as_ref());
                }
            }),
            // Keyboard navigation moves the tree's selection.
            cx.observe_in(&tree, window, |this, tree, window, cx| {
                let Some(id) = tree.read(cx).selected_item().map(|i| i.id.to_string()) else {
                    return;
                };
                if this.selected.as_deref() != Some(id.as_str()) {
                    this.select(id, window, cx);
                }
            }),
            cx.subscribe(&viewer, |this, _, _: &CloseViewer, cx| {
                this.viewing = false;
                cx.notify();
            }),
        ];
        let mut this = Self {
            model,
            filter,
            query: String::new(),
            tree,
            rows: Rc::default(),
            expanded: HashSet::new(),
            built_for: None,
            file_count: 0,
            selected: None,
            disk: None,
            viewer,
            viewing: false,
            _pick: Task::ready(()),
            _subscriptions: subscriptions,
        };
        this.rebuild(true, cx);
        this
    }

    fn snapshot(&self, cx: &App) -> Option<Arc<Snapshot>> {
        self.model.read(cx).snapshot().cloned()
    }

    /// Rebuilds the tree for a new snapshot or query, keeping what was
    /// expanded and selected.
    fn rebuild(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot(cx) else {
            cx.notify();
            return;
        };
        let key = (Arc::as_ptr(&snapshot) as usize, self.query.clone());
        if !force && self.built_for.as_ref() == Some(&key) {
            return;
        }
        self.built_for = Some(key);
        let filtering = !self.query.trim().is_empty();
        let mut rows = HashMap::new();
        let items: Vec<TreeItem> = build_tree(&snapshot.managed, &self.query)
            .iter()
            .map(|node| to_item(node, &snapshot, &self.expanded, filtering, &mut rows))
            .collect();
        self.file_count = filter_entries(&snapshot.managed, &self.query).len();
        self.rows = Rc::new(rows);

        // A forgotten file is gone; a filtered-out one stays selected.
        let still_managed = self.selected.as_ref().is_some_and(|id| {
            id == SCRIPTS
                || self.rows.contains_key(id.as_str())
                || snapshot
                    .managed_entry(id.strip_prefix(SCRIPTS).unwrap_or(id))
                    .is_some()
        });
        if !still_managed {
            self.selected = None;
            self.viewing = false;
        }
        let selected = self.selected.clone();
        self.tree.update(cx, |tree, cx| {
            tree.set_items(items, cx);
            if let Some(ix) = selected.and_then(|id| tree.index_of(&id.into())) {
                tree.set_selected_index(Some(ix), cx);
            }
        });
        self.read_disk(&snapshot);
        cx.notify();
    }

    fn read_disk(&mut self, snapshot: &Snapshot) {
        self.disk = self
            .selected_entry(snapshot)
            .and_then(|entry| disk_info(Path::new(&entry.absolute)));
    }

    fn selected_entry<'a>(&self, snapshot: &'a Snapshot) -> Option<&'a ManagedEntry> {
        let id = self.selected.as_ref()?;
        let target = id.strip_prefix(SCRIPTS).unwrap_or(id);
        snapshot.managed_entry(target)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn visible_count(&self) -> usize {
        self.file_count
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn viewer(&self) -> Option<&Entity<FileViewer>> {
        self.viewing.then_some(&self.viewer)
    }

    /// Selects a row and shows its details. While an edit is unsaved the
    /// selection stays where it is.
    pub fn select(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.as_deref() == Some(id.as_str()) {
            return;
        }
        if self.viewing && self.viewer.read(cx).has_unsaved_changes() {
            let current = self.selected.clone();
            self.tree.update(cx, |tree, cx| {
                let ix = current.and_then(|id| tree.index_of(&id.into()));
                tree.set_selected_index(ix, cx);
            });
            window.push_notification(
                (
                    NotificationType::Warning,
                    SharedString::from(tr(
                        "Save or cancel the edit first.",
                        "请先保存或取消正在进行的编辑。",
                    )),
                ),
                cx,
            );
            return;
        }
        if self.viewer.read(cx).is_editing() {
            self.viewer
                .update(cx, |viewer, cx| viewer.cancel_editing(window, cx));
        }
        self.selected = Some(id);
        self.viewing = false;
        if let Some(snapshot) = self.snapshot(cx) {
            self.read_disk(&snapshot);
        }
        cx.notify();
    }

    /// Expands the folders around `target` and selects it. Used by the
    /// screenshot harness.
    #[allow(dead_code)]
    pub fn reveal(&mut self, target: &str, window: &mut Window, cx: &mut Context<Self>) {
        if target.starts_with(SCRIPTS) {
            self.expanded.insert(SCRIPTS.to_string());
        }
        let mut path = String::new();
        for part in target.split('/') {
            if !path.is_empty() {
                self.expanded.insert(path.clone());
                path.push('/');
            }
            path.push_str(part);
        }
        self.rebuild(true, cx);
        self.select(target.to_string(), window, cx);
        self.tree.update(cx, |tree, cx| {
            let ix = tree.index_of(&target.to_string().into());
            tree.set_selected_index(ix, cx);
        });
    }

    /// Opens the selected file in the viewer, on `tab` if it has one.
    pub fn open(&mut self, tab: Option<ViewTab>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot(cx) else {
            return;
        };
        let Some(entry) = self.selected_entry(&snapshot) else {
            return;
        };
        if tabs_for(entry, false).is_empty() && snapshot.review_diff(&entry.target).is_none() {
            return;
        }
        let target = entry.target.clone();
        self.viewer.update(cx, |viewer, cx| {
            if viewer.target() != target || !viewer.is_editing() {
                viewer.open(target, window, cx);
            }
            if let Some(tab) = tab {
                viewer.set_tab(tab, window, cx);
            }
        });
        self.viewing = true;
        cx.notify();
    }

    fn clicked(
        &mut self,
        id: SharedString,
        clicks: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // With an unsaved edit, the tree's own selection change puts the
        // selection back and says why.
        if self.viewing && self.viewer.read(cx).has_unsaved_changes() {
            return;
        }
        self.select(id.to_string(), window, cx);
        let is_file = self.rows.get(&id).is_some_and(|row| !row.folder);
        if clicks >= 2 && is_file && self.selected.as_deref() == Some(id.as_ref()) {
            self.open(None, window, cx);
        }
    }

    fn collapse_all(&mut self, cx: &mut Context<Self>) {
        self.expanded.clear();
        self.rebuild(true, cx);
    }

    /// Opens the system file picker and adds the chosen files.
    fn pick_and_add(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some(tr("Add", "添加").into()),
        });
        let model = self.model.clone();
        self._pick = cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(paths))) = receiver.await else {
                return;
            };
            let paths: Vec<String> = paths
                .into_iter()
                .map(|path: PathBuf| path.to_string_lossy().into_owned())
                .collect();
            if paths.is_empty() {
                return;
            }
            model.update(cx, |model, cx| model.add(paths, cx));
        });
    }

    fn forget(
        &mut self,
        entry: &ManagedEntry,
        files: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let model = self.model.clone();
        let target = entry.target.clone();
        let source = entry.source_relative.clone();
        let folder = entry.kind == EntryKind::Directory;
        window.open_alert_dialog(cx, move |alert, _, _| {
            let model = model.clone();
            let target = target.clone();
            let description = if folder {
                tr(
                    format!(
                        "{source} and the {files} files in it are removed from your dotfiles. The files in your home directory stay as they are."
                    ),
                    format!(
                        "{source} 和其中的 {files} 个文件会从 dotfiles 里移除，家目录里的文件保持不变。"
                    ),
                )
            } else {
                tr(
                    format!(
                        "{source} is removed from your dotfiles. The file in your home directory stays as it is."
                    ),
                    format!("{source} 会从 dotfiles 里移除，家目录里的文件保持不变。"),
                )
            };
            alert
                .title(tr(
                    format!("Stop managing “{target}”?"),
                    format!("停止管理「{target}」？"),
                ))
                .description(description)
                .confirm()
                .ok_text(tr("Stop Managing", "停止管理"))
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, _, cx| {
                    let target = target.clone();
                    model.update(cx, |model, cx| model.forget(target, cx));
                    true
                })
        });
    }

    fn render_list(&self, snapshot: &Arc<Snapshot>, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.model.read(cx).busy().is_some();
        let rows = self.rows.clone();
        let entries = snapshot.clone();
        let owner = cx.entity().downgrade();
        let empty = rows.is_empty();

        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .h_12()
                    .flex_none()
                    .gap_2()
                    .pl_4()
                    .pr_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(tr("Files", "文件")),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.file_count.to_string()),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("collapse-all")
                            .ghost()
                            .small()
                            .icon(Icon::new(Lucide::ChevronsDownUp))
                            .tooltip(tr("Collapse All", "全部折叠"))
                            .on_click(cx.listener(|this, _, _, cx| this.collapse_all(cx))),
                    )
                    .child(
                        Button::new("add-files")
                            .ghost()
                            .small()
                            .icon(IconName::Plus)
                            .tooltip(tr(
                                "Choose files in your home directory to manage. Press ⌘⇧. to show hidden files.",
                                "选择家目录里要管理的文件。按 ⌘⇧. 显示隐藏文件。",
                            ))
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.pick_and_add(cx))),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .py_2()
                    .child(Input::new(&self.filter).id("file-filter").small().cleanable(true)),
            )
            .child(
                h_flex()
                    .flex_none()
                    .px_4()
                    .pb_1()
                    .gap_1p5()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(Icon::new(Lucide::House).xsmall())
                    .child(
                        div()
                            .truncate()
                            .child(snapshot.info.paths.dest_dir.display().to_string()),
                    ),
            )
            .child(
                div()
                    .id("file-tree")
                    .flex_1()
                    .min_h_0()
                    .when(empty, |this| {
                        this.child(
                            div()
                                .p_4()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(tr("No managed files match.", "没有匹配的文件。")),
                        )
                    })
                    .child(
                        tree(&self.tree, move |_, item, selected, _, cx| {
                            tree_row(item, selected, &rows, &entries, owner.clone(), cx)
                        })
                        .size_full(),
                    ),
            )
            .into_any_element()
    }

    fn render_details(&self, snapshot: &Arc<Snapshot>, cx: &mut Context<Self>) -> AnyElement {
        let Some(id) = self.selected.clone() else {
            return empty_details(cx);
        };
        if id == SCRIPTS {
            return scripts_details(self.rows.get(id.as_str()).map_or(0, |r| r.files), cx);
        }
        let row = self.rows.get(id.as_str());
        let files = row.map_or(1, |r| r.files);
        let mark = row.and_then(|r| r.mark);
        let Some(entry) = self.selected_entry(snapshot).cloned() else {
            // A folder chezmoi does not list itself.
            return folder_details(&id, files, mark, snapshot, cx);
        };
        let busy = self.model.read(cx).busy().is_some();
        let folder = entry.kind == EntryKind::Directory;
        let openable = !tabs_for(&entry, false).is_empty();
        let attributes = attribute_labels(&entry);
        let source = PathBuf::from(&entry.source_absolute);
        let target = PathBuf::from(&entry.absolute);
        let (directory, name) = entry
            .target
            .rsplit_once('/')
            .map_or(("", entry.target.as_str()), |(d, n)| (d, n));
        let location = match (entry.kind, directory) {
            (EntryKind::Script, _) => {
                tr("Runs when chezmoi applies", "chezmoi 应用时运行").to_string()
            }
            (_, "") => "~".to_string(),
            (_, directory) => format!("~/{directory}"),
        };

        let header = h_flex()
            .gap_4()
            .child(icon_tile(file_icon(&entry), cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(name.to_string()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .truncate()
                            .child(location),
                    )
                    .child(
                        h_flex()
                            .flex_wrap()
                            .gap_1()
                            .pt_1()
                            .child(status_tag(&entry, mark, snapshot))
                            .children(
                                attributes
                                    .iter()
                                    .map(|label| Tag::secondary().small().child(*label)),
                            ),
                    ),
            )
            .when(openable, |this| {
                this.child(
                    Button::new("open-viewer")
                        .primary()
                        .small()
                        .icon(Icon::new(Lucide::Eye))
                        .label(tr("Open", "打开"))
                        .tooltip(tr("Double-click a file to open it", "双击文件也可以打开"))
                        .on_click(cx.listener(|this, _, window, cx| this.open(None, window, cx))),
                )
            });

        let mut location_list = DescriptionList::new()
            .columns(1)
            .bordered(true)
            .label_width(rems(7.));
        if entry.kind != EntryKind::Script {
            location_list =
                location_list.item(tr("This computer", "这台电脑"), entry.absolute.clone(), 1);
        }
        location_list = location_list.item(
            tr("Source file", "源文件"),
            entry.source_relative.clone(),
            1,
        );

        let mut file_list = DescriptionList::new()
            .columns(1)
            .bordered(true)
            .label_width(rems(7.))
            .item(tr("Type", "类型"), kind_label(entry.kind), 1);
        if folder {
            file_list = file_list.item(
                tr("Contains", "包含"),
                tr(format!("{files} files"), format!("{files} 个文件")),
                1,
            );
        }
        match (&self.disk, entry.kind) {
            (_, EntryKind::Script) => {}
            (Some(disk), _) => {
                if !folder {
                    file_list = file_list.item(tr("Size", "大小"), human_size(disk.size), 1);
                }
                if let Some(modified) = disk.modified {
                    file_list = file_list.item(tr("Modified", "修改时间"), ago(modified), 1);
                }
                file_list = file_list.item(tr("Permissions", "权限"), permissions(disk.mode), 1);
            }
            (None, _) => {
                file_list = file_list.item(
                    tr("On this computer", "这台电脑上"),
                    tr("Not yet", "还没有"),
                    1,
                );
            }
        }

        let status = snapshot.overview.file(&entry.target).cloned();
        let has_diff = snapshot.review_diff(&entry.target).is_some();
        let actions = h_flex()
            .gap_2()
            .flex_wrap()
            .when(target.exists(), |row| {
                let target = target.clone();
                row.child(
                    Button::new("reveal-target")
                        .outline()
                        .small()
                        .label(tr("Show in Finder", "在 Finder 中显示"))
                        .on_click(move |_, _, cx| cx.reveal_path(&target)),
                )
            })
            .child({
                let source = source.clone();
                // A folder or an encrypted file is not useful to open.
                let reveal = folder || entry.attributes.encrypted;
                Button::new("open-source")
                    .outline()
                    .small()
                    .label(if reveal {
                        tr("Show Source", "显示源文件")
                    } else {
                        tr("Open Source", "打开源文件")
                    })
                    .on_click(move |_, _, cx| {
                        if reveal {
                            cx.reveal_path(&source)
                        } else {
                            cx.open_with_system(&source)
                        }
                    })
            })
            .child(div().flex_1())
            .child({
                let entry = entry.clone();
                Button::new("forget")
                    .danger()
                    .small()
                    .label(tr("Stop Managing…", "停止管理…"))
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.forget(&entry, files, window, cx)
                    }))
            });

        let pending = mark.is_some() && !folder;
        let body = v_flex()
            .p_5()
            .gap_5()
            .when(pending, |this| {
                let title = match &status {
                    Some(file) => state_title(file).to_string(),
                    None => tr("Waiting to run", "等待运行").to_string(),
                };
                let text = match &status {
                    Some(file) => explanation(file),
                    None => tr(
                        "This script runs at the next sync.",
                        "下次同步时会运行这个脚本。",
                    )
                    .to_string(),
                };
                let target = entry.target.clone();
                this.child(
                    v_flex()
                        .gap_2()
                        .p_4()
                        .rounded_lg()
                        .border_1()
                        .border_color(mark.map_or(cx.theme().border, |m| m.color(cx).opacity(0.4)))
                        .bg(mark.map_or(cx.theme().muted, |m| m.color(cx).opacity(0.06)))
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .size_2()
                                        .rounded_full()
                                        .bg(mark.map_or(cx.theme().info, |m| m.color(cx))),
                                )
                                .child(
                                    div().text_sm().font_weight(FontWeight::MEDIUM).child(title),
                                ),
                        )
                        .child(div().text_sm().child(text))
                        .child(
                            h_flex()
                                .gap_2()
                                .pt_1()
                                .when(has_diff, |row| {
                                    row.child(
                                        Button::new("show-diff")
                                            .outline()
                                            .small()
                                            .label(tr("See Changes", "查看差异"))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.open(Some(ViewTab::Diff), window, cx)
                                            })),
                                    )
                                })
                                .child(
                                    Button::new("review-change")
                                        .ghost()
                                        .small()
                                        .label(tr("Review in Changes", "在 Changes 页处理"))
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.emit(ReviewChange(target.clone()))
                                        })),
                                ),
                        ),
                )
            })
            .child(section(
                tr("Location", "位置"),
                location_list.into_any_element(),
                cx,
            ))
            .child(section(
                if folder {
                    tr("Folder", "文件夹")
                } else {
                    tr("File", "文件")
                },
                file_list.into_any_element(),
                cx,
            ));

        v_flex()
            .size_full()
            .min_h_0()
            .child(
                v_flex()
                    .flex_none()
                    .gap_4()
                    .p_5()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(header)
                    .child(actions),
            )
            .child(
                div()
                    .id("file-details")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(body),
            )
            .into_any_element()
    }
}

fn tree_row(
    item: &gpui_kit::component::tree::TreeEntry,
    selected: bool,
    rows: &HashMap<SharedString, Row>,
    snapshot: &Snapshot,
    owner: gpui_kit::WeakEntity<FilesView>,
    cx: &App,
) -> ListItem {
    let id = item.item().id.clone();
    let row = rows.get(&id);
    let entry = row.and_then(|r| r.entry).map(|ix| &snapshot.managed[ix]);
    let folder = row.is_some_and(|r| r.folder);
    let expanded = item.is_expanded();
    let icon = if id.as_ref() == SCRIPTS {
        Lucide::SquareTerminal
    } else if folder {
        if expanded && item.is_folder() {
            Lucide::FolderOpen
        } else {
            Lucide::Folder
        }
    } else {
        entry.map_or(Lucide::FileText, file_icon)
    };
    let element_id: SharedString = match entry {
        Some(entry) if !folder => format!("file:{}", entry.target).into(),
        _ => format!("dir:{id}").into(),
    };
    let mark = row.and_then(|r| r.mark);
    let label = match (mark, entry.and_then(|e| snapshot.overview.file(&e.target))) {
        (_, Some(file)) => format!("{} · {}", item.item().label, state_title(file)),
        _ => item.item().label.to_string(),
    };
    let depth = item.depth() as f32;

    ListItem::new(element_id)
        .h(rems(1.75))
        .py_0()
        .pl(px(8. + depth * 14.))
        .pr_3()
        .w_full()
        .min_w_0()
        .selected(selected)
        .accessibility_label(label)
        .on_click(move |event, window, cx| {
            let id = id.clone();
            let clicks = event.click_count();
            let _ = owner.update(cx, |this, cx| this.clicked(id, clicks, window, cx));
        })
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .gap_1p5()
                .child(div().w(px(14.)).flex_none().when(item.is_folder(), |this| {
                    this.child(
                        Icon::new(if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .xsmall()
                        .text_color(cx.theme().muted_foreground),
                    )
                }))
                .child(Icon::new(icon).small().text_color(if folder {
                    cx.theme().primary.opacity(0.75)
                } else {
                    cx.theme().muted_foreground
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .truncate()
                        .child(item.item().label.clone()),
                )
                .when(entry.is_some_and(|e| e.attributes.template), |this| {
                    this.child(
                        Icon::new(Lucide::Braces)
                            .xsmall()
                            .text_color(cx.theme().muted_foreground),
                    )
                })
                .when(
                    entry.is_some_and(|e| e.attributes.encrypted && e.kind != EntryKind::File),
                    |this| {
                        this.child(
                            Icon::new(Lucide::Lock)
                                .xsmall()
                                .text_color(cx.theme().muted_foreground),
                        )
                    },
                )
                .when(folder && !expanded, |this| {
                    let files = row.map_or(0, |r| r.files);
                    this.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(files.to_string()),
                    )
                })
                .when_some(mark, |this, mark| {
                    this.child(
                        div()
                            .flex_none()
                            .size(px(7.))
                            .rounded_full()
                            .bg(mark.color(cx)),
                    )
                }),
        )
}

fn status_tag(entry: &ManagedEntry, mark: Option<Mark>, snapshot: &Snapshot) -> Tag {
    if let Some(file) = snapshot.overview.file(&entry.target) {
        let tag = match mark {
            Some(Mark::Decide) => Tag::danger(),
            Some(Mark::ChangedHere) => Tag::warning(),
            _ => Tag::info(),
        };
        return tag.small().child(state_title(file));
    }
    match (entry.kind, mark) {
        (EntryKind::Script, Some(_)) => Tag::info().small().child(tr("Waiting to run", "等待运行")),
        (EntryKind::Script, None)
            if snapshot
                .overview
                .every_apply
                .iter()
                .any(|step| step.target == entry.target) =>
        {
            Tag::secondary()
                .small()
                .child(tr("Runs on every apply", "每次应用都运行"))
        }
        (EntryKind::Script, None) => Tag::secondary().small().child(tr("Ran", "已运行")),
        (EntryKind::Directory, Some(_)) => Tag::warning()
            .small()
            .child(tr("Has changes inside", "里面有变化")),
        _ => Tag::success().small().child(tr("In sync", "已同步")),
    }
}

fn icon_tile(icon: Lucide, cx: &App) -> AnyElement {
    div()
        .flex_none()
        .size_12()
        .rounded_xl()
        .flex()
        .items_center()
        .justify_center()
        .bg(cx.theme().muted)
        .child(
            Icon::new(icon)
                .large()
                .text_color(cx.theme().muted_foreground),
        )
        .into_any_element()
}

fn section(title: &str, content: AnyElement, cx: &App) -> AnyElement {
    v_flex()
        .gap_2()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(cx.theme().muted_foreground)
                .child(title.to_string()),
        )
        .child(content)
        .into_any_element()
}

fn centered(icon: Lucide, title: String, description: String, cx: &App) -> AnyElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_3()
        .p_8()
        .child(icon_tile(icon, cx))
        .child(
            div()
                .text_base()
                .font_weight(FontWeight::MEDIUM)
                .child(title),
        )
        .child(
            div()
                .max_w_96()
                .text_sm()
                .text_center()
                .text_color(cx.theme().muted_foreground)
                .child(description),
        )
        .into_any_element()
}

fn empty_details(cx: &App) -> AnyElement {
    centered(
        Lucide::FolderTree,
        tr("Select a file", "选择一个文件").to_string(),
        tr(
            "See where it comes from. Double-click to open its contents.",
            "查看它从哪里来。双击可以打开内容。",
        )
        .to_string(),
        cx,
    )
}

fn scripts_details(count: usize, cx: &App) -> AnyElement {
    centered(
        Lucide::SquareTerminal,
        tr(format!("{count} scripts"), format!("{count} 个脚本")),
        tr(
            "Scripts are not files in your home directory: chezmoi runs them when it applies. run_once_ scripts run once on each computer, run_onchange_ scripts whenever they change, the others every time.",
            "脚本不是家目录里的文件，chezmoi 应用时运行它们：run_once_ 在每台电脑上只运行一次，run_onchange_ 内容变化时运行，其余每次都运行。",
        )
        .to_string(),
        cx,
    )
}

fn folder_details(
    id: &str,
    files: usize,
    mark: Option<Mark>,
    snapshot: &Snapshot,
    cx: &App,
) -> AnyElement {
    let changed = if mark.is_some() {
        let prefix = format!("{id}/");
        let n = snapshot
            .overview
            .files
            .iter()
            .filter(|f| f.target.starts_with(&prefix))
            .count();
        tr(
            format!(" {n} of them changed."),
            format!("其中 {n} 个有变化。"),
        )
    } else {
        String::new()
    };
    centered(
        Lucide::Folder,
        format!("~/{id}"),
        tr(
            format!("{files} managed files inside.{changed}"),
            format!("里面有 {files} 个托管文件。{changed}"),
        ),
        cx,
    )
}

impl Render for FilesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let Some(snapshot) = model.snapshot().cloned() else {
            return match model.error() {
                Some(error) => error_state(error.to_string(), cx),
                None => loading_state(cx),
            };
        };
        let rem = window.rem_size();
        let right = if self.viewing {
            div()
                .size_full()
                .child(self.viewer.clone())
                .into_any_element()
        } else {
            self.render_details(&snapshot, cx)
        };
        h_resizable("files-panes")
            .child(
                resizable_panel()
                    .size(rem * 20.)
                    .size_range(rem * 14. ..rem * 40.)
                    .child(self.render_list(&snapshot, cx)),
            )
            .child(
                resizable_panel()
                    .size_range(rem * 24. ..rem * 400.)
                    .child(right),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chezmate_core::classify;

    fn entry(target: &str, source: &str) -> ManagedEntry {
        let directory = !source.contains('.') && !source.starts_with("run_");
        let (kind, attributes) = classify(source.rsplit('/').next().unwrap(), directory);
        ManagedEntry {
            target: target.into(),
            absolute: format!("/h/{target}"),
            source_absolute: format!("/s/{source}"),
            source_relative: source.into(),
            kind,
            attributes,
        }
    }

    fn file(target: &str, source: &str) -> ManagedEntry {
        let (kind, attributes) = classify(source.rsplit('/').next().unwrap(), false);
        ManagedEntry {
            kind,
            attributes,
            ..entry(target, source)
        }
    }

    fn outline(nodes: &[Node], depth: usize, out: &mut Vec<String>) {
        for node in nodes {
            out.push(format!("{}{}", "  ".repeat(depth), node.name));
            outline(&node.children, depth + 1, out);
        }
    }

    #[test]
    fn targets_are_laid_out_as_folders() {
        let entries = vec![
            file(".zshrc", "dot_zshrc"),
            entry(".config", "dot_config"),
            entry(".config/nvim", "dot_config/nvim"),
            file(".config/nvim/init.lua", "dot_config/nvim/init.lua.tmpl"),
            file(
                ".config/fish/config.fish",
                "dot_config/private_fish/config.fish",
            ),
            file("Brewfile", "Brewfile"),
            file("10-setup.sh", "run_once_10-setup.sh"),
            file(".bashrc", "dot_bashrc"),
        ];
        let mut lines = Vec::new();
        outline(&build_tree(&entries, ""), 0, &mut lines);
        assert_eq!(
            lines,
            [
                ".config",
                "  fish",
                "    config.fish",
                "  nvim",
                "    init.lua",
                ".bashrc",
                "Brewfile",
                ".zshrc",
                tr("Scripts", "脚本"),
                "  10-setup.sh",
            ]
        );
        let tree = build_tree(&entries, "");
        // Folders chezmoi lists point at their entry; `fish` it does not.
        assert_eq!(tree[0].entry, Some(1));
        assert_eq!(tree[0].children[0].entry, None);
        assert_eq!(tree[0].children[0].id, ".config/fish");
        assert_eq!(tree[4].children[0].id, "scripts:10-setup.sh");

        let mut lines = Vec::new();
        outline(&build_tree(&entries, "NVIM"), 0, &mut lines);
        assert_eq!(lines, [".config", "  nvim", "    init.lua"]);
    }

    #[test]
    fn filter_counts_files_not_plain_folders() {
        let mut entries = vec![
            file(".zshrc", "dot_zshrc"),
            file(".config/nvim/init.lua", "dot_config/nvim/init.lua.tmpl"),
            entry(".config", "dot_config"),
        ];
        let (kind, attributes) = classify("private_dot_ssh", true);
        entries.push(ManagedEntry {
            kind,
            attributes,
            ..entry(".ssh", "private_dot_ssh")
        });
        assert_eq!(filter_entries(&entries, ""), vec![0, 1, 3]);
        assert_eq!(filter_entries(&entries, "NVIM"), vec![1]);
        assert_eq!(filter_entries(&entries, "tmpl"), vec![1]);
        assert!(filter_entries(&entries, "fish").is_empty());
    }

    #[test]
    fn details_read_naturally() {
        assert_eq!(permissions(0o100644), "rw-r--r-- (644)");
        assert_eq!(permissions(0o40700), "rwx------ (700)");
        let hour_ago = SystemTime::now() - std::time::Duration::from_secs(3 * 3600 + 5);
        assert_eq!(ago(hour_ago), tr("3 hours ago", "3 小时前"));
        let e = file(".netrc", "encrypted_private_dot_netrc.tmpl.age");
        assert_eq!(
            attribute_labels(&e),
            vec![
                tr("Template", "模板"),
                tr("Encrypted", "加密"),
                tr("Private", "私有")
            ]
        );
    }
}
