//! Changes: what differs between your home directory and your dotfiles, the
//! diff for each target, and the two ways to resolve it.

use std::sync::Arc;

use chezmate_core::{Change, EntryKind, ManagedEntry, StatusEntry};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    alert::Alert,
    button::{Button, ButtonVariant, ButtonVariants as _},
    diff::{Diff, DiffMode, DiffState},
    empty::{
        Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant,
        EmptyTitle,
    },
    h_flex,
    list::ListItem,
    resizable::{h_resizable, resizable_panel},
    spinner::Spinner,
    tag::Tag,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, KeyBinding, ParentElement as _, Render, ScrollStrategy, SharedString, Styled as _,
    Subscription, UniformListScrollHandle, Window, div, prelude::FluentBuilder as _, rems,
    uniform_list,
};

use crate::model::{Dotfiles, Snapshot};

gpui_kit::actions!(changes, [SelectNext, SelectPrevious]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("down", SelectNext, Some("ChangesList")),
        KeyBinding::new("j", SelectNext, Some("ChangesList")),
        KeyBinding::new("up", SelectPrevious, Some("ChangesList")),
        KeyBinding::new("k", SelectPrevious, Some("ChangesList")),
    ]);
}

/// What Apply does to one target, and whether it needs confirmation first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyPlan {
    /// No local edits to lose: apply right away.
    Direct,
    /// The target was edited outside chezmoi; applying replaces those edits.
    ConfirmOverwrite,
    /// A script: running it can change anything.
    ConfirmScript,
    /// chezmoi will delete the target.
    ConfirmDelete,
}

pub fn apply_plan(entry: &StatusEntry) -> ApplyPlan {
    if entry.is_script() {
        ApplyPlan::ConfirmScript
    } else if entry.has_local_changes() {
        ApplyPlan::ConfirmOverwrite
    } else if entry.apply == Change::Deleted {
        ApplyPlan::ConfirmDelete
    } else {
        ApplyPlan::Direct
    }
}

/// Whether "Keep Local" (re-add) can copy a target's local edits back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeepLocal {
    /// The target has no local edits.
    NotApplicable,
    Available,
    Unavailable(&'static str),
}

pub fn keep_local(entry: &StatusEntry, managed: Option<&ManagedEntry>) -> KeepLocal {
    if !entry.has_local_changes() || entry.is_script() {
        return KeepLocal::NotApplicable;
    }
    match managed {
        Some(m) if m.attributes.template => KeepLocal::Unavailable(
            "This file comes from a template. Edit the template in your source directory instead.",
        ),
        Some(m) if matches!(m.kind, EntryKind::Modify | EntryKind::Remove) => {
            KeepLocal::Unavailable("chezmoi computes this file with a script.")
        }
        Some(m) if m.kind == EntryKind::Directory => KeepLocal::NotApplicable,
        // Deleted locally: re-adding a missing file is not possible.
        _ if entry.local == Change::Deleted => {
            KeepLocal::Unavailable("The file no longer exists in your home directory.")
        }
        Some(_) => KeepLocal::Available,
        None => KeepLocal::Unavailable("chezmoi does not list this target as managed."),
    }
}

pub fn action_label(entry: &StatusEntry) -> &'static str {
    match entry.apply {
        Change::Added => "Create",
        Change::Modified => "Update",
        Change::Deleted => "Delete",
        Change::Run => "Run",
        Change::None => "Edited",
    }
}

pub struct ChangesView {
    model: Entity<Dotfiles>,
    selected: Option<String>,
    diff: Entity<DiffState>,
    /// The snapshot and selection the diff state currently displays.
    shown: Option<(usize, Option<String>)>,
    list_focus: FocusHandle,
    list_scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl ChangesView {
    pub fn new(model: Entity<Dotfiles>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let diff = cx.new(|cx| DiffState::new([], cx));
        let list_focus = cx.focus_handle().tab_stop(true);
        let subscriptions = vec![
            cx.observe(&model, |this, _, cx| this.sync(cx)),
            cx.observe(&diff, |_, _, cx| cx.notify()),
            cx.on_focus(&list_focus, window, |_, _, cx| cx.notify()),
            cx.on_blur(&list_focus, window, |_, _, cx| cx.notify()),
        ];
        let mut this = Self {
            model,
            selected: None,
            diff,
            shown: None,
            list_focus,
            list_scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        };
        this.sync(cx);
        this
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn focus_list(&self, window: &mut Window, cx: &mut App) {
        self.list_focus.focus(window, cx);
    }

    fn snapshot(&self, cx: &App) -> Option<Arc<Snapshot>> {
        self.model.read(cx).snapshot().cloned()
    }

    /// Keeps the selection valid for the current snapshot and shows its diff.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot(cx) else {
            cx.notify();
            return;
        };
        let still_listed = self
            .selected
            .as_ref()
            .is_some_and(|path| snapshot.status.iter().any(|e| &e.path == path));
        if !still_listed {
            self.selected = snapshot.status.first().map(|e| e.path.clone());
        }
        let key = (Arc::as_ptr(&snapshot) as usize, self.selected.clone());
        if self.shown.as_ref() != Some(&key) {
            let files: Vec<_> = self
                .selected
                .as_deref()
                .and_then(|path| snapshot.diff(path))
                .cloned()
                .into_iter()
                .collect();
            self.diff.update(cx, |state, cx| state.set_files(files, cx));
            self.shown = Some(key);
        }
        cx.notify();
    }

