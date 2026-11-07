//! Overview: the home page of the simplified model. Every file that differs
//! between this computer and the shared version, grouped by what a sync does
//! with it; one Sync button; and a choice for each file that needs one.

use std::collections::BTreeSet;
use std::sync::Arc;

use chezmate_core::{
    Change, EntryKind, FileItem, FileState, Overview, SaveBlocker, ScriptRuns, SetupStep,
    SharedStatus, SyncPlan,
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    alert::Alert,
    button::{Button, ButtonVariant, ButtonVariants as _},
    checkbox::Checkbox,
    diff::{Diff, DiffMode, DiffState},
    empty::{
        Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant,
        EmptyTitle,
    },
    h_flex,
    list::ListItem,
    resizable::{h_resizable, resizable_panel},
    scroll::ScrollableElement as _,
    tag::Tag,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, px, rems,
};

use crate::changes::{SelectNext, SelectPrevious, error_state, loading_state};
use crate::i18n::tr;
use crate::model::{Dotfiles, DotfilesEvent, Prepared, Snapshot};
use crate::setup::{ConnectView, GetStarted};

/// A row of the overview list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    File(String),
    Step(String),
}

impl Selection {
    fn id(&self) -> SharedString {
        match self {
            Selection::File(target) => format!("item:{target}").into(),
            Selection::Step(target) => format!("step:{target}").into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// A sync leaves these alone until someone decides.
    Decide,
    ChangedHere,
    Updates,
    Steps,
}

impl Group {
    fn title(self) -> &'static str {
        match self {
            Group::Decide => tr("Needs your decision", "需要你决定"),
            Group::ChangedHere => tr("Changed on this computer", "这台电脑上改过"),
            Group::Updates => tr("New versions", "有新版本"),
            Group::Steps => tr("Setup steps", "设置步骤"),
        }
    }
}

/// The overview's rows in display order, by group. Empty groups are left
/// out.
pub fn grouped(overview: &Overview) -> Vec<(Group, Vec<Selection>)> {
    let files = |keep: fn(&FileItem) -> bool| -> Vec<Selection> {
        overview
            .files
            .iter()
            .filter(|f| keep(f))
            .map(|f| Selection::File(f.target.clone()))
            .collect()
    };
    let groups = [
        (Group::Decide, files(FileItem::needs_decision)),
        (
            Group::ChangedHere,
            files(|f| f.state == FileState::ChangedHere && !f.needs_decision()),
        ),
        (
            Group::Updates,
            files(|f| f.state == FileState::UpdateAvailable),
        ),
        (
            Group::Steps,
            overview
                .setup_steps
                .iter()
                .map(|s| Selection::Step(s.target.clone()))
                .collect(),
        ),
    ];
    groups
        .into_iter()
        .filter(|(_, rows)| !rows.is_empty())
        .collect()
}

/// What the file's state means, as a short title.
pub fn state_title(file: &FileItem) -> &'static str {
    match file.state {
        FileState::ChangedHere => match file.cannot_save {
            None => tr("Changed on this computer", "这台电脑上改过"),
            Some(SaveBlocker::Template) => tr(
                "Changed here · made from a template",
                "这台电脑上改过 · 由模板生成",
            ),
            Some(_) => tr(
                "Changed here · cannot be saved",
                "这台电脑上改过 · 不能自动保存",
            ),
        },
        FileState::UpdateAvailable => match file.incoming {
            Change::Added => tr("New in the shared version", "共享版本里新增"),
            Change::Deleted => tr("Removed from the shared version", "共享版本里已删除"),
            _ => tr("New version", "有新版本"),
        },
        FileState::BothChanged if file.never_synced => tr(
            "This computer has a different version",
            "这台电脑上已有不同的版本",
        ),
        FileState::BothChanged => tr("Changed on both sides", "两边都改了"),
        FileState::DeletedHere => tr("Deleted on this computer", "这台电脑上删除了"),
    }
}

