//! Getting started: what someone sees before the Overview makes sense.
//! `SetupView` covers a computer without chezmoi or without a shared
//! version, and a config template's questions; `GetStarted` picks the first
//! files to sync; `ConnectView` uploads the shared version to GitHub.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use chezmate_core::{ConfigAnswer, ConfigQuestion, QuestionKind, Suggestion, Topic};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _,
    alert::Alert,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    clipboard::Clipboard,
    h_flex,
    input::{Input, InputEvent, InputState},
    radio::{Radio, RadioGroup},
    scroll::ScrollableElement as _,
    spinner::Spinner,
    switch::Switch,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _,
    Subscription, Task, Window, div, prelude::FluentBuilder as _, px,
};

use crate::i18n::tr;
use crate::model::{Dotfiles, GitHubState, Setup};
use crate::viewer::icon_tile;

const INSTALL_COMMAND: &str = "brew install chezmoi";
const INSTALL_URL: &str = "https://www.chezmoi.io/install/";
const RELEASES_URL: &str = "https://github.com/kadaliao/chezmate/releases/latest";
const NEW_REPOSITORY_URL: &str = "https://github.com/new";

/// The answer field for one config question.
enum Field {
    Text(Entity<InputState>),
    YesNo(bool),
    Choice(usize),
    Several(BTreeSet<usize>),
}

pub struct SetupView {
    model: Entity<Dotfiles>,
    repo: Entity<InputState>,
    /// The questions the fields below answer, in order.
    questions: Vec<ConfigQuestion>,
    fields: Vec<Field>,
    _subscriptions: Vec<Subscription>,
}

impl SetupView {
    pub fn new(model: Entity<Dotfiles>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let repo = cx.new(|cx| {
            InputState::new(window, cx).placeholder(tr(
                "GitHub user name, or the repository's address",
                "GitHub 用户名，或仓库地址",
            ))
        });
        let subscriptions = vec![
            cx.observe_in(&model, window, |this, _, window, cx| {
                this.sync_questions(window, cx);
                cx.notify();
            }),
            cx.subscribe_in(
                &repo,
                window,
                |this, _, event: &InputEvent, _, cx| match event {
                    InputEvent::Change => cx.notify(),
                    InputEvent::PressEnter { .. } => this.start_from(cx),
                    _ => {}
                },
            ),
        ];
        let mut this = Self {
            model,
            repo,
            questions: Vec::new(),
            fields: Vec::new(),
            _subscriptions: subscriptions,
        };
        this.sync_questions(window, cx);
        this
    }

