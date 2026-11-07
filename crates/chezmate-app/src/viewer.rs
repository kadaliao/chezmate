//! The file viewer in the lower half of the Files page details: a file's
//! contents on this computer, its source file, and its pending changes.
//! Read-only until someone chooses to edit.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chezmate_core::{EntryKind, ManagedEntry};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    diff::{Diff, DiffMode, DiffState},
    h_flex,
    input::{Editor, EditorState, InputEvent},
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, IntoElement, ObjectFit,
    ParentElement as _, Render, Styled as _, StyledImage as _, Subscription, Window, div, img,
    prelude::FluentBuilder as _,
};

use crate::i18n::tr;
use crate::language::{language_for, language_label};
use crate::model::{Dotfiles, Snapshot};
use crate::overview::diff_caption;

/// Text files larger than this are not loaded into the editor.
const MAX_TEXT: u64 = 1024 * 1024;

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"];

/// What a file holds, as far as showing it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    Text(String),
    /// A binary property list, converted to XML for reading only.
    Plist(String),
    Image(PathBuf, u64),
    Binary(u64),
    TooLarge(u64),
    Missing,
    Link(PathBuf),
    /// The source file is encrypted; its ciphertext is not worth showing.
    Encrypted,
    /// The decrypted file on this computer, shown only when asked: a click
    /// on a file should not put a secret on screen.
    Hidden,
    Folder,
    Failed(String),
}

/// Reads `path` for display, without following a symlink.
pub fn read_content(path: &Path) -> Content {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => return Content::Missing,
        Err(err) => return Content::Failed(err.to_string()),
    };
    if meta.file_type().is_symlink() {
        return Content::Link(fs::read_link(path).unwrap_or_default());
    }
    if meta.is_dir() {
        return Content::Folder;
    }
    let size = meta.len();
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase);
    if extension.is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.as_str())) {
        return Content::Image(path.to_path_buf(), size);
    }
    if size > MAX_TEXT {
        return Content::TooLarge(size);
    }
    match fs::read(path) {
        Ok(bytes) if bytes.starts_with(b"bplist") => match plist_as_xml(path) {
            Some(xml) => Content::Plist(xml),
            None => Content::Binary(size),
        },
        Ok(bytes) if bytes[..bytes.len().min(8000)].contains(&0) => Content::Binary(size),
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Content::Text(text),
            Err(_) => Content::Binary(size),
        },
        Err(err) => Content::Failed(err.to_string()),
    }
}

/// macOS's `plutil` turns a binary property list into XML. It only reads
/// the file.
fn plist_as_xml(path: &Path) -> Option<String> {
    let output = std::process::Command::new("/usr/bin/plutil")
        .args(["-convert", "xml1", "-o", "-"])
        .arg(path)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewTab {
    /// The file in the home directory.
    Here,
    /// The file in the source directory.
    Source,
    /// What a sync would change.
    Diff,
}

impl ViewTab {
    fn label(self) -> &'static str {
        match self {
            ViewTab::Here => tr("This Computer", "这台电脑"),
            ViewTab::Source => tr("Source File", "源文件"),
            ViewTab::Diff => tr("Changes", "差异"),
        }
    }
}

/// The tabs worth showing for an entry, most useful first.
pub fn tabs_for(entry: &ManagedEntry, has_diff: bool) -> Vec<ViewTab> {
    let mut tabs = Vec::new();
    if has_diff {
        tabs.push(ViewTab::Diff);
    }
    if matches!(
        entry.kind,
        EntryKind::File | EntryKind::Symlink | EntryKind::Modify | EntryKind::Remove
    ) {
        tabs.push(ViewTab::Here);
    }
    if matches!(
        entry.kind,
        EntryKind::File | EntryKind::Symlink | EntryKind::Script | EntryKind::Modify
    ) {
        tabs.push(ViewTab::Source);
    }
    tabs
}

pub struct FileViewer {
    model: Entity<Dotfiles>,
    target: String,
    tabs: Vec<ViewTab>,
    tab: ViewTab,
    content: Content,
    language: String,
    editor: Entity<EditorState>,
    /// The text as loaded, to tell whether an edit changed anything.
    original: String,
    editing: bool,
    dirty: bool,
    diff: Entity<DiffState>,
    /// The snapshot the current tab was loaded from.
    loaded_for: Option<usize>,
    /// The contents of an encrypted file were asked for.
    revealed: bool,
    _subscriptions: Vec<Subscription>,
}