/// One or two sentences on what happened and what a sync does.
pub fn explanation(file: &FileItem) -> String {
    let base = match file.state {
        FileState::ChangedHere => match file.cannot_save {
            None => tr(
                "You changed this file on this computer; the shared version has not changed. Sync saves your change, and your other computers get it when they sync.",
                "这个文件在这台电脑上改过，共享版本没有变化。同步时会把修改存到共享版本，其他电脑同步时就会收到。",
            ),
            Some(SaveBlocker::Template) => tr(
                "This file is made from a template, so each computer can get different contents. Changes made here cannot be saved back automatically: edit the template to keep them, or replace them with the shared version.",
                "这个文件由模板生成，每台电脑上的内容可能不同，所以这里的修改不能自动存回。想保留修改，请编辑模板；也可以用共享版本覆盖这里的修改。",
            ),
            Some(SaveBlocker::Script) => tr(
                "A script produces this file, so changes made here cannot be saved back. Replace them with the shared version, or change the script.",
                "这个文件由脚本生成，这里的修改不能存回。可以用共享版本覆盖，或者修改脚本。",
            ),
            Some(SaveBlocker::Directory) => tr(
                "This folder's permissions changed on this computer. Sync leaves it alone; restore it from the shared version if the change was not intended.",
                "这个文件夹的权限在这台电脑上改过。同步不会处理它；如果不是有意改的，可以用共享版本恢复。",
            ),
        },
        FileState::UpdateAvailable => match file.incoming {
            Change::Added => tr(
                "The shared version has this file and this computer does not, usually because it was added on another computer. Sync adds it here.",
                "共享版本里有这个文件，这台电脑上还没有，通常是在另一台电脑上新加的。同步时会把它放到这台电脑上。",
            ),
            Change::Deleted => tr(
                "The shared version no longer has this file, usually because it was removed on another computer. Sync deletes it here too.",
                "共享版本里已经没有这个文件，通常是在另一台电脑上删除的。同步时这台电脑上的也会删除。",
            ),
            _ => tr(
                "The shared version has a newer version, usually from another computer. Sync updates this computer. Nothing was changed here, so nothing is lost.",
                "共享版本里有更新的版本，通常来自另一台电脑。同步时会更新这台电脑；这里没有改过，不会丢失内容。",
            ),
        },
        FileState::BothChanged if file.never_synced => tr(
            "This computer already had this file before it was synced, and it differs from the shared version. Sync leaves it alone: choose which one to keep.",
            "这台电脑上原本就有这个文件，内容和共享版本不同，还没有同步过。同步不会动它，请选择保留哪一个。",
        ),
        FileState::BothChanged => tr(
            "This file changed on this computer and in the shared version. Sync leaves it alone: choose which one to keep.",
            "这个文件在这台电脑上和共享版本里都改过。同步不会动它，请选择保留哪一边。",
        ),
        FileState::DeletedHere => tr(
            "This file was deleted on this computer. Sync does not bring it back on its own: restore it, or stop syncing it.",
            "这个文件在这台电脑上被删除了。同步不会自动恢复它：可以恢复，也可以不再同步它。",
        ),
    };
    let mut text = base.to_string();
    if file.state == FileState::BothChanged && file.cannot_save == Some(SaveBlocker::Template) {
        text.push(' ');
        text.push_str(tr(
            "It is made from a template, so keeping this computer's version means editing the template.",
            "它由模板生成，要保留这台电脑的版本，需要编辑模板。",
        ));
    }
    text
}

fn step_explanation(step: &SetupStep) -> &'static str {
    match step.runs {
        ScriptRuns::Once => tr(
            "A setup script that runs once on each computer, typically to install software. Sync runs it. Check what it does below.",
            "每台电脑只运行一次的设置脚本，通常用来安装软件。同步时会运行它，可以先在下面看看它做什么。",
        ),
        _ => tr(
            "A setup script that runs again whenever it changes; it is new or has changed. Sync runs it. Check what it does below.",
            "内容变化时会再次运行的设置脚本，它是新加的或有变化。同步时会运行它，可以先在下面看看它做什么。",
        ),
    }
}

pub fn diff_caption(file: Option<&FileItem>) -> &'static str {
    match file.map(|f| f.state) {
        None => tr("The script that will run.", "将要运行的脚本。"),
        Some(FileState::ChangedHere) => tr(
            "Red: the shared version. Green: this computer.",
            "红色：共享版本。绿色：这台电脑上的内容。",
        ),
        Some(FileState::DeletedHere) => tr(
            "The shared version of the deleted file.",
            "被删除文件在共享版本里的内容。",
        ),
        Some(_) => tr(
            "Red: this computer now. Green: the shared version.",
            "红色：这台电脑上现在的内容。绿色：共享版本。",
        ),
    }
}

fn row_tag(file: &FileItem) -> Option<(Tag, &'static str)> {
    Some(match file.state {
        FileState::ChangedHere if file.cannot_save.is_some() => {
            (Tag::warning(), tr("Can't save", "不能自动保存"))
        }
        FileState::ChangedHere => return None,
        FileState::UpdateAvailable => match file.incoming {
            Change::Added => (Tag::secondary(), tr("New", "新增")),
            Change::Deleted => (Tag::danger(), tr("Delete", "删除")),
            _ => (Tag::secondary(), tr("Update", "更新")),
        },
        FileState::BothChanged if file.never_synced => (Tag::danger(), tr("Different", "版本不同")),
        FileState::BothChanged => (Tag::danger(), tr("Both changed", "两边都改了")),
        FileState::DeletedHere => (Tag::warning(), tr("Deleted", "已删除")),
    })
}

fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        one.to_string()
    } else {
        many.replace("{n}", &count.to_string())
    }
}

/// The headline above the Sync button.
pub fn headline(overview: &Overview) -> String {
    let pending = overview.files.len() + overview.setup_steps.len();
    let decide = overview.decisions().count();
    match (pending, decide) {
        (0, _) => tr("Everything is in sync", "全部已同步").to_string(),
        (n, 0) => tr(
            plural(n, "1 item to sync", "{n} items to sync"),
            format!("{n} 项待同步"),
        ),
        (n, d) if n == d => tr(
            plural(d, "1 file needs you", "{n} files need you"),
            format!("{d} 个文件需要你决定"),
        ),
        (n, d) => tr(
            format!("{n} items · {d} need you"),
            format!("{n} 项待同步 · {d} 项需要你决定"),
        ),
    }
}

