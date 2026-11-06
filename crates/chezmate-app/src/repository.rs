//! Repository: the git repository behind the chezmoi source directory —
//! sync with the remote, commit what changed, and see recent history.

use std::sync::Arc;

use chezmate_core::{GitChange, GitFileStatus, RepoStatus};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    alert::Alert,
    button::{Button, ButtonVariants as _},
    diff::{Diff, DiffState},
    empty::{Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant, EmptyTitle},
    h_flex,
    input::{Input, InputEvent, InputState},
    list::ListItem,
    resizable::{h_resizable, resizable_panel},
    scroll::ScrollableElement as _,
    tag::Tag,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription,
    UniformListScrollHandle, Window, div, prelude::FluentBuilder as _, rems, uniform_list,
};

use crate::changes::{error_state, loading_state};
use crate::model::{Dotfiles, DotfilesEvent, Repo, Snapshot};

/// A one-line summary of how the branch relates to its upstream.
pub fn sync_summary(status: &RepoStatus) -> String {
    let Some(upstream) = &status.upstream else {
        return "No remote branch. Push needs an upstream set up in a terminal.".into();
    };
    match (status.ahead, status.behind) {
        (0, 0) => format!("Up to date with {upstream}"),
        (ahead, 0) => format!("{} to push to {upstream}", commits(ahead)),
        (0, behind) => format!("{} to pull from {upstream}", commits(behind)),
        (ahead, behind) => format!("{} to push, {} to pull", commits(ahead), commits(behind)),
    }
}

fn commits(n: u32) -> String {
    if n == 1 {
        "1 commit".into()
    } else {
        format!("{n} commits")
    }
}

pub fn status_letter(status: GitFileStatus) -> &'static str {
    match status {
        GitFileStatus::Modified => "Modified",
        GitFileStatus::Added => "Added",
        GitFileStatus::Deleted => "Deleted",
        GitFileStatus::Renamed => "Renamed",
        GitFileStatus::Copied => "Copied",
        GitFileStatus::TypeChanged => "Type changed",
        GitFileStatus::Untracked => "New",
        GitFileStatus::Conflicted => "Conflict",
    }
}

