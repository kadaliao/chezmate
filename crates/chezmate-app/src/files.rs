//! Files: every target chezmoi manages, with its source file and the
//! commands that change what is managed.

use std::path::PathBuf;
use std::sync::Arc;

use chezmate_core::{EntryKind, ManagedEntry};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    description_list::DescriptionList,
    h_flex,
    input::{Input, InputEvent, InputState},
    list::ListItem,
    resizable::{h_resizable, resizable_panel},
    tag::Tag,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, PathPromptOptions, Render,
    SharedString, Styled as _, Subscription, Task, UniformListScrollHandle, Window, div,
    prelude::FluentBuilder as _, rems, uniform_list,
};

use crate::changes::{error_state, loading_state};
use crate::model::{Dotfiles, Snapshot};

/// Asks the shell to show a target on the Changes page.
pub struct ReviewChange(pub String);

pub fn kind_label(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::File => "File",
        EntryKind::Directory => "Folder",
        EntryKind::Symlink => "Symlink",
        EntryKind::Script => "Script",
        EntryKind::Modify => "Modify script",
        EntryKind::Remove => "Removal",
    }
}

/// Human names for the attributes encoded in the source file name.
pub fn attribute_labels(entry: &ManagedEntry) -> Vec<&'static str> {
    let a = &entry.attributes;
    [
        (a.template, "Template"),
        (a.encrypted, "Encrypted"),
        (a.private, "Private"),
        (a.readonly, "Read-only"),
        (a.executable, "Executable"),
        (a.exact, "Exact"),
        (a.create, "Create only"),
        (a.empty, "Keep if empty"),
        (a.once, "Runs once"),
        (a.onchange, "Runs on change"),
    ]
    .into_iter()
    .filter_map(|(on, label)| on.then_some(label))
    .collect()
}

/// Directories chezmoi manages only because they contain managed files.
/// Every listed file already shows its folder, so these rows add nothing;
/// directories with attributes (private, exact, …) stay listed.
pub fn is_plain_directory(entry: &ManagedEntry) -> bool {
    entry.kind == EntryKind::Directory && entry.attributes == Default::default()
}

/// Indices of listed entries whose target or source path contains `query`
/// (case-insensitive).
pub fn filter_entries(entries: &[ManagedEntry], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| !is_plain_directory(e))
        .filter(|(_, e)| {
            query.is_empty()
                || e.target.to_lowercase().contains(&query)
                || e.source_relative.to_lowercase().contains(&query)
        })
        .map(|(ix, _)| ix)
        .collect()
}