/// One line about the shared version.
pub fn shared_line(shared: Option<&SharedStatus>) -> String {
    let Some(shared) = shared else {
        return tr(
            "The shared version is only on this computer (not a Git repository).",
            "共享版本只在这台电脑上（不是 Git 仓库）。",
        )
        .into();
    };
    let Some(upstream) = &shared.upstream else {
        return tr(
            "No remote set up: changes are saved on this computer only.",
            "没有设置远端：改动只保存在这台电脑上。",
        )
        .into();
    };
    if shared.behind > 0 {
        return tr(
            format!(
                "{} new from {upstream}",
                plural(shared.behind as usize, "1 commit", "{n} commits")
            ),
            format!("{upstream} 有 {} 个新提交", shared.behind),
        );
    }
    if shared.has_upload() {
        return tr(
            "Changes waiting to be uploaded.".to_string(),
            "有改动等待上传。".to_string(),
        );
    }
    tr(
        format!("Up to date with {upstream} as of the last check."),
        format!("上次检查时已和 {upstream} 一致。"),
    )
}

fn tilde(target: &str) -> String {
    format!("~/{target}")
}

/// Title, description, OK button text and style of a confirmation.
type Question = (String, &'static str, &'static str, ButtonVariant);

pub struct OverviewView {
    model: Entity<Dotfiles>,
    selected: Option<Selection>,
    diff: Entity<DiffState>,
    /// The snapshot and selection the diff state currently displays.
    shown: Option<(usize, Option<Selection>)>,
    list_focus: FocusHandle,
    /// Shown while nothing is synced yet.
    get_started: Entity<GetStarted>,
    _subscriptions: Vec<Subscription>,
}