    /// Builds a field for each question, keeping answers already given to
    /// questions that are still asked.
    fn sync_questions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let questions = match self.model.read(cx).setup() {
            Some(Setup::Questions(questions)) => questions.clone(),
            _ => Vec::new(),
        };
        if questions == self.questions {
            return;
        }
        let mut old: Vec<(ConfigQuestion, Field)> = self
            .questions
            .drain(..)
            .zip(self.fields.drain(..))
            .collect();
        for question in &questions {
            let kept = old
                .iter()
                .position(|(q, _)| q == question)
                .map(|ix| old.remove(ix).1);
            let field = kept.unwrap_or_else(|| new_field(question, window, cx));
            self.fields.push(field);
        }
        self.questions = questions;
    }

    fn start_from(&mut self, cx: &mut Context<Self>) {
        let repo = self.repo.read(cx).value().trim().to_string();
        if repo.is_empty() || self.model.read(cx).busy().is_some() {
            return;
        }
        self.model
            .update(cx, |model, cx| model.start_from(repo, cx));
    }

    /// The answers as they stand; `None` while a number is not a number.
    fn answers(&self, cx: &App) -> Option<Vec<ConfigAnswer>> {
        self.questions
            .iter()
            .zip(&self.fields)
            .map(|(question, field)| {
                let value = match field {
                    Field::Text(input) => {
                        let value = input.read(cx).value().trim().to_string();
                        if question.kind == QuestionKind::Number && value.parse::<i64>().is_err() {
                            return None;
                        }
                        value
                    }
                    Field::YesNo(yes) => yes.to_string(),
                    Field::Choice(ix) => question.choices.get(*ix)?.clone(),
                    Field::Several(picked) => picked
                        .iter()
                        .filter_map(|ix| question.choices.get(*ix).cloned())
                        .collect::<Vec<_>>()
                        .join("/"),
                };
                Some(ConfigAnswer {
                    kind: question.kind,
                    prompt: question.prompt.clone(),
                    value,
                })
            })
            .collect()
    }

    fn render_no_chezmoi(&self, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        page(
            Lucide::SquareTerminal,
            tr("Chezmate needs chezmoi", "需要 chezmoi"),
            tr(
                "Chezmate keeps your settings files in sync with chezmoi. This copy of Chezmate does not include chezmoi (as when it runs from source), and none is installed on this computer. Install it with Homebrew:",
                "Chezmate 在后台用 chezmoi 同步配置文件。这个版本的 Chezmate 没有自带 chezmoi（例如直接从源码运行时），这台电脑上也没有安装。可以用 Homebrew 安装：",
            ),
            cx,
        )
        .child(
            h_flex()
                .gap_2()
                .px_3()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().muted)
                .child(
                    div()
                        .flex_1()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_sm()
                        .child(INSTALL_COMMAND),
                )
                .child(Clipboard::new("copy-install").value(INSTALL_COMMAND)),
        )
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(tr(
                    "Other ways to install it are on chezmoi's website. Or download Chezmate from its releases page: that version includes chezmoi. Check again once it is installed.",
                    "其他安装方式见 chezmoi 官网。也可以从发布页重新下载 Chezmate，发布版自带 chezmoi。装好后点「重新检查」。",
                )),
        )
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("recheck")
                        .primary()
                        .label(tr("Check Again", "重新检查"))
                        .loading(busy)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model.update(cx, |model, cx| model.refresh(cx));
                        })),
                )
                .child(
                    Button::new("install-help")
                        .ghost()
                        .icon(Icon::new(Lucide::ExternalLink))
                        .label(tr("Install Guide", "安装说明"))
                        .on_click(|_, _, cx| cx.open_url(INSTALL_URL)),
                )
                .child(
                    Button::new("download-chezmate")
                        .ghost()
                        .icon(Icon::new(Lucide::ExternalLink))
                        .label(tr("Download Chezmate", "下载 Chezmate"))
                        .on_click(|_, _, cx| cx.open_url(RELEASES_URL)),
                ),
        )
        .into_any_element()
    }

    fn render_start(&self, source_dir: &Path, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let has_repo = !self.repo.read(cx).value().trim().is_empty();
        let card = |icon: Lucide, title: &'static str, text: &'static str, cx: &App| {
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_3()
                .p_5()
                .rounded_lg()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().background)
                .child(
                    h_flex()
                        .gap_2()
                        .child(Icon::new(icon).text_color(cx.theme().primary))
                        .child(
                            div()
                                .text_base()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        ),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(text),
                )
        };
        let first = card(
            Lucide::Laptop,
            tr("This is my first computer", "这是我的第一台电脑"),
            tr(
                "Start from the settings files already on this computer: pick the ones to sync, then upload them to GitHub so your other computers can get them.",
                "从这台电脑上已有的配置文件开始：挑选要同步的文件，再上传到 GitHub，其他电脑就能取回。",
            ),
            cx,
        )
        .child(div().flex_1())
        .child(
            Button::new("start-new")
                .primary()
                .label(tr("Start", "开始"))
                .loading(busy)
                .disabled(busy)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.model.update(cx, |model, cx| model.start_new(cx));
                })),
        );
        let other = card(
            Lucide::CloudDownload,
            tr(
                "I use it on another computer",
                "我已经在别的电脑上用过",
            ),
            tr(
                "Get the shared version from GitHub or another server. Nothing on this computer changes until you sync, and you can check every file first.",
                "从 GitHub 等处取回共享版本。同步之前，这台电脑上的文件不会改动，可以逐个检查。",
            ),
            cx,
        )
        .child(Input::new(&self.repo).id("repo-input").disabled(busy))
        .child(
            Button::new("start-from")
                .outline()
                .label(tr("Get Shared Version", "取回共享版本"))
                .loading(busy)
                .disabled(busy || !has_repo)
                .on_click(cx.listener(|this, _, _, cx| this.start_from(cx))),
        );
        page(
            Lucide::ListChecks,
            tr("Welcome to Chezmate", "欢迎使用 Chezmate"),
            tr(
                "Chezmate keeps settings files such as .zshrc and .gitconfig the same on all your computers. It saves them as a shared version and shows what differs before changing anything. How does this computer start?",
                "Chezmate 让 .zshrc、.gitconfig 这类配置文件在你的几台电脑上保持一致：它把这些文件保存成一份「共享版本」，改动前先告诉你哪里不同。这台电脑从哪里开始？",
            ),
            cx,
        )
        .max_w(px(820.))
        .child(h_flex().gap_4().items_stretch().child(first).child(other))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(tr(
                    format!("The shared version is kept in {}.", source_dir.display()),
                    format!("共享版本保存在 {}。", source_dir.display()),
                )),
        )
        .into_any_element()
    }

    fn render_questions(&self, busy: bool, cx: &mut Context<Self>) -> AnyElement {
        let answers = self.answers(cx);
        let mut form = v_flex().gap_4();
        for (ix, (question, field)) in self.questions.iter().zip(&self.fields).enumerate() {
            let id = |part: &str| SharedString::from(format!("question-{ix}-{part}"));
            let control: AnyElement = match field {
                Field::Text(input) => Input::new(input)
                    .id(id("input"))
                    .disabled(busy)
                    .into_any_element(),
                Field::YesNo(yes) => Switch::new(id("switch"))
                    .checked(*yes)
                    .label(if *yes {
                        tr("Yes", "是")
                    } else {
                        tr("No", "否")
                    })
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        if let Some(Field::YesNo(yes)) = this.fields.get_mut(ix) {
                            *yes = *checked;
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
                Field::Choice(picked) => RadioGroup::vertical(id("choice"))
                    .selected_index(Some(*picked))
                    .children(
                        question
                            .choices
                            .iter()
                            .map(|c| Radio::new(c.clone()).label(c.clone())),
                    )
                    .on_click(cx.listener(move |this, picked: &usize, _, cx| {
                        if let Some(Field::Choice(current)) = this.fields.get_mut(ix) {
                            *current = *picked;
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
                Field::Several(picked) => v_flex()
                    .gap_1()
                    .children(question.choices.iter().enumerate().map(|(choice, label)| {
                        Checkbox::new(SharedString::from(format!("question-{ix}-{choice}")))
                            .label(label.clone())
                            .checked(picked.contains(&choice))
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                if let Some(Field::Several(picked)) = this.fields.get_mut(ix) {
                                    if *checked {
                                        picked.insert(choice);
                                    } else {
                                        picked.remove(&choice);
                                    }
                                }
                                cx.notify();
                            }))
                    }))
                    .into_any_element(),
            };
            form = form.child(
                v_flex()
                    .gap_1p5()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(question.prompt.clone()),
                    )
                    .child(control),
            );
        }
        let description = if self.questions.is_empty() {
            tr(
                "The shared version makes this computer's chezmoi settings from a template. Continue to create them.",
                "共享版本会用模板生成这台电脑的 chezmoi 设置，点「继续」生成。",
            )
        } else {
            tr(
                "The shared version makes this computer's chezmoi settings from a template, which needs these answers (for example the email address Git uses here).",
                "共享版本会用模板生成这台电脑的 chezmoi 设置，模板需要知道下面这些信息（例如这台电脑上 Git 用的邮箱）。",
            )
        };
        page(
            Lucide::Settings2,
            tr("A few questions", "还差几个设置"),
            description,
            cx,
        )
        .child(form)
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("answer")
                        .primary()
                        .label(tr("Continue", "继续"))
                        .loading(busy)
                        .disabled(busy || answers.is_none())
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(answers) = this.answers(cx) {
                                this.model
                                    .update(cx, |model, cx| model.answer_config(answers, cx));
                            }
                        })),
                )
                .child(
                    Button::new("skip-config")
                        .ghost()
                        .label(tr("Skip", "跳过"))
                        .disabled(busy)
                        .tooltip(tr(
                            "Start with empty settings; edit them later under Advanced › Environment",
                            "先用空的设置，之后可以在「高级 › 环境」里修改",
                        ))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.model.update(cx, |model, cx| model.skip_config(cx));
                        })),
                ),
        )
        .into_any_element()
    }
}