impl FileViewer {
    pub fn new(model: Entity<Dotfiles>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .line_number(true)
                .searchable(true)
        });
        let diff = cx.new(|cx| DiffState::new([], cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &editor,
                window,
                |this, editor, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) && this.editing {
                        this.dirty = editor.read(cx).value().as_ref() != this.original;
                        cx.notify();
                    }
                },
            ),
            cx.observe(&diff, |_, _, cx| cx.notify()),
        ];
        Self {
            model,
            target: String::new(),
            tabs: Vec::new(),
            tab: ViewTab::Here,
            content: Content::Missing,
            language: String::new(),
            editor,
            original: String::new(),
            editing: false,
            dirty: false,
            diff,
            loaded_for: None,
            revealed: false,
            _subscriptions: subscriptions,
        }
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn tab(&self) -> ViewTab {
        self.tab
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn tabs(&self) -> &[ViewTab] {
        &self.tabs
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn content(&self) -> &Content {
        &self.content
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn text(&self, cx: &App) -> String {
        self.editor.read(cx).value().to_string()
    }

    pub fn is_editing(&self) -> bool {
        self.editing
    }

    pub fn has_unsaved_changes(&self) -> bool {
        self.editing && self.dirty
    }

    fn snapshot(&self, cx: &App) -> Option<Arc<Snapshot>> {
        self.model.read(cx).snapshot().cloned()
    }

    /// Shows `target`, starting on its most useful tab.
    pub fn open(&mut self, target: String, window: &mut Window, cx: &mut Context<Self>) {
        self.target = target;
        self.editing = false;
        self.dirty = false;
        self.revealed = false;
        self.tabs.clear();
        self.load(true, window, cx);
    }

    /// Shows the decrypted contents of an encrypted file.
    pub fn reveal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.revealed = true;
        self.load(false, window, cx);
    }

    /// Reads the file again after a refresh, unless it is being edited.
    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.snapshot(cx).map(|s| Arc::as_ptr(&s) as usize);
        if !self.editing && current != self.loaded_for {
            self.load(false, window, cx);
        }
    }

    pub fn set_tab(&mut self, tab: ViewTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing || !self.tabs.contains(&tab) {
            return;
        }
        self.tab = tab;
        self.load(false, window, cx);
    }

    fn load(&mut self, reset_tab: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot(cx) else {
            return;
        };
        let Some(entry) = snapshot.managed_entry(&self.target).cloned() else {
            return;
        };
        self.loaded_for = Some(Arc::as_ptr(&snapshot) as usize);
        let diff = snapshot.review_diff(&self.target).cloned();
        let tab = self.tab;
        self.tabs = tabs_for(&entry, diff.is_some());
        if reset_tab || !self.tabs.contains(&self.tab) {
            self.tab = self.tabs.first().copied().unwrap_or(ViewTab::Here);
        }
        let content = match self.tab {
            ViewTab::Diff => Content::Missing,
            ViewTab::Here if entry.attributes.encrypted && !self.revealed => {
                match fs::symlink_metadata(&entry.absolute) {
                    Ok(_) => Content::Hidden,
                    Err(_) => Content::Missing,
                }
            }
            ViewTab::Here => read_content(Path::new(&entry.absolute)),
            ViewTab::Source if entry.attributes.encrypted => Content::Encrypted,
            ViewTab::Source => read_content(Path::new(&entry.source_absolute)),
        };
        let files: Vec<_> = diff.into_iter().collect();
        self.diff.update(cx, |state, cx| state.set_files(files, cx));
        // The same text stays put, scrolled where it was.
        if !reset_tab && tab == self.tab && content == self.content {
            cx.notify();
            return;
        }
        self.content = content;
        if let Content::Text(text) | Content::Plist(text) = &self.content {
            let first_line = text.lines().next().unwrap_or_default();
            self.language = if matches!(self.content, Content::Plist(_)) {
                "xml".to_string()
            } else if entry.kind == EntryKind::Symlink {
                "text".to_string()
            } else {
                language_for(&self.target, first_line)
            };
            self.original = text.clone();
            let (language, text) = (self.language.clone(), text.clone());
            self.editor.update(cx, |editor, cx| {
                if editor.language_name() != language.as_str() {
                    editor.set_highlighter(language, cx);
                }
                // After a save the editor already shows this text.
                if editor.value().as_ref() != text {
                    editor.set_value(text, window, cx);
                }
            });
        }
        cx.notify();
    }

    /// Why the current tab cannot be edited, or `None` if it can.
    fn edit_blocker(&self, entry: &ManagedEntry) -> Option<&'static str> {
        match self.tab {
            ViewTab::Here if entry.attributes.template => Some(tr(
                "Made from a template: edit the source file instead.",
                "由模板生成：请在「源文件」里编辑。",
            )),
            ViewTab::Here if entry.attributes.readonly => Some(tr(
                "Read-only on this computer: edit the source file instead.",
                "这台电脑上是只读文件：请在「源文件」里编辑。",
            )),
            ViewTab::Here if entry.kind != EntryKind::File => Some(tr(
                "Only regular files can be edited here.",
                "这里只能编辑普通文件。",
            )),
            ViewTab::Source if entry.kind == EntryKind::Symlink => Some(tr(
                "Only regular files can be edited here.",
                "这里只能编辑普通文件。",
            )),
            _ => None,
        }
    }

    fn start_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = true;
        self.dirty = false;
        self.editor
            .update(cx, |editor, cx| editor.focus(window, cx));
        cx.notify();
    }

    pub fn cancel_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let original = self.original.clone();
        self.editor
            .update(cx, |editor, cx| editor.set_value(original, window, cx));
        self.editing = false;
        self.dirty = false;
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        // Another operation is running; it would drop this one.
        if self.model.read(cx).busy().is_some() {
            return;
        }
        let Some(entry) = self
            .snapshot(cx)
            .and_then(|s| s.managed_entry(&self.target).cloned())
        else {
            return;
        };
        let (path, shown) = match self.tab {
            ViewTab::Here => (PathBuf::from(&entry.absolute), format!("~/{}", self.target)),
            ViewTab::Source => (
                PathBuf::from(&entry.source_absolute),
                entry.source_relative.clone(),
            ),
            ViewTab::Diff => return,
        };
        let text = self.editor.read(cx).value().to_string();
        self.original = text.clone();
        self.editing = false;
        self.dirty = false;
        // What the file should now hold. The refresh after the save reads
        // it back: if the write failed, the editor shows the file as it is.
        self.content = Content::Text(text.clone());
        self.loaded_for = None;
        self.model
            .update(cx, |model, cx| model.save_file(path, text, shown, cx));
        cx.notify();
    }

    /// Edit, or Cancel and Save while editing; the layout switch on the
    /// Changes tab.
    fn render_actions(&self, entry: &ManagedEntry, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.model.read(cx).busy().is_some();
        let text_tab = matches!(self.tab, ViewTab::Here | ViewTab::Source)
            && matches!(self.content, Content::Text(_));
        if self.editing {
            h_flex()
                .gap_1()
                .child(
                    Button::new("cancel-edit")
                        .ghost()
                        .xsmall()
                        .label(tr("Cancel", "取消"))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.cancel_editing(window, cx)),
                        ),
                )
                .child(
                    Button::new("save-file")
                        .primary()
                        .xsmall()
                        .icon(Icon::new(Lucide::Save))
                        .label(tr("Save", "保存"))
                        .disabled(!self.dirty || busy)
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                )
                .into_any_element()
        } else if text_tab {
            let blocker = self.edit_blocker(entry);
            Button::new("edit-file")
                .ghost()
                .xsmall()
                .icon(Icon::new(Lucide::Pencil))
                .label(tr("Edit", "编辑"))
                .disabled(blocker.is_some() || busy)
                .when_some(blocker, |button, why| button.tooltip(why))
                .on_click(cx.listener(|this, _, window, cx| this.start_editing(window, cx)))
                .into_any_element()
        } else if self.tab == ViewTab::Diff {
            let split = self.diff.read(cx).mode() == DiffMode::Split;
            Button::new("viewer-diff-mode")
                .ghost()
                .xsmall()
                .label(if split {
                    tr("Unified", "合并显示")
                } else {
                    tr("Side by Side", "左右对照")
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    let mode = if split {
                        DiffMode::Unified
                    } else {
                        DiffMode::Split
                    };
                    this.diff.update(cx, |state, cx| state.set_mode(mode, cx));
                }))
                .into_any_element()
        } else {
            div().into_any_element()
        }
    }

    fn render_tabs(&self, entry: &ManagedEntry, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.tabs.iter().position(|t| *t == self.tab).unwrap_or(0);
        let info = match &self.content {
            Content::Text(text) if self.tab != ViewTab::Diff => {
                let lines = text.lines().count();
                let lines = tr(format!("{lines} lines"), format!("{lines} 行"));
                match language_label(&self.language) {
                    Some(language) => format!("{language} · {lines}"),
                    None => lines,
                }
            }
            Content::Plist(_) if self.tab != ViewTab::Diff => tr(
                "Binary property list, shown as XML",
                "二进制属性列表，以 XML 显示",
            )
            .to_string(),
            _ => String::new(),
        };
        let editing = self.editing;
        TabBar::new("viewer-tabs")
            .underline()
            .px_2()
            .selected_index(selected)
            .on_click(cx.listener(move |this, ix: &usize, window, cx| {
                if let Some(tab) = this.tabs.get(*ix).copied() {
                    this.set_tab(tab, window, cx);
                }
            }))
            .children(
                self.tabs
                    .iter()
                    .map(|tab| Tab::new().label(tab.label()).disabled(editing)),
            )
            .suffix(
                h_flex()
                    .gap_3()
                    .pr_3()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(info),
                    )
                    .child(self.render_actions(entry, cx)),
            )
            .into_any_element()
    }

    /// What saving an edit will do, which colour is which in a diff, or
    /// that a source file is a template. Nothing otherwise: the details
    /// above already say where the file is.
    fn render_caption(
        &self,
        entry: &ManagedEntry,
        snapshot: &Snapshot,
        cx: &App,
    ) -> Option<AnyElement> {
        let text = match self.tab {
            ViewTab::Source if self.editing => tr(
                "Editing the source file. Once saved, this computer shows a new version; Sync applies it here and uploads it.",
                "正在编辑源文件。保存后这台电脑会显示「有新版本」，同步时会更新这台电脑并上传。",
            ),
            _ if self.editing => tr(
                "Editing the file on this computer. Once saved, it shows as changed here; Sync saves it to the shared version.",
                "正在编辑这台电脑上的文件。保存后它会显示为「这台电脑上改过」，同步时存到共享版本。",
            ),
            ViewTab::Diff => diff_caption(snapshot.overview.file(&self.target)),
            ViewTab::Source if entry.attributes.template => tr(
                "A template: each computer gets its own result.",
                "这是模板：每台电脑按它生成自己的内容。",
            ),
            _ => return None,
        };
        let editing = self.editing;
        Some(
            div()
                .flex_none()
                .px_4()
                .py_1p5()
                .text_xs()
                .truncate()
                .border_b_1()
                .border_color(cx.theme().border)
                .map(|this| {
                    if editing {
                        this.bg(cx.theme().warning.opacity(0.12))
                            .text_color(cx.theme().foreground)
                    } else {
                        this.text_color(cx.theme().muted_foreground)
                    }
                })
                .child(text)
                .into_any_element(),
        )
    }

    fn render_body(&self, entry: &ManagedEntry, cx: &mut Context<Self>) -> AnyElement {
        if self.tab == ViewTab::Diff {
            return Diff::new(&self.diff)
                .header_visible(false)
                .flex_1()
                .min_h_0()
                .w_full()
                .into_any_element();
        }
        let path = PathBuf::from(match self.tab {
            ViewTab::Source => &entry.source_absolute,
            _ => &entry.absolute,
        });
        match &self.content {
            Content::Text(_) | Content::Plist(_) => div()
                .flex_1()
                .min_h_0()
                .child(
                    Editor::new(&self.editor)
                        .readonly(!self.editing)
                        .bordered(false)
                        .size_full(),
                )
                .into_any_element(),
            Content::Image(image, size) => v_flex()
                .flex_1()
                .min_h_0()
                .p_6()
                .gap_3()
                .items_center()
                .justify_center()
                .bg(cx.theme().muted.opacity(0.4))
                .child(
                    div().flex_1().min_h_0().w_full().child(
                        img(image.clone())
                            .size_full()
                            .object_fit(ObjectFit::ScaleDown),
                    ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(human_size(*size)),
                )
                .into_any_element(),
            Content::Binary(size) => placeholder(
                Lucide::FileDigit,
                tr("Binary file", "二进制文件"),
                tr(
                    format!(
                        "{} · It cannot be shown as text. Open it with its app instead.",
                        human_size(*size)
                    ),
                    format!(
                        "{} · 不能显示为文本，可以用对应的应用打开。",
                        human_size(*size)
                    ),
                ),
                Some(path),
                cx,
            ),
            Content::TooLarge(size) => placeholder(
                Lucide::FileText,
                tr("Too large to show here", "文件太大，不在这里显示"),
                tr(
                    format!("{} · Open it with its app instead.", human_size(*size)),
                    format!("{} · 可以用对应的应用打开。", human_size(*size)),
                ),
                Some(path),
                cx,
            ),
            Content::Missing => placeholder(
                Lucide::FileQuestionMark,
                tr("Not on this computer", "这台电脑上没有这个文件"),
                tr(
                    "It appears here once it is synced.".to_string(),
                    "同步后就会出现。".to_string(),
                ),
                None,
                cx,
            ),
            Content::Link(to) => placeholder(
                Lucide::FileSymlink,
                tr("Symbolic link", "符号链接"),
                tr(
                    format!("Points to {}", to.display()),
                    format!("指向 {}", to.display()),
                ),
                None,
                cx,
            ),
            Content::Encrypted => placeholder(
                Lucide::FileLock,
                tr("The source file is encrypted", "源文件已加密"),
                tr(
                    "The shared version keeps it encrypted. See its contents under This Computer."
                        .to_string(),
                    "共享版本里保存的是加密后的内容。解密后的内容见「这台电脑」。".to_string(),
                ),
                None,
                cx,
            ),
            Content::Hidden => v_flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .gap_3()
                .p_8()
                .child(icon_tile(Lucide::FileLock, cx))
                .child(
                    div()
                        .text_base()
                        .font_weight(FontWeight::MEDIUM)
                        .child(tr("Encrypted file", "加密文件")),
                )
                .child(
                    div()
                        .max_w_96()
                        .text_sm()
                        .text_center()
                        .text_color(cx.theme().muted_foreground)
                        .child(tr(
                            "The shared version keeps it encrypted. The copy on this computer is decrypted, so it is shown only when you ask.",
                            "共享版本里保存的是加密内容。这台电脑上的是解密后的文件，点了才显示。",
                        )),
                )
                .child(
                    Button::new("show-contents")
                        .outline()
                        .small()
                        .icon(Icon::new(Lucide::Eye))
                        .label(tr("Show Contents", "显示内容"))
                        .on_click(cx.listener(|this, _, window, cx| this.reveal(window, cx))),
                )
                .into_any_element(),
            Content::Folder => placeholder(
                Lucide::Folder,
                tr("A folder", "这是一个文件夹"),
                String::new(),
                Some(path),
                cx,
            ),
            Content::Failed(err) => placeholder(
                Lucide::FileX,
                tr("Could not read the file", "读取文件失败"),
                err.clone(),
                None,
                cx,
            ),
        }
    }
}