pub struct RepositoryView {
    model: Entity<Dotfiles>,
    message: Entity<InputState>,
    message_empty: bool,
    diff: Entity<DiffState>,
    shown: usize,
    selected: Option<String>,
    list_scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl RepositoryView {
    pub fn new(model: Entity<Dotfiles>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let message = cx.new(|cx| InputState::new(window, cx).placeholder("Describe what changed"));
        let diff = cx.new(|cx| DiffState::new([], cx));
        let subscriptions = vec![
            cx.observe(&model, |this, _, cx| this.sync(cx)),
            cx.observe(&diff, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &message,
                window,
                |this, state, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.message_empty = state.read(cx).value().trim().is_empty();
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(
                &model,
                window,
                |this, _, event: &DotfilesEvent, window, cx| {
                    if matches!(event, DotfilesEvent::Committed) {
                        this.message
                            .update(cx, |state, cx| state.set_value("", window, cx));
                        this.message_empty = true;
                    }
                },
            ),
        ];
        let mut this = Self {
            model,
            message,
            message_empty: true,
            diff,
            shown: 0,
            selected: None,
            list_scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        this.sync(cx);
        this
    }

    fn snapshot(&self, cx: &App) -> Option<Arc<Snapshot>> {
        self.model.read(cx).snapshot().cloned()
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        if let Some(snapshot) = self.snapshot(cx) {
            let key = Arc::as_ptr(&snapshot) as usize;
            if key != self.shown {
                let files = match &snapshot.repo {
                    Some(Ok(repo)) => repo.diffs.clone(),
                    _ => Vec::new(),
                };
                self.diff.update(cx, |state, cx| state.set_files(files, cx));
                self.shown = key;
            }
        }
        cx.notify();
    }

    fn select(&mut self, path: String, cx: &mut Context<Self>) {
        self.diff
            .update(cx, |state, cx| state.scroll_to_file(path.as_str(), cx));
        self.selected = Some(path);
        cx.notify();
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        let message = self.message.read(cx).value().trim().to_string();
        if message.is_empty() {
            return;
        }
        self.model.update(cx, |model, cx| model.commit(message, cx));
    }

    fn render_sync(&self, repo: &Repo, busy: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let status = &repo.status;
        let has_upstream = status.upstream.is_some();
        let branch = status
            .branch
            .clone()
            .unwrap_or_else(|| "Detached HEAD".into());
        v_flex()
            .flex_none()
            .gap_2()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(IconName::Github).small().text_color(cx.theme().muted_foreground))
                    .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(branch)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(sync_summary(status)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("fetch")
                            .outline()
                            .small()
                            .label("Fetch")
                            .tooltip("Check the remote for new commits")
                            .disabled(busy || !has_upstream)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.model.update(cx, |model, cx| model.fetch(cx))
                            })),
                    )
                    .child(
                        Button::new("pull")
                            .outline()
                            .small()
                            .label(match status.behind {
                                0 => "Pull".to_string(),
                                n => format!("Pull {n}"),
                            })
                            .tooltip("Update your dotfiles from the remote. Nothing is applied until you review it in Changes.")
                            .disabled(busy || !has_upstream)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.model.update(cx, |model, cx| model.pull(cx))
                            })),
                    )
                    .child(
                        Button::new("push")
                            .outline()
                            .small()
                            .label(match status.ahead {
                                0 => "Push".to_string(),
                                n => format!("Push {n}"),
                            })
                            .disabled(busy || !has_upstream || status.ahead == 0)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.model.update(cx, |model, cx| model.push(cx))
                            })),
                    ),
            )
    }

    fn render_changes(&self, repo: &Repo, cx: &mut Context<Self>) -> AnyElement {
        let changes: Arc<Vec<GitChange>> = Arc::new(repo.status.changes.clone());
        let count = changes.len();
        let selected = self.selected.clone();
        let owner = cx.entity().downgrade();
        v_flex()
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .px_4()
                    .pt_3()
                    .pb_1()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Uncommitted"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(count.to_string()),
                    ),
            )
            .child(
                div()
                    .id("git-changes")
                    .flex_1()
                    .min_h_0()
                    .when(count == 0, |this| {
                        this.child(
                            div()
                                .px_4()
                                .py_2()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("Nothing to commit."),
                        )
                    })
                    .child(
                        uniform_list("git-change-list", count, move |range, _, cx| {
                            range
                                .map(|ix| {
                                    let change = &changes[ix];
                                    let path = change.path.clone();
                                    let owner = owner.clone();
                                    git_change_row(
                                        change,
                                        selected.as_deref() == Some(path.as_str()),
                                        cx,
                                    )
                                    .on_click(
                                        move |_, _, cx| {
                                            let path = path.clone();
                                            let _ =
                                                owner.update(cx, |this, cx| this.select(path, cx));
                                        },
                                    )
                                })
                                .collect()
                        })
                        .track_scroll(&self.list_scroll)
                        .size_full(),
                    ),
            )
            .into_any_element()
    }

    fn render_commit_box(
        &self,
        repo: &Repo,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let can_commit = !repo.status.is_clean() && !self.message_empty && !busy;
        v_flex()
            .flex_none()
            .gap_2()
            .px_4()
            .py_3()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(Input::new(&self.message).id("commit-message").small())
            .child(
                Button::new("commit")
                    .primary()
                    .small()
                    .w_full()
                    .label("Commit All")
                    .disabled(!can_commit)
                    .on_click(cx.listener(|this, _, _, cx| this.commit(cx))),
            )
    }

    fn render_history(&self, repo: &Repo, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .flex_none()
            .h(rems(14.))
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex_none()
                    .px_4()
                    .pt_3()
                    .pb_1()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .child("History"),
            )
            .child(
                div()
                    .id("history")
                    .flex_1()
                    .min_h_0()
                    .child(
                        v_flex()
                            .px_4()
                            .pb_2()
                            .gap_2()
                            .when(repo.log.is_empty(), |this| {
                                this.child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("No commits yet."),
                                )
                            })
                            .children(repo.log.iter().map(|commit| {
                                let date = commit
                                    .date
                                    .split('T')
                                    .next()
                                    .unwrap_or_default()
                                    .to_string();
                                v_flex()
                                    .min_w_0()
                                    .child(div().text_sm().truncate().child(commit.subject.clone()))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!(
                                                "{} · {date} · {}",
                                                commit.short_hash, commit.author
                                            )),
                                    )
                            })),
                    )
                    .overflow_y_scrollbar(),
            )
    }

    fn render_diff(&self, repo: &Repo, cx: &mut Context<Self>) -> AnyElement {
        if repo.status.is_clean() {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Your source directory has no uncommitted changes.")
                .into_any_element();
        }
        if let Some(error) = &repo.diff_error {
            return div()
                .p_4()
                .child(Alert::error("git-diff-error", error.clone()).small())
                .into_any_element();
        }
        Diff::new(&self.diff).size_full().into_any_element()
    }
}

