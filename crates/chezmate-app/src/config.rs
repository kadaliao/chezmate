//! The config panel on the Environment page: chezmoi's config file and the
//! template `chezmoi init` makes it from, in the same read-only editor as
//! the Files page until someone chooses to edit.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Editor, EditorState, InputEvent},
    tab::{Tab, TabBar},
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
    Styled as _, Subscription, Window, div, px,
};

use crate::i18n::tr;
use crate::language::{language_for, language_label};
use crate::model::{Dotfiles, Snapshot};
use crate::viewer::{Content, caption, human_size, placeholder, read_content};

/// Rows the editor grows to before it scrolls; the page scrolls around it.
const MIN_ROWS: usize = 8;
const MAX_ROWS: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigTab {
    /// This computer's config file.
    File,
    /// The `.chezmoi.<format>.tmpl` in the source directory.
    Template,
}

impl ConfigTab {
    fn label(self) -> &'static str {
        match self {
            ConfigTab::File => tr("This Computer", "这台电脑"),
            ConfigTab::Template => tr("Template", "模板"),
        }
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub struct ConfigPanel {
    model: Entity<Dotfiles>,
    tabs: Vec<ConfigTab>,
    tab: ConfigTab,
    /// The file the current tab shows.
    path: PathBuf,
    template: Option<PathBuf>,
    /// The config file does not exist yet; saving creates it.
    missing: bool,
    content: Content,
    language: String,
    editor: Entity<EditorState>,
    /// The text as loaded, to tell whether an edit changed anything.
    original: String,
    editing: bool,
    dirty: bool,
    /// The snapshot the current tab was loaded from.
    loaded_for: Option<usize>,
    _subscriptions: Vec<Subscription>,
}

impl ConfigPanel {
    pub fn new(model: Entity<Dotfiles>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .line_number(true)
                .searchable(true)
        });
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
            cx.observe_in(&model, window, |this, _, window, cx| {
                this.reload(window, cx)
            }),
        ];
        let mut this = Self {
            model,
            tabs: Vec::new(),
            tab: ConfigTab::File,
            path: PathBuf::new(),
            template: None,
            missing: false,
            content: Content::Missing,
            language: String::new(),
            editor,
            original: String::new(),
            editing: false,
            dirty: false,
            loaded_for: None,
            _subscriptions: subscriptions,
        };
        this.reload(window, cx);
        this
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn tabs(&self) -> &[ConfigTab] {
        &self.tabs
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn content(&self) -> &Content {
        &self.content
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_missing(&self) -> bool {
        self.missing
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_editing(&self) -> bool {
        self.editing
    }

    fn snapshot(&self, cx: &App) -> Option<Arc<Snapshot>> {
        self.model.read(cx).snapshot().cloned()
    }

    /// Reads the file again after a refresh, unless it is being edited.
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.snapshot(cx).map(|s| Arc::as_ptr(&s) as usize);
        if !self.editing && current.is_some() && current != self.loaded_for {
            self.load(window, cx);
        }
    }

    pub fn set_tab(&mut self, tab: ConfigTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing || !self.tabs.contains(&tab) {
            return;
        }
        self.tab = tab;
        self.load(window, cx);
    }

    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot(cx) else {
            return;
        };
        self.loaded_for = Some(Arc::as_ptr(&snapshot) as usize);
        let info = &snapshot.info;
        self.template = info.config_template.clone();
        self.tabs = vec![ConfigTab::File];
        if self.template.is_some() {
            self.tabs.push(ConfigTab::Template);
        }
        if !self.tabs.contains(&self.tab) {
            self.tab = ConfigTab::File;
        }
        self.path = match (self.tab, &self.template) {
            (ConfigTab::Template, Some(template)) => template.clone(),
            _ => info.paths.config_file.clone(),
        };
        let mut content = read_content(&self.path);
        self.missing = self.tab == ConfigTab::File && content == Content::Missing;
        if self.missing {
            // chezmoi runs on its defaults; an empty file is where to start.
            content = Content::Text(String::new());
        }
        self.content = content;
        if let Content::Text(text) = &self.content {
            // `.chezmoi.toml.tmpl` is TOML with template directives.
            let name = file_name(&self.path);
            let name = name.strip_suffix(".tmpl").unwrap_or(&name);
            self.language = language_for(name, text.lines().next().unwrap_or_default());
            self.original = text.clone();
            let (language, text) = (self.language.clone(), text.clone());
            self.editor.update(cx, |editor, cx| {
                if editor.language_name() != language.as_str() {
                    editor.set_highlighter(language, cx);
                }
                if editor.value().as_ref() != text {
                    editor.set_value(text, window, cx);
                }
            });
        }
        cx.notify();
    }

    fn start_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = true;
        self.dirty = false;
        self.editor
            .update(cx, |editor, cx| editor.focus(window, cx));
        cx.notify();
    }

    fn cancel_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        let text = self.editor.read(cx).value().to_string();
        self.original = text.clone();
        self.editing = false;
        self.dirty = false;
        // The refresh after the save reads the file back: if the write
        // failed, the editor shows the file as it is. Until then the file
        // on disk may still be the old one.
        self.content = Content::Text(text.clone());
        self.loaded_for = self.snapshot(cx).map(|s| Arc::as_ptr(&s) as usize);
        let (path, shown) = (self.path.clone(), file_name(&self.path));
        self.model
            .update(cx, |model, cx| model.save_file(path, text, shown, cx));
        cx.notify();
    }

    fn render_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.model.read(cx).busy().is_some();
        if self.editing {
            h_flex()
                .gap_1()
                .child(
                    Button::new("cancel-config")
                        .ghost()
                        .xsmall()
                        .label(tr("Cancel", "取消"))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.cancel_editing(window, cx)),
                        ),
                )
                .child(
                    Button::new("save-config")
                        .primary()
                        .xsmall()
                        .icon(Icon::new(Lucide::Save))
                        .label(tr("Save", "保存"))
                        .disabled(!self.dirty || busy)
                        .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                )
                .into_any_element()
        } else if matches!(self.content, Content::Text(_)) {
            Button::new("edit-config")
                .ghost()
                .xsmall()
                .icon(Icon::new(Lucide::Pencil))
                .label(tr("Edit", "编辑"))
                .disabled(busy)
                .on_click(cx.listener(|this, _, window, cx| this.start_editing(window, cx)))
                .into_any_element()
        } else {
            div().into_any_element()
        }
    }

    fn render_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.tabs.iter().position(|t| *t == self.tab).unwrap_or(0);
        let info = match &self.content {
            Content::Text(_) if self.missing => tr("Not created", "尚未创建").to_string(),
            Content::Text(text) => {
                let lines = text.lines().count();
                let lines = tr(format!("{lines} lines"), format!("{lines} 行"));
                match language_label(&self.language) {
                    Some(language) => format!("{language} · {lines}"),
                    None => lines,
                }
            }
            _ => String::new(),
        };
        let editing = self.editing;
        TabBar::new("config-tabs")
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
                    .child(self.render_actions(cx)),
            )
            .into_any_element()
    }

    /// What the file is and what an edit to it does. The details above
    /// already say where it is.
    fn caption_text(&self) -> Option<String> {
        let template = self.template.as_deref().map(file_name);
        Some(match (self.tab, template) {
            (ConfigTab::Template, _) if self.editing => tr(
                "Editing the template all computers share. Sync uploads it; chezmoi init on each computer remakes its config file from it.",
                "正在编辑所有电脑共用的模板。同步时会上传；在每台电脑上运行 chezmoi init 才会按它重新生成配置文件。",
            )
            .to_string(),
            (ConfigTab::Template, _) => tr(
                "In the source folder, shared by all computers: chezmoi init makes each computer's config file from it.",
                "在源目录里，所有电脑共用：chezmoi init 按它生成每台电脑的配置文件。",
            )
            .to_string(),
            (ConfigTab::File, Some(template)) if self.editing => tr(
                format!(
                    "Editing chezmoi's settings on this computer; they apply once saved. chezmoi init replaces them with what {template} makes."
                ),
                format!(
                    "正在编辑这台电脑上的 chezmoi 设置，保存后生效。运行 chezmoi init 时会被 {template} 生成的内容替换。"
                ),
            ),
            (ConfigTab::File, None) if self.editing => tr(
                "Editing chezmoi's settings on this computer; they apply once saved.",
                "正在编辑这台电脑上的 chezmoi 设置，保存后生效。",
            )
            .to_string(),
            (ConfigTab::File, _) if self.missing => tr(
                "Not created yet, so chezmoi uses its defaults. Edit and save to create it.",
                "还没有这个文件，chezmoi 使用默认设置。编辑并保存即可创建。",
            )
            .to_string(),
            (ConfigTab::File, Some(template)) => tr(
                format!(
                    "Made from {template} in the source folder; chezmoi init makes it again and replaces edits made here."
                ),
                format!("由源目录里的 {template} 生成；运行 chezmoi init 会重新生成，替换这里的修改。"),
            ),
            (ConfigTab::File, None) => return None,
        })
    }

    fn render_body(&self, cx: &App) -> AnyElement {
        match &self.content {
            Content::Text(text) => {
                let rows = (text.lines().count() + 1).clamp(MIN_ROWS, MAX_ROWS);
                // The editor's rows are 1.5 times its font size, plus padding.
                let height = cx.theme().mono_font_size * (1.5 * rows as f32) + px(16.);
                Editor::new(&self.editor)
                    .readonly(!self.editing)
                    .bordered(false)
                    .w_full()
                    .h(height)
                    .into_any_element()
            }
            Content::Binary(size) | Content::TooLarge(size) => placeholder(
                Lucide::FileText,
                tr("Cannot be shown here", "不能在这里显示"),
                human_size(*size),
                Some(self.path.clone()),
                cx,
            ),
            Content::Failed(err) => placeholder(
                Lucide::FileX,
                tr("Could not read the file", "读取文件失败"),
                err.clone(),
                None,
                cx,
            ),
            _ => placeholder(
                Lucide::FileQuestionMark,
                tr("The file is gone", "文件不见了"),
                self.path.display().to_string(),
                None,
                cx,
            ),
        }
    }
}

impl Render for ConfigPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.loaded_for.is_none() && self.tabs.is_empty() {
            return div().into_any_element();
        }
        let icon = (self.tab == ConfigTab::Template).then_some(Lucide::Braces);
        v_flex()
            .w_full()
            .min_h(px(160.))
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius_lg)
            .overflow_hidden()
            .child(self.render_tabs(cx))
            .children(
                self.caption_text()
                    .map(|text| caption(icon, &text, self.editing, cx)),
            )
            .child(self.render_body(cx))
            .into_any_element()
    }
}