fn new_field(question: &ConfigQuestion, window: &mut Window, cx: &mut Context<SetupView>) -> Field {
    let default = question.default.clone().unwrap_or_default();
    match question.kind {
        QuestionKind::YesNo => Field::YesNo(default == "true"),
        QuestionKind::Choice if !question.choices.is_empty() => Field::Choice(
            question
                .choices
                .iter()
                .position(|c| *c == default)
                .unwrap_or(0),
        ),
        QuestionKind::MultipleChoice if !question.choices.is_empty() => {
            Field::Several(BTreeSet::new())
        }
        _ => {
            let input = cx.new(|cx| {
                let state = InputState::new(window, cx).default_value(default);
                match question.kind {
                    QuestionKind::Number => state.placeholder(tr("A number", "一个数字")),
                    QuestionKind::MultipleChoice => {
                        state.placeholder(tr("Separate choices with /", "多个选项用 / 分隔"))
                    }
                    _ => state,
                }
            });
            cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify())
                .detach();
            Field::Text(input)
        }
    }
}

/// A centered column with an icon, a title and a description; the caller
/// adds the rest.
fn page(icon: Lucide, title: &str, description: &str, cx: &App) -> gpui_kit::Div {
    v_flex()
        .w_full()
        .max_w(px(620.))
        .mx_auto()
        .gap_5()
        .px_8()
        .py_10()
        .child(icon_tile(icon, cx))
        .child(
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(description.to_string()),
                ),
        )
}