impl OverviewView {
    pub fn new(model: Entity<Dotfiles>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let diff = cx.new(|cx| DiffState::new([], cx));
        let list_focus = cx.focus_handle().tab_stop(true);
        let subscriptions = vec![
            cx.observe(&model, |this, _, cx| this.sync_selection(cx)),
            cx.observe(&diff, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &model,
                window,
                |this, _, event: &DotfilesEvent, window, cx| match event {
                    DotfilesEvent::SyncReady(prepared) => {
                        this.review(prepared.clone(), window, cx);
                    }
                    DotfilesEvent::Connected => window.close_dialog(cx),
                    _ => {}
                },
            ),
        ];
        let get_started = cx.new(|cx| GetStarted::new(model.clone(), cx));
        let mut this = Self {
            model,
            selected: None,
            diff,
            shown: None,
            list_focus,
            get_started,
            _subscriptions: subscriptions,
        };
        this.sync_selection(cx);
        this
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected(&self) -> Option<&Selection> {
        self.selected.as_ref()
    }

    pub fn focus_list(&self, window: &mut Window, cx: &mut App) {
        self.list_focus.focus(window, cx);
    }

    fn snapshot(&self, cx: &App) -> Option<Arc<Snapshot>> {
        self.model.read(cx).snapshot().cloned()
    }

    fn rows(snapshot: &Snapshot) -> Vec<Selection> {
        grouped(&snapshot.overview)
            .into_iter()
            .flat_map(|(_, rows)| rows)
            .collect()
    }

    /// Keeps the selection valid for the current snapshot and shows its diff.
    fn sync_selection(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot(cx) else {
            cx.notify();
            return;
        };
        let rows = Self::rows(&snapshot);
        if !self.selected.as_ref().is_some_and(|s| rows.contains(s)) {
            self.selected = rows.first().cloned();
        }
        let key = (Arc::as_ptr(&snapshot) as usize, self.selected.clone());
        if self.shown.as_ref() != Some(&key) {
            let file = match &self.selected {
                Some(Selection::File(target)) => snapshot.review_diff(target),
                Some(Selection::Step(target)) => snapshot.diff(target),
                None => None,
            };
            let files: Vec<_> = file.cloned().into_iter().collect();
            self.diff.update(cx, |state, cx| state.set_files(files, cx));
            self.shown = Some(key);
        }
        cx.notify();
    }

    pub fn select(&mut self, selection: Selection, cx: &mut Context<Self>) {
        if self.selected.as_ref() != Some(&selection) {
            self.selected = Some(selection);
            self.sync_selection(cx);
        }
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot(cx) else {
            return;
        };
        let rows = Self::rows(&snapshot);
        if rows.is_empty() {
            return;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|s| rows.iter().position(|r| r == s));
        let next = match current {
            Some(ix) => (ix as isize + delta).clamp(0, rows.len() as isize - 1) as usize,
            None => 0,
        };
        self.select(rows[next].clone(), cx);
    }

    /// Opens the sync review for a freshly prepared overview.
    fn review(&mut self, prepared: Arc<Prepared>, window: &mut Window, cx: &mut Context<Self>) {
        let review = cx.new(|_| SyncReview::new(prepared));
        let model = self.model.clone();
        window.open_alert_dialog(cx, move |alert, _, cx| {
            let model = model.clone();
            let plan = review.read(cx).plan();
            let review_for_ok = review.clone();
            alert
                .title(tr("Sync this computer?", "同步这台电脑？"))
                .description(review.read(cx).summary())
                .width(px(520.))
                .child(review.clone())
                .confirm()
                .ok_text(tr("Sync", "同步"))
                .ok_variant(ButtonVariant::Primary)
                .when(plan.is_empty(), |alert| alert.ok_text(tr("Close", "关闭")))
                .on_ok(move |_, _, cx| {
                    let plan = review_for_ok.read(cx).plan();
                    if !plan.is_empty() {
                        model.update(cx, |model, cx| model.execute_sync(plan, cx));
                    }
                    true
                })
        });
    }

    /// Opens the dialog that uploads the shared version to a new or empty
    /// repository.
    pub fn open_connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let connect = cx.new(|cx| ConnectView::new(self.model.clone(), window, cx));
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title(tr("Upload to GitHub", "上传到 GitHub"))
                .width(px(560.))
                .child(connect.clone())
        });
    }

    /// Asks `question`, then runs `action` on the model if confirmed.
    fn confirm(
        &self,
        question: Question,
        action: impl Fn(&mut Dotfiles, &mut Context<Dotfiles>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (title, description, ok, variant) = question;
        let model = self.model.clone();
        let action = std::rc::Rc::new(action);
        window.open_alert_dialog(cx, move |alert, _, _| {
            let model = model.clone();
            let action = action.clone();
            alert
                .title(title.clone())
                .description(description)
                .confirm()
                .ok_text(ok)
                .ok_variant(variant)
                .on_ok(move |_, _, cx| {
                    let action = action.clone();
                    model.update(cx, |model, cx| action(model, cx));
                    true
                })
        });
    }

    fn render_header(&self, snapshot: &Snapshot, cx: &mut Context<Self>) -> impl IntoElement {
        let overview = &snapshot.overview;
        let model = self.model.read(cx);
        let busy = model.busy().is_some();
        let synced = overview.files.is_empty() && overview.setup_steps.is_empty();
        let decide = overview.decisions().count() > 0;
        let (icon, color) = if synced {
            (IconName::CircleCheck, cx.theme().success)
        } else if decide {
            (IconName::TriangleAlert, cx.theme().warning)
        } else {
            (IconName::RefreshCw, cx.theme().info)
        };
        v_flex()
            .flex_none()
            .gap_2()
            .p_4()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(icon).text_color(color))
                    .child(
                        div()
                            .text_base()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(headline(overview)),
                    ),
            )
            .map(|this| {
                if !snapshot.needs_remote() {
                    return this.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(shared_line(overview.shared.as_ref())),
                    );
                }
                // Synced files that only this computer has: the next step
                // is a place the other computers can get them from.
                this.child(
                    v_flex()
                        .gap_2()
                        .p_3()
                        .rounded_md()
                        .border_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().background)
                        .child(
                            div().text_xs().child(tr(
                                "These files are only on this computer. Upload them to GitHub so your other computers can get them.",
                                "这些文件目前只在这台电脑上。上传到 GitHub 后，其他电脑才能取回。",
                            )),
                        )
                        .child(
                            Button::new("open-connect")
                                .outline()
                                .small()
                                .icon(IconName::Github)
                                .label(tr("Upload to GitHub…", "上传到 GitHub…"))
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_connect(window, cx);
                                })),
                        ),
                )
            })
            .child(
                Button::new("sync")
                    .primary()
                    .w_full()
                    .icon(IconName::RefreshCw)
                    .label(tr("Sync", "同步"))
                    .loading(busy)
                    .disabled(busy)
                    .tooltip(tr(
                        "Get the shared version, then review what a sync saves and updates",
                        "先获取共享版本，再确认要保存和更新的内容",
                    ))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.model.update(cx, |model, cx| model.prepare_sync(cx));
                    })),
            )
    }

    fn render_list(&self, snapshot: &Arc<Snapshot>, cx: &mut Context<Self>) -> AnyElement {
        let overview = &snapshot.overview;
        let groups = grouped(overview);
        let mut list = v_flex().w_full().pb_2();
        if groups.is_empty() {
            list = list.child(
                div()
                    .px_4()
                    .py_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(tr(
                        format!("{} files, all in sync.", overview.managed_files),
                        format!("共 {} 个文件，全部已同步。", overview.managed_files),
                    )),
            );
        }
        for (group, rows) in groups {
            list = list.child(
                h_flex()
                    .px_4()
                    .pt_3()
                    .pb_1()
                    .gap_1()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(cx.theme().muted_foreground)
                    .child(group.title())
                    .child(format!("({})", rows.len())),
            );
            for row in rows {
                let selected = self.selected.as_ref() == Some(&row);
                let element = match &row {
                    Selection::File(target) => overview
                        .file(target)
                        .map(|file| file_row(file, selected, cx)),
                    Selection::Step(target) => overview
                        .setup_steps
                        .iter()
                        .find(|s| &s.target == target)
                        .map(|step| step_row(step, selected, cx)),
                };
                if let Some(element) = element {
                    let row = row.clone();
                    list = list.child(element.on_click(cx.listener(move |this, _, window, cx| {
                        this.list_focus.focus(window, cx);
                        this.select(row.clone(), cx);
                    })));
                }
            }
        }

        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().sidebar)
            .child(self.render_header(snapshot, cx))
            .child(
                div()
                    .id("overview-list")
                    .flex_1()
                    .min_h_0()
                    .track_focus(&self.list_focus)
                    .key_context("ChangesList")
                    .on_action(
                        cx.listener(|this, _: &SelectNext, _, cx| this.move_selection(1, cx)),
                    )
                    .on_action(
                        cx.listener(|this, _: &SelectPrevious, _, cx| this.move_selection(-1, cx)),
                    )
                    .child(list)
                    .overflow_y_scrollbar(),
            )
            .into_any_element()
    }

    fn actions(
        &self,
        file: &FileItem,
        snapshot: &Snapshot,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let target = file.target.clone();
        let mut row = h_flex().gap_2().flex_wrap();

        let keep_here = |label: &'static str, cx: &mut Context<Self>| {
            let target = target.clone();
            Button::new("keep-here")
                .primary()
                .small()
                .label(label)
                .disabled(busy)
                .on_click(cx.listener(move |this, _, _, cx| {
                    let target = target.clone();
                    this.model
                        .update(cx, |model, cx| model.keep_here(target, cx));
                }))
        };
        // Replaces this computer's version with the shared one, after asking.
        let replace = |label: &'static str, cx: &mut Context<Self>| {
            let target = target.clone();
            Button::new("take-shared")
                .outline()
                .small()
                .label(label)
                .disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| {
                    let target = target.clone();
                    this.confirm(
                        (
                            tr(
                                format!("Replace {} with the shared version?", tilde(&target)),
                                format!("用共享版本替换 {}？", tilde(&target)),
                            ),
                            tr(
                                "The changes made to this file on this computer will be lost.",
                                "这台电脑上对这个文件的修改会丢失。",
                            ),
                            tr("Replace", "替换"),
                            ButtonVariant::Danger,
                        ),
                        move |model, cx| model.take_shared(target.clone(), true, cx),
                        window,
                        cx,
                    );
                }))
        };
        let open_template = || {
            let source = snapshot
                .managed_entry(&target)
                .map(|m| std::path::PathBuf::from(&m.source_absolute));
            Button::new("open-source")
                .ghost()
                .small()
                .label(tr("Open Template", "打开模板"))
                .disabled(source.is_none())
                .on_click(move |_, _, cx| {
                    if let Some(source) = &source {
                        cx.open_with_system(source);
                    }
                })
        };

        match file.state {
            FileState::ChangedHere => {
                if file.can_save() {
                    row = row
                        .child(keep_here(tr("Save to Shared Version", "存到共享版本"), cx))
                        .child(replace(tr("Discard This Change…", "放弃这里的修改…"), cx));
                } else {
                    if file.cannot_save == Some(SaveBlocker::Template) {
                        row = row.child(open_template());
                    }
                    row = row.child(replace(tr("Use Shared Version…", "用共享版本覆盖…"), cx));
                }
            }
            FileState::BothChanged => {
                if file.can_save() {
                    row = row.child(keep_here(tr("Keep This Computer's", "保留这台电脑的"), cx));
                } else if file.cannot_save == Some(SaveBlocker::Template) {
                    row = row.child(open_template());
                }
                row = row.child(replace(tr("Use Shared Version…", "用共享版本…"), cx));
            }
            FileState::DeletedHere => {
                let restore_target = target.clone();
                let forget_target = target.clone();
                row = row
                    .child(
                        Button::new("restore")
                            .primary()
                            .small()
                            .label(tr("Restore", "恢复"))
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let target = restore_target.clone();
                                this.model
                                    .update(cx, |model, cx| model.take_shared(target, true, cx));
                            })),
                    )
                    .child(
                        Button::new("stop-syncing")
                            .outline()
                            .small()
                            .label(tr("Stop Syncing…", "不再同步…"))
                            .disabled(busy || file.kind == EntryKind::Directory)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let target = forget_target.clone();
                                this.confirm((
                                    tr(
                                        format!("Stop syncing {}?", tilde(&target)),
                                        format!("不再同步 {}？", tilde(&target)),
                                    ),
                                    tr(
                                        "It is removed from the shared version. Your other computers keep their copy, which is no longer synced.",
                                        "会从共享版本中移除这个文件。其他电脑上的这个文件会保留，但不再同步。",
                                    ),
                                    tr("Stop Syncing", "不再同步"),
                                    ButtonVariant::Danger),
                                    move |model, cx| model.stop_syncing(target.clone(), cx),
                                    window,
                                    cx,
                                );
                            })),
                    );
            }
            FileState::UpdateAvailable => {
                let delete = file.incoming == Change::Deleted;
                row = row.child(
                    Button::new("take-shared")
                        .primary()
                        .small()
                        .label(if delete {
                            tr("Delete Now…", "现在删除…")
                        } else {
                            tr("Update Now", "现在更新")
                        })
                        .disabled(busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            let target = target.clone();
                            if !delete {
                                this.model
                                    .update(cx, |model, cx| model.take_shared(target, false, cx));
                                return;
                            }
                            this.confirm((
                                tr(
                                    format!("Delete {}?", tilde(&target)),
                                    format!("删除 {}？", tilde(&target)),
                                ),
                                tr(
                                    "The shared version no longer has this file, so it is deleted on this computer too.",
                                    "共享版本里已经没有这个文件，这台电脑上的也会删除。",
                                ),
                                tr("Delete", "删除"),
                                ButtonVariant::Danger),
                                move |model, cx| model.take_shared(target.clone(), false, cx),
                                window,
                                cx,
                            );
                        })),
                );
            }
        }
        row
    }

    fn render_detail(&self, snapshot: &Arc<Snapshot>, cx: &mut Context<Self>) -> AnyElement {
        let Some(selection) = self.selected.clone() else {
            return self.render_synced(snapshot, cx);
        };
        let busy = self.model.read(cx).busy().is_some();
        let overview = &snapshot.overview;
        let (title, subtitle, explanation, actions, file) = match &selection {
            Selection::File(target) => {
                let Some(file) = overview.file(target) else {
                    return div().into_any_element();
                };
                (
                    tilde(target),
                    state_title(file).to_string(),
                    explanation(file),
                    self.actions(file, snapshot, busy, cx).into_any_element(),
                    Some(file),
                )
            }
            Selection::Step(target) => {
                let Some(step) = overview.setup_steps.iter().find(|s| &s.target == target) else {
                    return div().into_any_element();
                };
                let target = target.clone();
                let run = Button::new("run-step")
                    .primary()
                    .small()
                    .label(tr("Run Now…", "现在运行…"))
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let target = target.clone();
                        this.confirm((
                            tr(format!("Run {target}?"), format!("运行 {target}？")),
                            tr(
                                "Scripts can change anything on this computer. Check the script below first.",
                                "脚本可以修改这台电脑上的任何东西，请先看看下面的脚本内容。",
                            ),
                            tr("Run", "运行"),
                            ButtonVariant::Primary),
                            move |model, cx| model.run_step(target.clone(), cx),
                            window,
                            cx,
                        );
                    }));
                (
                    step.target.clone(),
                    tr("Setup step", "设置步骤").to_string(),
                    step_explanation(step).to_string(),
                    h_flex().child(run).into_any_element(),
                    None,
                )
            }
        };
        let has_diff = !self.diff.read(cx).files().is_empty();
        let split = self.diff.read(cx).mode() == DiffMode::Split;

        let header = v_flex()
            .flex_none()
            .gap_3()
            .p_4()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_base()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(subtitle),
                            ),
                    )
                    .child(
                        Button::new("diff-mode")
                            .ghost()
                            .small()
                            .label(if split {
                                tr("Unified", "合并显示")
                            } else {
                                tr("Side by Side", "左右对照")
                            })
                            .disabled(!has_diff)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let mode = if split {
                                    DiffMode::Unified
                                } else {
                                    DiffMode::Split
                                };
                                this.diff.update(cx, |state, cx| state.set_mode(mode, cx));
                            })),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(explanation),
            )
            .child(actions);

        let body = if has_diff {
            v_flex()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .flex_none()
                        .px_4()
                        .py_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(diff_caption(file)),
                )
                .child(
                    Diff::new(&self.diff)
                        .header_visible(false)
                        .flex_1()
                        .min_h_0()
                        .w_full(),
                )
                .into_any_element()
        } else {
            let message = snapshot.diff_error.clone().unwrap_or_else(|| {
                tr("No content differences to show.", "没有可显示的内容差异。").into()
            });
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(message),
                )
                .into_any_element()
        };

        v_flex()
            .size_full()
            .min_h_0()
            .child(header)
            .child(body)
            .into_any_element()
    }

    fn render_synced(&self, snapshot: &Snapshot, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.model.read(cx).busy().is_some();
        let overview = &snapshot.overview;
        let description = match overview.every_apply.as_slice() {
            [] => tr(
                format!(
                    "This computer matches the shared version ({} files).",
                    overview.managed_files
                ),
                format!(
                    "这台电脑和共享版本一致（共 {} 个文件）。",
                    overview.managed_files
                ),
            ),
            scripts => tr(
                format!(
                    "This computer matches the shared version ({} files). {} runs whenever a sync updates this computer.",
                    overview.managed_files,
                    scripts
                        .iter()
                        .map(|s| s.target.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                format!(
                    "这台电脑和共享版本一致（共 {} 个文件）。每次同步更新这台电脑时，还会运行 {}。",
                    overview.managed_files,
                    scripts
                        .iter()
                        .map(|s| s.target.as_str())
                        .collect::<Vec<_>>()
                        .join("、")
                ),
            ),
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(
                Empty::new()
                    .header(
                        EmptyHeader::new()
                            .media(
                                EmptyMedia::new()
                                    .with_variant(EmptyMediaVariant::Icon)
                                    .child(Icon::new(IconName::CircleCheck)),
                            )
                            .title(EmptyTitle::new().child(tr("In sync", "已同步")))
                            .description(EmptyDescription::new().child(description)),
                    )
                    .content(
                        EmptyContent::new().child(
                            Button::new("check-again")
                                .outline()
                                .small()
                                .label(tr("Check for Updates", "检查更新"))
                                .loading(busy)
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.model.update(cx, |model, cx| model.prepare_sync(cx));
                                })),
                        ),
                    ),
            )
            .into_any_element()
    }
}

