//! Environment: which chezmoi Chezmate uses, where its files are, and an
//! on-demand `chezmoi doctor`.

use chezmate_core::CheckResult;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, alert::Alert, button::Button,
    description_list::DescriptionList, h_flex, scroll::ScrollableElement as _, tag::Tag, v_flex,
};
use gpui_kit::{
    AnyElement, App, Context, Entity, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _,
};

use crate::changes::{error_state, loading_state};
use crate::model::{DoctorState, Dotfiles};

pub struct EnvironmentView {
    model: Entity<Dotfiles>,
    _subscription: Subscription,
}

impl EnvironmentView {
    pub fn new(model: Entity<Dotfiles>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&model, |_, _, cx| cx.notify());
        Self {
            model,
            _subscription: subscription,
        }
    }
}

fn section_title(text: &'static str) -> impl IntoElement {
    div()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}

fn render_doctor(state: &DoctorState, cx: &App) -> AnyElement {
    match state {
        DoctorState::Idle => div()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child("Checks chezmoi's configuration and the tools it uses. Runs without network access.")
            .into_any_element(),
        DoctorState::Running => div()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child("Running chezmoi doctor…")
            .into_any_element(),
        DoctorState::Failed(error) => Alert::error(
            "doctor-error",
            format!(
                "{error}. A tool chezmoi checks (for example 1Password's `op`) may be waiting for approval."
            ),
        )
        .small()
        .into_any_element(),
        DoctorState::Done(checks) => v_flex()
            .gap_1()
            .children(checks.iter().map(|check| {
                let tag = match check.result {
                    CheckResult::Ok => Tag::success(),
                    CheckResult::Warning => Tag::warning(),
                    CheckResult::Error | CheckResult::Failed => Tag::danger(),
                    _ => Tag::secondary(),
                };
                let label = serde_label(check.result);
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(div().w_20().flex_none().child(tag.xsmall().child(label)))
                    .child(
                        div()
                            .w_40()
                            .flex_none()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .child(check.check.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(check.message.clone()),
                    )
            }))
            .into_any_element(),
    }
}

fn serde_label(result: CheckResult) -> &'static str {
    match result {
        CheckResult::Ok => "ok",
        CheckResult::Info => "info",
        CheckResult::Warning => "warning",
        CheckResult::Error => "error",
        CheckResult::Failed => "failed",
        CheckResult::Skipped => "skipped",
        CheckResult::Unknown => "other",
    }
}

impl Render for EnvironmentView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.model.read(cx);
        let Some(snapshot) = model.snapshot().cloned() else {
            return match model.error() {
                Some(error) => error_state(error.to_string(), cx),
                None => loading_state(cx),
            };
        };
        let info = &snapshot.info;
        let running = matches!(model.doctor(), DoctorState::Running);
        let doctor = render_doctor(model.doctor(), cx);
        let source = info.paths.source_dir.clone();
        let config = info.paths.config_file.clone();

        let source_label = format!(
            "{}{}",
            info.paths.source_dir.display(),
            if info.source_is_git_repo {
                "  ·  Git repository"
            } else {
                ""
            }
        );

        div()
            .id("environment")
            .size_full()
            .child(
                v_flex()
                    .p_6()
                    .gap_6()
                    .max_w(gpui_kit::rems(56.))
                    .child(
                        v_flex()
                            .gap_3()
                            .child(section_title("chezmoi"))
                            .child(
                                DescriptionList::new()
                                    .columns(1)
                                    .bordered(true)
                                    .item("Version", info.version.clone(), 1)
                                    .item("Executable", info.binary.display().to_string(), 1)
                                    .item("Source", source_label, 1)
                                    .item(
                                        "Destination",
                                        info.paths.dest_dir.display().to_string(),
                                        1,
                                    )
                                    .item(
                                        "Config",
                                        if info.config_exists {
                                            info.paths.config_file.display().to_string()
                                        } else {
                                            format!(
                                                "{} (not created)",
                                                info.paths.config_file.display()
                                            )
                                        },
                                        1,
                                    )
                                    .item("Managed files", snapshot.managed.len().to_string(), 1),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new("reveal-source")
                                            .outline()
                                            .small()
                                            .label("Show Source Folder")
                                            .on_click(move |_, _, cx| cx.reveal_path(&source)),
                                    )
                                    .when(info.config_exists, |row| {
                                        row.child(
                                            Button::new("open-config")
                                                .outline()
                                                .small()
                                                .label("Open Config")
                                                .on_click(move |_, _, cx| {
                                                    cx.open_with_system(&config)
                                                }),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_3()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(section_title("Diagnostics"))
                                    .child(div().flex_1())
                                    .child(
                                        Button::new("run-doctor")
                                            .outline()
                                            .small()
                                            .label("Run Diagnostics")
                                            .loading(running)
                                            .disabled(running)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.model
                                                    .update(cx, |model, cx| model.run_doctor(cx));
                                            })),
                                    ),
                            )
                            .child(doctor),
                    ),
            )
            .overflow_y_scrollbar()
            .into_any_element()
    }
}