impl Render for SetupView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let busy = model.busy().is_some() || model.is_loading();
        let error = model.setup_error().map(str::to_string);
        let body = match model.setup().cloned() {
            Some(Setup::NoChezmoi(_)) => self.render_no_chezmoi(busy, cx),
            Some(Setup::Start { source_dir }) => self.render_start(&source_dir, busy, cx),
            Some(Setup::Questions(_)) => self.render_questions(busy, cx),
            None => div().into_any_element(),
        };
        div()
            .id("setup")
            .size_full()
            .bg(cx.theme().background)
            .child(
                v_flex()
                    .w_full()
                    .child(body)
                    .when_some(error, |this, error| {
                        this.child(
                            div()
                                .w_full()
                                .max_w(px(620.))
                                .mx_auto()
                                .px_8()
                                .pb_8()
                                .child(Alert::error("setup-error", error)),
                        )
                    }),
            )
            .overflow_y_scrollbar()
    }
}

/// The first files to sync, picked from common dotfiles on this computer.
pub struct GetStarted {
    model: Entity<Dotfiles>,
    /// Suggestions the person unchecked.
    unchecked: BTreeSet<String>,
    _pick: Task<()>,
}

impl GetStarted {
    pub fn new(model: Entity<Dotfiles>, cx: &mut Context<Self>) -> Self {
        let _ = cx;
        Self {
            model,
            unchecked: BTreeSet::new(),
            _pick: Task::ready(()),
        }
    }

    fn picked(&self, suggestions: &[Suggestion]) -> Vec<String> {
        suggestions
            .iter()
            .filter(|s| !self.unchecked.contains(&s.target))
            .map(|s| s.target.clone())
            .collect()
    }

    /// Opens the system file picker and syncs the chosen files.
    fn pick(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some(tr("Sync", "同步").into()),
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
            model.update(cx, |model, cx| model.start_syncing(paths, cx));
        });
    }
}

fn topic_title(topic: Topic) -> &'static str {
    match topic {
        Topic::Shell => tr("Shell", "Shell 终端"),
        Topic::Git => "Git",
        Topic::Editor => tr("Editors", "编辑器"),
        Topic::Terminal => tr("Terminal and windows", "终端与窗口"),
        Topic::Ssh => "SSH",
    }
}