/// A centred icon, title, explanation and, for a file on disk, buttons to
/// open it elsewhere.
fn placeholder(
    icon: Lucide,
    title: &str,
    description: String,
    path: Option<PathBuf>,
    cx: &App,
) -> AnyElement {
    v_flex()
        .flex_1()
        .min_h_0()
        .items_center()
        .justify_center()
        .gap_3()
        .p_8()
        .child(icon_tile(icon, cx))
        .child(
            div()
                .text_base()
                .font_weight(FontWeight::MEDIUM)
                .child(title.to_string()),
        )
        .when(!description.is_empty(), |this| {
            this.child(
                div()
                    .max_w_96()
                    .text_sm()
                    .text_center()
                    .text_color(cx.theme().muted_foreground)
                    .child(description),
            )
        })
        .when_some(path.filter(|p| p.exists()), |this, path| {
            let reveal = path.clone();
            this.child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("open-with-app")
                            .outline()
                            .small()
                            .label(tr("Open", "打开"))
                            .on_click(move |_, _, cx| cx.open_with_system(&path)),
                    )
                    .child(
                        Button::new("reveal-in-finder")
                            .ghost()
                            .small()
                            .label(tr("Show in Finder", "在 Finder 中显示"))
                            .on_click(move |_, _, cx| cx.reveal_path(&reveal)),
                    ),
            )
        })
        .into_any_element()
}