fn file_row(file: &FileItem, selected: bool, cx: &App) -> ListItem {
    let (directory, name) = file
        .target
        .rsplit_once('/')
        .unwrap_or(("", file.target.as_str()));
    let icon = match file.kind {
        EntryKind::Directory => IconName::Folder,
        _ => IconName::FileText,
    };
    let location = if directory.is_empty() {
        "~".to_string()
    } else {
        format!("~/{directory}")
    };
    row(
        Selection::File(file.target.clone()).id(),
        icon,
        name.to_string(),
        location,
        row_tag(file),
        selected,
        format!("{} · {}", tilde(&file.target), state_title(file)),
        cx,
    )
}

fn step_row(step: &SetupStep, selected: bool, cx: &App) -> ListItem {
    let when = match step.runs {
        ScriptRuns::Once => tr("Runs once", "只运行一次"),
        _ => tr("New or changed", "新加的或有变化"),
    };
    row(
        Selection::Step(step.target.clone()).id(),
        IconName::SquareTerminal,
        step.target.clone(),
        when.to_string(),
        None,
        selected,
        format!("{} · {}", step.target, tr("Setup step", "设置步骤")),
        cx,
    )
}

#[allow(clippy::too_many_arguments)]
fn row(
    id: SharedString,
    icon: IconName,
    name: String,
    detail: String,
    tag: Option<(Tag, &'static str)>,
    selected: bool,
    label: String,
    cx: &App,
) -> ListItem {
    ListItem::new(id)
        .h(rems(3.))
        .px_4()
        .w_full()
        .min_w_0()
        .selected(selected)
        .accessibility_label(label)
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .child(
                    Icon::new(icon)
                        .small()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().truncate().child(name))
                        .child(
                            div()
                                .text_xs()
                                .truncate()
                                .text_color(cx.theme().muted_foreground)
                                .child(detail),
                        ),
                )
                .when_some(tag, |this, (tag, text)| {
                    this.child(tag.xsmall().child(text))
                }),
        )
}