fn git_change_row(change: &GitChange, selected: bool, cx: &App) -> ListItem {
    let (directory, name) = change
        .path
        .rsplit_once('/')
        .unwrap_or(("", change.path.as_str()));
    let tag = match change.status {
        GitFileStatus::Deleted => Tag::danger(),
        GitFileStatus::Conflicted => Tag::warning(),
        GitFileStatus::Untracked | GitFileStatus::Added => Tag::success(),
        _ => Tag::secondary(),
    };
    ListItem::new(SharedString::from(format!("git:{}", change.path)))
        .h(rems(2.75))
        .px_3()
        .w_full()
        .min_w_0()
        .selected(selected)
        .accessibility_label(format!(
            "{} · {}",
            change.path,
            status_letter(change.status)
        ))
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().truncate().child(name.to_string()))
                        .when(!directory.is_empty(), |this| {
                            this.child(
                                div()
                                    .text_xs()
                                    .truncate()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(directory.to_string()),
                            )
                        }),
                )
                .child(tag.xsmall().child(status_letter(change.status))),
        )
}

impl Render for RepositoryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let busy = model.busy().is_some();
        let Some(snapshot) = model.snapshot().cloned() else {
            return match model.error() {
                Some(error) => error_state(error.to_string(), cx),
                None => loading_state(cx),
            };
        };
        let repo = match &snapshot.repo {
            None => {
                return div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Empty::new().header(
                            EmptyHeader::new()
                                .media(
                                    EmptyMedia::new()
                                        .with_variant(EmptyMediaVariant::Icon)
                                        .child(Icon::new(IconName::Github)),
                                )
                                .title(EmptyTitle::new().child("Not a Git repository"))
                                .description(EmptyDescription::new().child(format!(
                                    "{} is not under version control. Run `chezmoi git init` and add a remote to sync between machines.",
                                    snapshot.info.paths.working_tree.display()
                                ))),
                        ),
                    )
                    .into_any_element();
            }
            Some(Err(error)) => return error_state(error.clone(), cx),
            Some(Ok(repo)) => repo,
        };

        let rem = window.rem_size();
        h_resizable("repository-panes")
            .child(
                resizable_panel()
                    .size(rem * 22.)
                    .size_range(rem * 18. ..rem * 36.)
                    .child(
                        v_flex()
                            .size_full()
                            .min_h_0()
                            .bg(cx.theme().sidebar)
                            .child(self.render_sync(repo, busy, cx))
                            .child(self.render_changes(repo, cx))
                            .child(self.render_commit_box(repo, busy, cx))
                            .child(self.render_history(repo, cx)),
                    ),
            )
            .child(
                resizable_panel()
                    .size_range(rem * 24. ..rem * 400.)
                    .child(self.render_diff(repo, cx)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_sync_state() {
        let mut status = RepoStatus {
            upstream: Some("origin/main".into()),
            ..Default::default()
        };
        assert_eq!(sync_summary(&status), "Up to date with origin/main");
        status.ahead = 2;
        assert_eq!(sync_summary(&status), "2 commits to push to origin/main");
        status.behind = 1;
        assert_eq!(sync_summary(&status), "2 commits to push, 1 commit to pull");
        status.upstream = None;
        assert!(sync_summary(&status).starts_with("No remote branch"));
    }
}