    pub fn select(&mut self, path: String, cx: &mut Context<Self>) {
        if self.selected.as_ref() == Some(&path) {
            return;
        }
        if let Some(snapshot) = self.snapshot(cx)
            && let Some(ix) = snapshot.status.iter().position(|e| e.path == path)
        {
            self.list_scroll.scroll_to_item(ix, ScrollStrategy::Center);
        }
        self.selected = Some(path);
        self.sync(cx);
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot(cx) else {
            return;
        };
        let count = snapshot.status.len();
        if count == 0 {
            return;
        }
        let current = self
            .selected
            .as_ref()
            .and_then(|path| snapshot.status.iter().position(|e| &e.path == path));
        let next = match current {
            Some(ix) => (ix as isize + delta).clamp(0, count as isize - 1) as usize,
            None => 0,
        };
        self.select(snapshot.status[next].path.clone(), cx);
    }

    fn selected_entry(&self, snapshot: &Snapshot) -> Option<StatusEntry> {
        let path = self.selected.as_ref()?;
        snapshot.status.iter().find(|e| &e.path == path).cloned()
    }

    /// Applies one target, asking first when the result is consequential.
    fn apply_entry(&mut self, entry: StatusEntry, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model.clone();
        let path = entry.path.clone();
        let (title, description, ok, variant, force) = match apply_plan(&entry) {
            ApplyPlan::Direct => {
                model.update(cx, |model, cx| model.apply(vec![path], false, cx));
                return;
            }
            ApplyPlan::ConfirmOverwrite => (
                format!("Overwrite “{path}”?"),
                "Your edits to this file will be replaced with the version from your dotfiles. To keep them, choose Keep Local instead.",
                "Overwrite",
                ButtonVariant::Danger,
                true,
            ),
            ApplyPlan::ConfirmScript => (
                format!("Run “{path}”?"),
                "Scripts can change anything on this computer. Review the script below before running it.",
                "Run Script",
                ButtonVariant::Primary,
                false,
            ),
            ApplyPlan::ConfirmDelete => (
                format!("Delete “{path}”?"),
                "Your dotfiles remove this file from your home directory.",
                "Delete",
                ButtonVariant::Danger,
                false,
            ),
        };
        window.open_alert_dialog(cx, move |alert, _, _| {
            let model = model.clone();
            let path = path.clone();
            alert
                .title(title.clone())
                .description(description)
                .confirm()
                .ok_text(ok)
                .ok_variant(variant)
                .on_ok(move |_, _, cx| {
                    let path = path.clone();
                    model.update(cx, |model, cx| model.apply(vec![path], force, cx));
                    true
                })
        });
    }