/// An icon on a rounded, muted square.
pub fn icon_tile(icon: Lucide, cx: &App) -> AnyElement {
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

/// The icon for a managed entry, by kind, attributes and file type.
pub fn file_icon(entry: &ManagedEntry) -> Lucide {
    match entry.kind {
        EntryKind::Directory => return Lucide::Folder,
        EntryKind::Symlink => return Lucide::FileSymlink,
        EntryKind::Script | EntryKind::Modify => return Lucide::FileTerminal,
        EntryKind::Remove => return Lucide::FileX,
        EntryKind::File => {}
    }
    if entry.attributes.encrypted {
        return Lucide::FileLock;
    }
    if entry.attributes.executable {
        return Lucide::FileTerminal;
    }
    let extension = entry
        .target
        .rsplit('/')
        .next()
        .and_then(|name| name.trim_start_matches('.').rsplit_once('.'))
        .map(|(_, e)| e.to_lowercase());
    match extension.as_deref() {
        Some(e) if IMAGE_EXTENSIONS.contains(&e) => Lucide::FileImage,
        Some("json" | "toml" | "yaml" | "yml" | "plist" | "xml" | "conf" | "ini") => {
            Lucide::FileCog
        }
        Some("sh" | "fish" | "zsh" | "bash" | "py" | "lua" | "js" | "rb") => Lucide::FileCode,
        Some("pdf") => Lucide::FileText,
        Some("pem" | "pub" | "key") => Lucide::FileKey,
        _ => Lucide::FileText,
    }
}

impl Render for FileViewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(snapshot) = self.snapshot(cx) else {
            return div().into_any_element();
        };
        let Some(entry) = snapshot.managed_entry(&self.target).cloned() else {
            return div().into_any_element();
        };
        v_flex()
            .size_full()
            .min_h_0()
            .child(self.render_tabs(&entry, cx))
            .children(self.render_caption(&entry, &snapshot, cx))
            .child(self.render_body(&entry, cx))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chezmate_core::classify;

    fn entry(target: &str, source: &str) -> ManagedEntry {
        let (kind, attributes) = classify(source.rsplit('/').next().unwrap(), false);
        ManagedEntry {
            target: target.into(),
            absolute: format!("/h/{target}"),
            source_absolute: format!("/s/{source}"),
            source_relative: source.into(),
            kind,
            attributes,
        }
    }

    #[test]
    fn content_is_sorted_into_what_can_be_shown() {
        let dir = tempfile::tempdir().unwrap();
        let path = |name: &str| dir.path().join(name);
        fs::write(path("text"), "hello\n").unwrap();
        fs::write(path("binary.plist"), b"bplist00\x00\x01").unwrap();
        fs::write(path("latin1"), [0xe9u8, 0x20, 0x61]).unwrap();
        fs::write(path("icon.PNG"), b"\x89PNG").unwrap();
        fs::write(path("big"), vec![b'a'; MAX_TEXT as usize + 1]).unwrap();
        std::os::unix::fs::symlink("text", path("link")).unwrap();

        assert_eq!(read_content(&path("text")), Content::Text("hello\n".into()));
        assert_eq!(read_content(&path("binary.plist")), Content::Binary(10));
        assert_eq!(read_content(&path("latin1")), Content::Binary(3));
        assert_eq!(
            read_content(&path("icon.PNG")),
            Content::Image(path("icon.PNG"), 4)
        );
        assert_eq!(read_content(&path("big")), Content::TooLarge(MAX_TEXT + 1));
        assert_eq!(read_content(&path("link")), Content::Link("text".into()));
        assert_eq!(read_content(&path("missing")), Content::Missing);
        assert_eq!(read_content(dir.path()), Content::Folder);

        // A real binary property list reads as XML.
        fs::write(
            path("prefs.plist"),
            "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>Greeting</key><string>hi</string></dict></plist>",
        )
        .unwrap();
        let converted = std::process::Command::new("/usr/bin/plutil")
            .args(["-convert", "binary1"])
            .arg(path("prefs.plist"))
            .status()
            .unwrap();
        assert!(converted.success());
        match read_content(&path("prefs.plist")) {
            Content::Plist(xml) => assert!(xml.contains("<key>Greeting</key>"), "{xml}"),
            other => panic!("not converted: {other:?}"),
        }
    }

    #[test]
    fn tabs_follow_the_entry() {
        use ViewTab::*;
        assert_eq!(
            tabs_for(&entry(".zshrc", "dot_zshrc"), false),
            [Here, Source]
        );
        assert_eq!(
            tabs_for(&entry(".zshrc", "dot_zshrc"), true),
            [Diff, Here, Source]
        );
        assert_eq!(
            tabs_for(&entry("setup.sh", "run_once_setup.sh"), false),
            [Source]
        );
        let (kind, attributes) = classify("private_dot_ssh", true);
        let folder = ManagedEntry {
            kind,
            attributes,
            ..entry(".ssh", "private_dot_ssh")
        };
        assert!(tabs_for(&folder, false).is_empty());
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(human_size(12), "12 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(5 * 1024 * 1024 + 1), "5.0 MB");
    }
}