impl Render for OverviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let snapshot = model.snapshot().cloned();
        let error = model.error().map(str::to_string);
        let Some(snapshot) = snapshot else {
            return match error {
                Some(error) => error_state(error, cx),
                None => loading_state(cx),
            };
        };
        if snapshot.is_empty() {
            return self.get_started.clone().into_any_element();
        }
        let rem = window.rem_size();
        v_flex()
            .size_full()
            .min_h_0()
            .when_some(error, |this, error| {
                this.child(
                    div()
                        .p_2()
                        .child(Alert::error("refresh-error", error).small()),
                )
            })
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("overview-panes")
                        .child(
                            resizable_panel()
                                .size(rem * 20.)
                                .size_range(rem * 15. ..rem * 36.)
                                .child(self.render_list(&snapshot, cx)),
                        )
                        .child(
                            resizable_panel()
                                .size_range(rem * 24. ..rem * 400.)
                                .child(self.render_detail(&snapshot, cx)),
                        ),
                ),
            )
            .into_any_element()
    }
}

/// The sync review inside the confirmation dialog: what will be saved
/// (each can be unchecked), updated, run and left alone.
pub struct SyncReview {
    prepared: Arc<Prepared>,
    /// Changes on this computer the person chose not to save this time.
    excluded: BTreeSet<String>,
}