    fn apply_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot(cx) else {
            return;
        };
        let total = snapshot.status.len();
        let scripts = snapshot.status.iter().filter(|e| e.is_script()).count();
        let overwrites = snapshot
            .status
            .iter()
            .filter(|e| e.has_local_changes() && !e.is_script())
            .count();
        let mut lines = Vec::new();
        if scripts > 0 {
            lines.push(plural(scripts, "script runs", "scripts run"));
        }
        if overwrites > 0 {
            lines.push(format!(
                "{} edited outside chezmoi will be overwritten.",
                plural(overwrites, "file", "files")
            ));
        }
        let description = if lines.is_empty() {
            "Your home directory will match your dotfiles.".to_string()
        } else {
            lines.join(" ")
        };
        let title = format!("Apply {}?", plural(total, "change", "changes"));
        let variant = if overwrites > 0 {
            ButtonVariant::Danger
        } else {
            ButtonVariant::Primary
        };
        let model = self.model.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let model = model.clone();
            alert
                .title(title.clone())
                .description(description.clone())
                .confirm()
                .ok_text("Apply All")
                .ok_variant(variant)
                .on_ok(move |_, _, cx| {
                    model.update(cx, |model, cx| model.apply(Vec::new(), overwrites > 0, cx));
                    true
                })
        });
    }

    fn render_list(&self, snapshot: &Arc<Snapshot>, cx: &mut Context<Self>) -> AnyElement {
        let model = self.model.read(cx);
        let busy = model.busy().is_some();
        let loading = model.is_loading();
        let count = snapshot.status.len();
        let entries = snapshot.clone();
        let selected = self.selected.clone();
        let owner = cx.entity().downgrade();

        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .h_12()
                    .flex_none()
                    .gap_2()
                    .px_4()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child("Changes"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(count.to_string()),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("refresh")
                            .ghost()
                            .small()
                            .icon(IconName::RefreshCw)
                            .loading(loading)
                            .tooltip("Refresh")
                            .disabled(loading)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.model.update(cx, |model, cx| model.refresh(cx));
                            })),
                    )
                    .child(
                        Button::new("apply-all")
                            .primary()
                            .small()
                            .label("Apply All")
                            .disabled(busy || count == 0)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.apply_all(window, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .id("change-list")
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
                    .child(
                        uniform_list("changes", count, move |range, _, cx| {
                            range
                                .map(|ix| {
                                    let entry = &entries.status[ix];
                                    let managed = entries.managed_entry(&entry.path);
                                    let is_selected =
                                        selected.as_deref() == Some(entry.path.as_str());
                                    let owner = owner.clone();
                                    let path = entry.path.clone();
                                    change_row(entry, managed, is_selected, cx).on_click(
                                        move |_, window, cx| {
                                            let path = path.clone();
                                            let _ = owner.update(cx, |this, cx| {
                                                this.list_focus.focus(window, cx);
                                                this.select(path, cx);
                                            });
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

    fn render_detail(&self, snapshot: &Arc<Snapshot>, cx: &mut Context<Self>) -> AnyElement {
        let Some(entry) = self.selected_entry(snapshot) else {
            return div().into_any_element();
        };
        let busy = self.model.read(cx).busy().is_some();
        let managed = snapshot.managed_entry(&entry.path);
        let keep = keep_local(&entry, managed);
        let source = managed
            .map(|m| m.source_relative.clone())
            .unwrap_or_default();
        let has_diff = snapshot.diff(&entry.path).is_some();
        let split = self.diff.read(cx).mode() == DiffMode::Split;
        let apply_text = match apply_plan(&entry) {
            ApplyPlan::ConfirmScript => "Run Script…",
            ApplyPlan::ConfirmOverwrite => "Overwrite…",
            ApplyPlan::ConfirmDelete => "Delete…",
            ApplyPlan::Direct => "Apply",
        };

        let toolbar = h_flex()
            .h_12()
            .flex_none()
            .gap_2()
            .px_4()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .truncate()
                            .child(entry.path.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .truncate()
                            .text_color(cx.theme().muted_foreground)
                            .child(source),
                    ),
            )
            .child(
                Button::new("diff-mode")
                    .ghost()
                    .small()
                    .label(if split { "Unified" } else { "Side by Side" })
                    .disabled(!has_diff)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let mode = if split {
                            DiffMode::Unified
                        } else {
                            DiffMode::Split
                        };
                        this.diff.update(cx, |state, cx| state.set_mode(mode, cx));
                    })),
            )
            .when(keep != KeepLocal::NotApplicable, |row| {
                let (disabled, tooltip) = match keep {
                    KeepLocal::Unavailable(reason) => (true, reason),
                    _ => (false, "Copy your local edits into your dotfiles"),
                };
                let path = entry.path.clone();
                row.child(
                    Button::new("keep-local")
                        .outline()
                        .small()
                        .label("Keep Local")
                        .tooltip(tooltip)
                        .disabled(disabled || busy)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let path = path.clone();
                            this.model
                                .update(cx, |model, cx| model.re_add(vec![path], cx));
                        })),
                )
            })
            .child({
                let entry = entry.clone();
                Button::new("apply-selected")
                    .primary()
                    .small()
                    .label(apply_text)
                    .disabled(busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.apply_entry(entry.clone(), window, cx)
                    }))
            });

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
                        .child(if entry.is_script() {
                            "The script chezmoi will run."
                        } else {
                            "Removed lines are on disk now. Added lines are what Apply writes."
                        }),
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
            let message = snapshot
                .diff_error
                .clone()
                .unwrap_or_else(|| "chezmoi reports no content changes for this target.".into());
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
            .child(toolbar)
            .when(entry.has_local_changes() && !entry.is_script(), |this| {
                this.child(
                    div().flex_none().px_4().pt_3().child(
                        Alert::warning(
                            "local-edits",
                            "This file was changed outside chezmoi. Apply replaces your edits; Keep Local saves them to your dotfiles.",
                        )
                        .small()
                        .title("Edited locally"),
                    ),
                )
            })
            .child(body)
            .into_any_element()
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    if count == 1 {
        format!("1 {one}")
    } else {
        format!("{count} {many}")
    }
}