pub struct FilesView {
    model: Entity<Dotfiles>,
    filter: Entity<InputState>,
    query: String,
    visible: Arc<Vec<usize>>,
    /// The snapshot `visible` was computed for.
    computed_for: usize,
    selected: Option<String>,
    list_scroll: UniformListScrollHandle,
    _pick: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ReviewChange> for FilesView {}

impl FilesView {
    pub fn new(model: Entity<Dotfiles>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter files"));
        let subscriptions = vec![
            cx.observe(&model, |this, _, cx| this.recompute(false, cx)),
            cx.subscribe_in(&filter, window, |this, state, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.query = state.read(cx).value().to_string();
                    this.recompute(true, cx);
                }
            }),
        ];
        let mut this = Self {
            model,
            filter,
            query: String::new(),
            visible: Arc::default(),
            computed_for: 0,
            selected: None,
            list_scroll: UniformListScrollHandle::new(),
            _pick: Task::ready(()),
            _subscriptions: subscriptions,
        };
        this.recompute(true, cx);
        this
    }

    fn snapshot(&self, cx: &App) -> Option<Arc<Snapshot>> {
        self.model.read(cx).snapshot().cloned()
    }

    fn recompute(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot(cx) else {
            cx.notify();
            return;
        };
        let key = Arc::as_ptr(&snapshot) as usize;
        if force || key != self.computed_for {
            self.visible = Arc::new(filter_entries(&snapshot.managed, &self.query));
            self.computed_for = key;
            let listed = self
                .selected
                .as_ref()
                .is_some_and(|target| snapshot.managed_entry(target).is_some());
            if !listed {
                self.selected = None;
            }
        }
        cx.notify();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn visible_count(&self) -> usize {
        self.visible.len()
    }

    pub fn select(&mut self, target: String, cx: &mut Context<Self>) {
        self.selected = Some(target);
        cx.notify();
    }

    /// Opens the system file picker and adds the chosen files.
    fn pick_and_add(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Add".into()),
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

    fn forget(&mut self, entry: &ManagedEntry, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model.clone();
        let target = entry.target.clone();
        let source = entry.source_relative.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let model = model.clone();
            let target = target.clone();
            alert
                .title(format!("Stop managing “{target}”?"))
                .description(format!(
                    "{source} is removed from your dotfiles. The file in your home directory stays as it is."
                ))
                .confirm()
                .ok_text("Stop Managing")
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
        let visible = self.visible.clone();
        let entries = snapshot.clone();
        let selected = self.selected.clone();
        let owner = cx.entity().downgrade();
        let count = visible.len();

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
                    .child(div().text_sm().font_weight(FontWeight::MEDIUM).child("Files"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(count.to_string()),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("add-files")
                            .outline()
                            .small()
                            .icon(IconName::Plus)
                            .label("Add…")
                            .tooltip("Choose files in your home directory to manage. Press ⌘⇧. to show hidden files.")
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
                div()
                    .id("file-list")
                    .flex_1()
                    .min_h_0()
                    .when(count == 0, |this| {
                        this.child(
                            div()
                                .p_4()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No managed files match."),
                        )
                    })
                    .child(
                        uniform_list("managed", count, move |range, _, cx| {
                            range
                                .map(|row| {
                                    let entry = &entries.managed[visible[row]];
                                    let target = entry.target.clone();
                                    let owner = owner.clone();
                                    let pending = entries.status.iter().any(|s| s.path == entry.target);
                                    file_row(entry, selected.as_deref() == Some(entry.target.as_str()), pending, cx)
                                        .on_click(move |_, _, cx| {
                                            let target = target.clone();
                                            let _ = owner.update(cx, |this, cx| this.select(target, cx));
                                        })
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
        let Some(entry) = self
            .selected
            .as_ref()
            .and_then(|target| snapshot.managed_entry(target))
            .cloned()
        else {
            return div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Select a file to see where it comes from.")
                .into_any_element();
        };
        let busy = self.model.read(cx).busy().is_some();
        let pending = snapshot
            .status
            .iter()
            .find(|s| s.path == entry.target)
            .cloned();
        let attributes = attribute_labels(&entry);
        let source = PathBuf::from(&entry.source_absolute);
        let target = PathBuf::from(&entry.absolute);
        let target_exists = target.exists();

        v_flex()
            .size_full()
            .min_h_0()
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
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(entry.target.clone()),
                    ),
            )
            .child(
                v_flex()
                    .p_4()
                    .gap_4()
                    .child(
                        DescriptionList::new()
                            .columns(1)
                            .bordered(true)
                            .item("In your home", entry.absolute.clone(), 1)
                            .item("Source file", entry.source_relative.clone(), 1)
                            .item("Type", kind_label(entry.kind), 1)
                            .item(
                                "Attributes",
                                if attributes.is_empty() {
                                    "None".to_string()
                                } else {
                                    attributes.join(", ")
                                },
                                1,
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child({
                                let source = source.clone();
                                // A directory or an encrypted file is not useful to open.
                                let reveal = entry.kind == EntryKind::Directory
                                    || entry.attributes.encrypted;
                                Button::new("open-source")
                                    .outline()
                                    .small()
                                    .label(if reveal { "Show Source" } else { "Open Source" })
                                    .on_click(move |_, _, cx| {
                                        if reveal {
                                            cx.reveal_path(&source)
                                        } else {
                                            cx.open_with_system(&source)
                                        }
                                    })
                            })
                            .when(target_exists, |row| {
                                let target = target.clone();
                                row.child(
                                    Button::new("reveal-target")
                                        .outline()
                                        .small()
                                        .label("Show in Finder")
                                        .on_click(move |_, _, cx| cx.reveal_path(&target)),
                                )
                            })
                            .child(div().flex_1())
                            .child({
                                let entry = entry.clone();
                                Button::new("forget")
                                    .danger()
                                    .small()
                                    .label("Stop Managing…")
                                    .disabled(busy)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.forget(&entry, window, cx)
                                    }))
                            }),
                    )
                    .when_some(pending, |this, _| {
                        let target = entry.target.clone();
                        this.child(
                            h_flex()
                                .gap_3()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("This file has changes waiting to be applied."),
                                )
                                .child(
                                    Button::new("review-change")
                                        .outline()
                                        .small()
                                        .label("Review Change")
                                        .on_click(cx.listener(move |_, _, _, cx| {
                                            cx.emit(ReviewChange(target.clone()))
                                        })),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }
}

fn file_row(entry: &ManagedEntry, selected: bool, pending: bool, cx: &App) -> ListItem {
    let (directory, name) = entry
        .target
        .rsplit_once('/')
        .unwrap_or(("", entry.target.as_str()));
    let icon = match entry.kind {
        EntryKind::Directory => IconName::Folder,
        EntryKind::Script | EntryKind::Modify => IconName::SquareTerminal,
        _ => IconName::FileText,
    };
    ListItem::new(SharedString::from(format!("file:{}", entry.target)))
        .h(rems(3.))
        .px_3()
        .w_full()
        .min_w_0()
        .selected(selected)
        .accessibility_label(entry.target.clone())
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
                        .child(
                            div()
                                .text_xs()
                                .truncate()
                                .text_color(cx.theme().muted_foreground)
                                .child(if directory.is_empty() {
                                    "~".to_string()
                                } else {
                                    format!("~/{directory}")
                                }),
                        ),
                )
                .when(pending, |this| {
                    this.child(Tag::info().xsmall().child("Changed"))
                })
                .when(entry.attributes.encrypted, |this| {
                    this.child(Tag::secondary().xsmall().child("Encrypted"))
                })
                .when(entry.attributes.template, |this| {
                    this.child(Tag::secondary().xsmall().child("Template"))
                }),
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
        h_resizable("files-panes")
            .child(
                resizable_panel()
                    .size(rem * 22.)
                    .size_range(rem * 16. ..rem * 40.)
                    .child(self.render_list(&snapshot, cx)),
            )
            .child(
                resizable_panel()
                    .size_range(rem * 24. ..rem * 400.)
                    .child(self.render_detail(&snapshot, cx)),
            )
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
    fn filter_matches_target_and_source_case_insensitively() {
        let entries = vec![
            entry(".zshrc", "dot_zshrc"),
            entry(".config/nvim/init.lua", "dot_config/nvim/init.lua.tmpl"),
        ];
        assert_eq!(filter_entries(&entries, ""), vec![0, 1]);
        let mut with_dirs = entries.clone();
        with_dirs.push(entry(".config", "dot_config"));
        let (kind, attributes) = classify("private_dot_ssh", true);
        with_dirs.push(ManagedEntry {
            kind,
            attributes,
            ..entry(".ssh", "private_dot_ssh")
        });
        with_dirs[2].kind = EntryKind::Directory;
        assert_eq!(
            filter_entries(&with_dirs, ""),
            vec![0, 1, 3],
            "plain folders are not listed"
        );
        assert_eq!(filter_entries(&entries, "NVIM"), vec![1]);
        assert_eq!(filter_entries(&entries, "tmpl"), vec![1]);
        assert!(filter_entries(&entries, "fish").is_empty());
    }

    #[test]
    fn attributes_read_as_words() {
        let e = entry(".netrc", "encrypted_private_dot_netrc.tmpl.age");
        assert_eq!(
            attribute_labels(&e),
            vec!["Template", "Encrypted", "Private"]
        );
    }
}