fn describe_size(suggestion: &Suggestion) -> String {
    let size = if suggestion.bytes < 1024 {
        tr(
            format!("{} bytes", suggestion.bytes),
            format!("{} 字节", suggestion.bytes),
        )
    } else {
        format!("{:.1} KB", suggestion.bytes as f64 / 1024.0)
    };
    let mut text = if suggestion.directory {
        tr(
            format!("Folder, {} files, {size}", suggestion.files),
            format!("文件夹，{} 个文件，{size}", suggestion.files),
        )
    } else {
        size
    };
    if suggestion.topic == Topic::Ssh {
        text.push_str(tr(
            " · settings only, not your keys",
            " · 只有设置，不包括密钥",
        ));
    }
    text
}

impl Render for GetStarted {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let busy = model.busy().is_some();
        let suggestions = model
            .snapshot()
            .map(|s| s.suggestions.clone())
            .unwrap_or_default();
        let picked = self.picked(&suggestions);

        let mut list = v_flex().gap_1();
        let mut topic = None;
        for suggestion in &suggestions {
            if topic != Some(suggestion.topic) {
                topic = Some(suggestion.topic);
                list = list.child(
                    div()
                        .pt_3()
                        .pb_1()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(cx.theme().muted_foreground)
                        .child(topic_title(suggestion.topic)),
                );
            }
            let target = suggestion.target.clone();
            let checked = !self.unchecked.contains(&target);
            list = list.child(
                h_flex()
                    .gap_3()
                    .child(
                        Checkbox::new(SharedString::from(format!("suggest:{target}")))
                            .label(format!("~/{target}"))
                            .checked(checked)
                            .disabled(busy)
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                if *checked {
                                    this.unchecked.remove(&target);
                                } else {
                                    this.unchecked.insert(target.clone());
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(describe_size(suggestion)),
                    ),
            );
        }

        let description = if suggestions.is_empty() {
            tr(
                "No common settings files were found on this computer. Choose the files you want to keep the same on your computers.",
                "这台电脑上没有找到常见的配置文件。请自己选择要在几台电脑之间保持一致的文件。",
            )
        } else {
            tr(
                "These common settings files are on this computer. The checked ones go into the shared version; you can add more at any time under Advanced › Files.",
                "下面是在这台电脑上找到的常见配置文件，勾选的会放进共享版本。之后随时可以在「高级 › 文件」里添加更多。",
            )
        };
        let count = picked.len();
        div()
            .id("get-started")
            .size_full()
            .child(
                page(
                    Lucide::ListChecks,
                    tr("Choose files to sync", "选择要同步的文件"),
                    description,
                    cx,
                )
                .child(list)
                .child(
                    h_flex()
                        .gap_2()
                        .when(!suggestions.is_empty(), |row| {
                            row.child(
                                Button::new("add-suggested")
                                    .primary()
                                    .label(tr(
                                        if count == 1 {
                                            "Sync 1 Item".to_string()
                                        } else {
                                            format!("Sync {count} Items")
                                        },
                                        format!("同步这 {count} 项"),
                                    ))
                                    .loading(busy)
                                    .disabled(busy || count == 0)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        let suggestions = this
                                            .model
                                            .read(cx)
                                            .snapshot()
                                            .map(|s| s.suggestions.clone())
                                            .unwrap_or_default();
                                        let picked = this.picked(&suggestions);
                                        this.model.update(cx, |model, cx| {
                                            model.start_syncing(picked, cx)
                                        });
                                    })),
                            )
                        })
                        .child(
                            Button::new("pick-files")
                                .outline()
                                .label(tr("Choose Other Files…", "选择其他文件…"))
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| this.pick(cx))),
                        ),
                ),
            )
            .overflow_y_scrollbar()
    }
}