impl SyncReview {
    fn new(prepared: Arc<Prepared>) -> Self {
        Self {
            prepared,
            excluded: BTreeSet::new(),
        }
    }

    fn overview(&self) -> &Overview {
        &self.prepared.overview
    }

    pub fn plan(&self) -> SyncPlan {
        let skip: Vec<String> = self.excluded.iter().cloned().collect();
        SyncPlan::new(self.overview(), &skip)
    }

    fn summary(&self) -> String {
        let plan = self.plan();
        let (save, update, run) = (plan.save.len(), plan.update.len(), plan.run.len());
        let mut parts = Vec::new();
        if save > 0 {
            parts.push(tr(format!("save {save}"), format!("保存 {save} 个")));
        }
        if update > 0 {
            parts.push(tr(format!("update {update}"), format!("更新 {update} 个")));
        }
        if run > 0 {
            parts.push(tr(
                plural(run, "run 1 setup step", "run {n} setup steps"),
                format!("运行 {run} 个设置步骤"),
            ));
        }
        let upload = plan.upload
            && self
                .overview()
                .shared
                .as_ref()
                .is_some_and(|s| s.upstream.is_some());
        match (parts.is_empty(), upload) {
            (true, false) => tr(
                "Nothing to do with the current selection.".to_string(),
                "按当前的选择，没有需要做的事。".to_string(),
            ),
            (true, true) => {
                let mut text = tr(
                    "Upload the shared version's changes".to_string(),
                    "上传共享版本里的改动".to_string(),
                );
                text.push_str(&self.other_changes_note());
                text.push_str(tr(".", "。"));
                text
            }
            (false, upload) => {
                let mut text = tr(
                    format!("This sync will {}", parts.join(", ")),
                    format!("这次同步会{}", parts.join("、")),
                );
                if upload {
                    text.push_str(tr(", then upload", "，然后上传"));
                    text.push_str(&self.other_changes_note());
                    text.push_str(tr(".", "。"));
                } else {
                    text.push_str(tr(".", "。"));
                }
                text
            }
        }
    }

    /// Uploading commits everything in the source directory, not only the
    /// files saved by this sync; say so when there is more.
    fn other_changes_note(&self) -> String {
        match self.overview().shared.as_ref().map_or(0, |s| s.uncommitted) {
            0 => String::new(),
            1 => tr(
                " (with 1 other change already in the shared version)".to_string(),
                "（包括共享版本里另外 1 处未上传的改动）".to_string(),
            ),
            n => tr(
                format!(" (with {n} other changes already in the shared version)"),
                format!("（包括共享版本里另外 {n} 处未上传的改动）"),
            ),
        }
    }

    fn toggle(&mut self, target: &str, checked: bool, cx: &mut Context<Self>) {
        if checked {
            self.excluded.remove(target);
        } else {
            self.excluded.insert(target.to_string());
        }
        cx.notify();
    }
}

