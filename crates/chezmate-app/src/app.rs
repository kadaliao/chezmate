//! The window shell: title bar, navigation, the current page and status bar.

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, TitleBar, WindowExt as _, h_flex,
    notification::NotificationType,
    sidebar::{Sidebar, SidebarGroup, SidebarMenu, SidebarMenuItem},
    spinner::Spinner,
    status_bar::StatusBar,
    v_flex,
};
use gpui_kit::{
    AnyView, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, KeyBinding, Menu, MenuItem, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _,
};

use chezmate_core::ChezmoiOptions;

use crate::changes::ChangesView;
use crate::environment::EnvironmentView;
use crate::files::{FilesView, ReviewChange};
use crate::model::{Dotfiles, DotfilesEvent};
use crate::repository::RepositoryView;

gpui_kit::actions!(chezmate, [Refresh, Quit]);

pub fn init(cx: &mut App) {
    crate::changes::init(cx);
    let mac = cx.platform_info().is_mac();
    cx.bind_keys([
        KeyBinding::new(
            if mac { "cmd-r" } else { "ctrl-r" },
            Refresh,
            Some("Chezmate"),
        ),
        KeyBinding::new(if mac { "cmd-q" } else { "ctrl-q" }, Quit, None),
    ]);
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.set_menus(vec![Menu {
        name: "Chezmate".into(),
        items: vec![MenuItem::action("Quit Chezmate", Quit)],
        disabled: false,
    }]);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Changes,
    Files,
    Repository,
    Environment,
}

pub struct Chezmate {
    model: Entity<Dotfiles>,
    page: Page,
    changes: Entity<ChangesView>,
    files: Entity<FilesView>,
    repository: Entity<RepositoryView>,
    environment: Entity<EnvironmentView>,
    _subscriptions: Vec<Subscription>,
}

impl Chezmate {
    pub fn new(options: ChezmoiOptions, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = cx.new(|cx| Dotfiles::new(options, cx));
        let changes = cx.new(|cx| ChangesView::new(model.clone(), window, cx));
        let files = cx.new(|cx| FilesView::new(model.clone(), window, cx));
        let repository = cx.new(|cx| RepositoryView::new(model.clone(), window, cx));
        let environment = cx.new(|cx| EnvironmentView::new(model.clone(), cx));
        let subscriptions = vec![
            cx.observe(&model, |_, _, cx| cx.notify()),
            cx.subscribe_in(&model, window, |_, _, event: &DotfilesEvent, window, cx| {
                let DotfilesEvent::Finished(result) = event else {
                    return;
                };
                let note = match result {
                    Ok(message) => (
                        NotificationType::Success,
                        SharedString::from(message.clone()),
                    ),
                    Err(error) => (NotificationType::Error, SharedString::from(error.clone())),
                };
                window.push_notification(note, cx);
            }),
            cx.subscribe_in(
                &files,
                window,
                |this, _, event: &ReviewChange, window, cx| {
                    let target = event.0.clone();
                    this.changes.update(cx, |changes, cx| {
                        changes.select(target, cx);
                        changes.focus_list(window, cx);
                    });
                    this.set_page(Page::Changes, cx);
                },
            ),
        ];
        changes.update(cx, |changes, cx| changes.focus_list(window, cx));
        Self {
            model,
            page: Page::Changes,
            changes,
            files,
            repository,
            environment,
            _subscriptions: subscriptions,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn model(&self) -> &Entity<Dotfiles> {
        &self.model
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn changes(&self) -> &Entity<ChangesView> {
        &self.changes
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn files(&self) -> &Entity<FilesView> {
        &self.files
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn page(&self) -> Page {
        self.page
    }

    pub fn set_page(&mut self, page: Page, cx: &mut Context<Self>) {
        self.page = page;
        cx.notify();
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let owner = cx.entity().downgrade();
        let changes = self
            .model
            .read(cx)
            .snapshot()
            .map(|snapshot| snapshot.status.len());
        let item = |label: &'static str, icon: IconName, page: Page| {
            let owner = owner.clone();
            SidebarMenuItem::new(label)
                .icon(icon)
                .active(self.page == page)
                .on_click(move |_, _, cx| {
                    let _ = owner.update(cx, |this, cx| this.set_page(page, cx));
                })
        };
        Sidebar::new("navigation")
            .collapsible(false)
            .child(
                SidebarGroup::new("Dotfiles").child(
                    SidebarMenu::new()
                        .child(item("Changes", IconName::Inbox, Page::Changes).when_some(
                            changes.filter(|count| *count > 0),
                            |item, count| {
                                item.suffix(move |_, cx| {
                                    div()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(count.to_string())
                                })
                            },
                        ))
                        .child(item("Files", IconName::Folder, Page::Files))
                        .child(item("Repository", IconName::Github, Page::Repository)),
                ),
            )
            .child(
                SidebarGroup::new("chezmoi").child(SidebarMenu::new().child(item(
                    "Environment",
                    IconName::Settings2,
                    Page::Environment,
                ))),
            )
    }

    fn render_status_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let left: SharedString = if let Some(busy) = model.busy() {
            format!("{busy}…").into()
        } else if model.is_loading() {
            "Reading chezmoi state…".into()
        } else if let Some(snapshot) = model.snapshot() {
            match snapshot.status.len() {
                0 => "Up to date".into(),
                1 => "1 change to apply".into(),
                n => format!("{n} changes to apply").into(),
            }
        } else {
            SharedString::default()
        };
        let working = model.busy().is_some() || model.is_loading();
        let right = model
            .snapshot()
            .map(|snapshot| format!("chezmoi {}", snapshot.info.version))
            .unwrap_or_default();
        StatusBar::new()
            .left(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .when(working, |this| this.child(Spinner::new().xsmall()))
                    .child(left),
            )
            .right(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(right),
            )
    }
}

impl Render for Chezmate {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page: AnyView = match self.page {
            Page::Changes => self.changes.clone().into(),
            Page::Files => self.files.clone().into(),
            Page::Repository => self.repository.clone().into(),
            Page::Environment => self.environment.clone().into(),
        };
        let source = self
            .model
            .read(cx)
            .snapshot()
            .map(|snapshot| snapshot.info.paths.source_dir.display().to_string())
            .unwrap_or_default();
        v_flex()
            .id("chezmate")
            .key_context("Chezmate")
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .on_action(cx.listener(|this, _: &Refresh, _, cx| {
                this.model.update(cx, |model, cx| model.refresh(cx));
            }))
            .child(
                TitleBar::new().child(
                    h_flex()
                        .size_full()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .child("Chezmate"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .truncate()
                                .child(source),
                        ),
                ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(self.render_sidebar(cx))
                    .child(div().flex_1().min_w_0().min_h_0().child(page)),
            )
            .child(self.render_status_bar(cx))
    }
}