/// Uploads the shared version: a new private repository through the
/// GitHub CLI, or an empty repository's address.
pub struct ConnectView {
    model: Entity<Dotfiles>,
    name: Entity<InputState>,
    url: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl ConnectView {
    pub fn new(model: Entity<Dotfiles>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx).default_value("dotfiles"));
        let url = cx.new(|cx| {
            InputState::new(window, cx).placeholder(tr(
                "git@github.com:you/dotfiles.git",
                "git@github.com:你的用户名/dotfiles.git",
            ))
        });
        let subscriptions = vec![
            cx.observe(&model, |_, _, cx| cx.notify()),
            cx.subscribe(&name, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&url, |this, _, event: &InputEvent, cx| match event {
                InputEvent::PressEnter { .. } => this.connect(cx),
                _ => cx.notify(),
            }),
        ];
        model.update(cx, |model, cx| model.check_github(cx));
        Self {
            model,
            name,
            url,
            _subscriptions: subscriptions,
        }
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        let url = self.url.read(cx).value().trim().to_string();
        if url.is_empty() || self.model.read(cx).busy().is_some() {
            return;
        }
        self.model.update(cx, |model, cx| model.connect(url, cx));
    }
}

impl Render for ConnectView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let busy = model.busy().is_some();
        let error = model.connect_error().map(str::to_string);
        let name = self.name.read(cx).value().trim().to_string();
        let has_url = !self.url.read(cx).value().trim().is_empty();
        let muted = cx.theme().muted_foreground;
        let text = |content: String| div().text_sm().text_color(muted).child(content);

        let github: AnyElement = match model.github().clone() {
            GitHubState::Unknown | GitHubState::Checking => h_flex()
                .gap_2()
                .child(Spinner::new().small())
                .child(text(
                    tr(
                        "Checking whether GitHub is set up on this computer…",
                        "正在检查这台电脑上的 GitHub 登录…",
                    )
                    .into(),
                ))
                .into_any_element(),
            GitHubState::LoggedIn(login) => v_flex()
                .gap_2()
                .child(text(tr(
                    format!(
                        "Create a private repository in your GitHub account {login} and upload the shared version to it."
                    ),
                    format!("在你的 GitHub 账号 {login} 下新建一个私有仓库，把共享版本上传进去。"),
                )))
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().child(Input::new(&self.name).id("repo-name")))
                        .child(
                            Button::new("create-repo")
                                .primary()
                                .icon(Icon::new(Lucide::CloudUpload))
                                .label(tr("Create and Upload", "创建并上传"))
                                .loading(busy)
                                .disabled(busy || !chezmate_core::is_repo_name(&name))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let name = this.name.read(cx).value().trim().to_string();
                                    this.model
                                        .update(cx, |model, cx| model.create_github_repo(name, cx));
                                })),
                        ),
                )
                .into_any_element(),
            GitHubState::Unavailable => v_flex()
                .gap_2()
                .child(text(
                    tr(
                        "Create an empty private repository on GitHub's website, then paste its address below. (With the GitHub CLI, gh, installed and logged in, Chezmate can create it for you.)",
                        "先在 GitHub 网站上新建一个空的私有仓库，再把地址粘贴到下面。（装好并登录 GitHub CLI，也就是 gh 之后，Chezmate 可以替你创建。）",
                    )
                    .into(),
                ))
                .child(
                    Button::new("new-repo-page")
                        .outline()
                        .small()
                        .icon(Icon::new(Lucide::ExternalLink))
                        .label(tr("New Repository on GitHub", "在 GitHub 上新建仓库"))
                        .on_click(|_, _, cx| cx.open_url(NEW_REPOSITORY_URL)),
                )
                .into_any_element(),
        };

        v_flex()
            .id("connect")
            .w_full()
            .gap_4()
            .child(github)
            .child(
                v_flex()
                    .gap_2()
                    .pt_4()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(tr("Use an empty repository", "使用已有的空仓库")),
                    )
                    .child(text(
                        tr(
                            "Paste the address of an empty repository on GitHub, GitLab or another server (SSH or HTTPS).",
                            "粘贴一个空仓库的地址，GitHub、GitLab 或其他服务器都可以（SSH 或 HTTPS）。",
                        )
                        .into(),
                    ))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(Input::new(&self.url).id("repo-url")))
                            .child(
                                Button::new("connect-url")
                                    .outline()
                                    .label(tr("Upload", "上传"))
                                    .loading(busy)
                                    .disabled(busy || !has_url)
                                    .on_click(cx.listener(|this, _, _, cx| this.connect(cx))),
                            ),
                    ),
            )
            .when_some(error, |this, error| {
                this.child(Alert::error("connect-error", error).small())
            })
    }
}