impl Render for SyncReview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = window;
        let plan = SyncPlan::new(self.overview(), &[]);
        let section = |title: String, cx: &App| {
            div()
                .pt_2()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(cx.theme().muted_foreground)
                .child(title)
        };
        let line = |text: String| div().text_sm().truncate().child(text);
        let mut body = v_flex().gap_1().w_full();
        if let Some(err) = &self.prepared.offline {
            let message = tr(
                format!(
                    "Could not reach the remote, so this sync covers this computer only and uploading will fail. Sync again when you are online. ({err})"
                ),
                format!("连不上远端：这次只同步这台电脑，上传会失败，联网后再同步一次。（{err}）"),
            );
            body = body.child(
                div()
                    .id("offline")
                    .pb_2()
                    .child(Alert::warning("offline-alert", message).small()),
            );
        }

        if !plan.save.is_empty() {
            body = body.child(section(
                tr(
                    "Save to the shared version (uncheck to keep a change on this computer only)",
                    "存到共享版本（取消勾选的修改只留在这台电脑上）",
                )
                .to_string(),
                cx,
            ));
            for target in &plan.save {
                let checked = !self.excluded.contains(target);
                let owner = cx.entity().downgrade();
                let t = target.clone();
                body = body.child(
                    Checkbox::new(SharedString::from(format!("save:{target}")))
                        .label(tilde(target))
                        .checked(checked)
                        .on_click(move |checked, window, cx| {
                            let _ = owner.update(cx, |this, cx| this.toggle(&t, *checked, cx));
                            window.refresh();
                        }),
                );
            }
        }
        if !plan.update.is_empty() {
            body = body.child(section(
                tr("Update this computer", "更新这台电脑").to_string(),
                cx,
            ));
            for target in &plan.update {
                let deleted = self
                    .overview()
                    .file(target)
                    .is_some_and(|f| f.incoming == Change::Deleted);
                body = body.child(line(if deleted {
                    tr(
                        format!("{} (deleted)", tilde(target)),
                        format!("{}（删除）", tilde(target)),
                    )
                } else {
                    tilde(target)
                }));
            }
        }
        if !plan.run.is_empty() || !plan.every_apply.is_empty() {
            body = body.child(section(
                tr("Run setup steps", "运行设置步骤").to_string(),
                cx,
            ));
            for target in &plan.run {
                body = body.child(line(target.clone()));
            }
            for target in &plan.every_apply {
                body = body.child(line(tr(
                    format!("{target} (runs whenever this computer is updated)"),
                    format!("{target}（每次更新这台电脑时都会运行）"),
                )));
            }
        }
        if !plan.decide.is_empty() {
            body = body.child(section(
                tr("Left alone: needs your decision", "这次不处理：需要你决定").to_string(),
                cx,
            ));
            for target in &plan.decide {
                body = body.child(line(tilde(target)).text_color(cx.theme().muted_foreground));
            }
        }
        if plan.upload
            && self
                .overview()
                .shared
                .as_ref()
                .is_some_and(|s| s.upstream.is_none())
        {
            body = body.child(
                div()
                    .pt_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(tr(
                        "No remote is set up, so changes are committed on this computer only.",
                        "没有设置远端，改动只会提交在这台电脑上。",
                    )),
            );
        }
        div()
            .id("sync-review")
            .max_h(px(380.))
            .w_full()
            .child(body)
            .overflow_y_scrollbar()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(target: &str, state: FileState) -> FileItem {
        FileItem {
            target: target.into(),
            state,
            incoming: Change::Modified,
            kind: EntryKind::File,
            template: false,
            encrypted: false,
            cannot_save: None,
            never_synced: false,
        }
    }

    #[test]
    fn groups_follow_what_sync_does() {
        let mut template = file(".t", FileState::ChangedHere);
        template.cannot_save = Some(SaveBlocker::Template);
        let overview = Overview {
            files: vec![
                file(".a", FileState::ChangedHere),
                file(".b", FileState::UpdateAvailable),
                file(".c", FileState::BothChanged),
                template,
            ],
            ..Default::default()
        };
        let groups: Vec<(Group, Vec<Selection>)> = grouped(&overview);
        let names: Vec<(Group, Vec<String>)> = groups
            .into_iter()
            .map(|(g, rows)| {
                let rows = rows
                    .into_iter()
                    .map(|r| match r {
                        Selection::File(t) | Selection::Step(t) => t,
                    })
                    .collect();
                (g, rows)
            })
            .collect();
        assert_eq!(
            names,
            vec![
                (Group::Decide, vec![".c".to_string(), ".t".to_string()]),
                (Group::ChangedHere, vec![".a".to_string()]),
                (Group::Updates, vec![".b".to_string()]),
            ]
        );
        assert!(headline(&overview).contains('4'));
        assert!(headline(&Overview::default()).len() > 3);
    }

    #[test]
    fn review_unchecks_saves() {
        let overview = Overview {
            files: vec![
                file(".a", FileState::ChangedHere),
                file(".b", FileState::ChangedHere),
            ],
            ..Default::default()
        };
        let mut review = SyncReview::new(Arc::new(Prepared {
            overview,
            offline: None,
        }));
        assert_eq!(review.plan().save, vec![".a", ".b"]);
        review.excluded.insert(".a".into());
        let plan = review.plan();
        assert_eq!(plan.save, vec![".b"]);
        assert_eq!(plan.skipped, vec![".a"]);
    }
}