fn change_row(
    entry: &StatusEntry,
    managed: Option<&ManagedEntry>,
    selected: bool,
    cx: &App,
) -> ListItem {
    let (directory, name) = entry
        .path
        .rsplit_once('/')
        .unwrap_or(("", entry.path.as_str()));
    let icon = match managed.map(|m| m.kind) {
        _ if entry.is_script() => IconName::SquareTerminal,
        Some(EntryKind::Directory) => IconName::Folder,
        _ => IconName::FileText,
    };
    let tag = match entry.apply {
        Change::Deleted => Tag::danger(),
        Change::Run => Tag::info(),
        _ => Tag::secondary(),
    };
    ListItem::new(SharedString::from(entry.path.clone()))
        .h(rems(3.))
        .px_3()
        .w_full()
        .min_w_0()
        .selected(selected)
        .accessibility_label(format!("{} · {}", entry.path, action_label(entry)))
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
                .when(entry.has_local_changes() && !entry.is_script(), |this| {
                    this.child(Tag::warning().xsmall().child("Edited"))
                })
                .child(tag.xsmall().child(action_label(entry))),
        )
}

impl Render for ChangesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let snapshot = model.snapshot().cloned();
        let error = model.error().map(str::to_string);
        let loading = model.is_loading();

        let Some(snapshot) = snapshot else {
            return match error {
                Some(error) => error_state(error, cx),
                None => loading_state(cx),
            };
        };

        if snapshot.status.is_empty() {
            return v_flex()
                .size_full()
                .when_some(error, |this, error| {
                    this.child(div().p_4().child(Alert::error("refresh-error", error).small()))
                })
                .child(
                    div().flex_1().flex().items_center().justify_center().child(
                        Empty::new()
                            .header(
                                EmptyHeader::new()
                                    .media(
                                        EmptyMedia::new()
                                            .with_variant(EmptyMediaVariant::Icon)
                                            .child(Icon::new(IconName::CircleCheck)),
                                    )
                                    .title(EmptyTitle::new().child("Up to date"))
                                    .description(EmptyDescription::new().child(format!(
                                        "Your home directory matches your dotfiles ({} files managed).",
                                        snapshot.managed.len()
                                    ))),
                            )
                            .content(
                                EmptyContent::new().child(
                                    Button::new("refresh-empty")
                                        .outline()
                                        .small()
                                        .label("Check Again")
                                        .loading(loading)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.model.update(cx, |model, cx| model.refresh(cx));
                                        })),
                                ),
                            ),
                    ),
                )
                .into_any_element();
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
                    h_resizable("changes-panes")
                        .child(
                            resizable_panel()
                                .size(rem * 20.)
                                .size_range(rem * 14. ..rem * 36.)
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

pub fn loading_state(cx: &App) -> AnyElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .gap_2()
        .child(Spinner::new())
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Reading your dotfiles…"),
        )
        .into_any_element()
}

pub fn error_state(error: String, cx: &App) -> AnyElement {
    let _ = cx;
    div()
        .size_full()
        .p_6()
        .child(Alert::error("load-error", error).title("chezmoi could not be read"))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chezmate_core::{SourceAttributes, classify};

    fn status(local: Change, apply: Change) -> StatusEntry {
        StatusEntry {
            path: ".zshrc".into(),
            local,
            apply,
        }
    }

    fn managed(source: &str) -> ManagedEntry {
        let (kind, attributes) = classify(source, false);
        ManagedEntry {
            target: ".zshrc".into(),
            absolute: "/h/.zshrc".into(),
            source_absolute: format!("/s/{source}"),
            source_relative: source.into(),
            kind,
            attributes,
        }
    }

    #[test]
    fn apply_asks_before_losing_work() {
        assert_eq!(
            apply_plan(&status(Change::None, Change::Modified)),
            ApplyPlan::Direct
        );
        assert_eq!(
            apply_plan(&status(Change::Modified, Change::Modified)),
            ApplyPlan::ConfirmOverwrite
        );
        assert_eq!(
            apply_plan(&status(Change::None, Change::Run)),
            ApplyPlan::ConfirmScript
        );
        assert_eq!(
            apply_plan(&status(Change::None, Change::Deleted)),
            ApplyPlan::ConfirmDelete
        );
    }

    #[test]
    fn keep_local_explains_templates() {
        let edited = status(Change::Modified, Change::Modified);
        assert_eq!(
            keep_local(&edited, Some(&managed("dot_zshrc"))),
            KeepLocal::Available
        );
        assert!(matches!(
            keep_local(&edited, Some(&managed("dot_zshrc.tmpl"))),
            KeepLocal::Unavailable(_)
        ));
        assert_eq!(
            keep_local(
                &status(Change::None, Change::Modified),
                Some(&managed("dot_zshrc"))
            ),
            KeepLocal::NotApplicable
        );
        let _ = SourceAttributes::default();
    }
}
