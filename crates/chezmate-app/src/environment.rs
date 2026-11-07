//! Environment: which chezmoi Chezmate uses, where its files are, its
//! config file (shown and edited in place), and an on-demand
//! `chezmoi doctor`.

use chezmate_core::CheckResult;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, alert::Alert, button::Button,
    description_list::DescriptionList, h_flex, scroll::ScrollableElement as _, tag::Tag, v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, Styled as _, Subscription, Window, div,
};

use crate::changes::{error_state, loading_state};
use crate::config::ConfigPanel;
use crate::i18n::tr;
use crate::model::{DoctorState, Dotfiles};

pub struct EnvironmentView {
    model: Entity<Dotfiles>,
    config: Entity<ConfigPanel>,
    _subscription: Subscription,
}

impl EnvironmentView {
    pub fn new(model: Entity<Dotfiles>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&model, |_, _, cx| cx.notify());
        let config = cx.new(|cx| ConfigPanel::new(model.clone(), window, cx));
        Self {
            model,
            config,
            _subscription: subscription,
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn config(&self) -> &Entity<ConfigPanel> {
        &self.config
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
            .child(tr(
                "Checks chezmoi's configuration and the tools it uses. Runs without network access.",
                "检查 chezmoi 的配置和它用到的工具，不联网。",
            ))
            .into_any_element(),
        DoctorState::Running => div()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(tr("Running chezmoi doctor…", "正在运行 chezmoi doctor…"))
            .into_any_element(),
        DoctorState::Failed(error) => Alert::error(
            "doctor-error",
            tr(
                format!(
                    "{error}. A tool chezmoi checks (for example 1Password's `op`) may be waiting for approval."
                ),
                format!("{error}。chezmoi 检查的某个工具（例如 1Password 的 `op`）可能在等待授权。"),
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
        CheckResult::Ok => tr("ok", "正常"),
        CheckResult::Info => tr("info", "信息"),
        CheckResult::Warning => tr("warning", "警告"),
        CheckResult::Error => tr("error", "错误"),
        CheckResult::Failed => tr("failed", "失败"),
        CheckResult::Skipped => tr("skipped", "跳过"),
        CheckResult::Unknown => tr("other", "其他"),
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
        // A refresh failed (for example after saving a config chezmoi cannot
        // read); the details below are from the last one that worked.
        let failed = model.error().map(|error| {
            Alert::error("environment-error", error.to_string())
                .title(tr("chezmoi could not be read", "chezmoi 读取失败"))
                .small()
        });

        let source_label = format!(
            "{}{}",
            info.paths.source_dir.display(),
            if info.source_is_git_repo {
                tr("  ·  Git repository", "  ·  Git 仓库")
            } else {
                ""
            }
        );
        let config_label = if info.config_exists {
            info.paths.config_file.display().to_string()
        } else {
            tr(
                format!("{} (not created)", info.paths.config_file.display()),
                format!("{}（尚未创建）", info.paths.config_file.display()),
            )
        };

        div()
            .id("environment")
            .size_full()
            .child(
                v_flex()
                    .p_6()
                    .gap_6()
                    .max_w(gpui_kit::rems(56.))
                    .children(failed)
                    .child(
                        v_flex()
                            .gap_3()
                            .child(section_title("chezmoi"))
                            .child(
                                DescriptionList::new()
                                    .columns(1)
                                    .bordered(true)
                                    .item(tr("Version", "版本"), info.version.clone(), 1)
                                    .item(
                                        tr("Executable", "程序"),
                                        info.binary.display().to_string(),
                                        1,
                                    )
                                    .item(tr("Source", "源目录"), source_label, 1)
                                    .item(
                                        tr("Destination", "目标目录"),
                                        info.paths.dest_dir.display().to_string(),
                                        1,
                                    )
                                    .item(tr("Config", "配置文件"), config_label, 1)
                                    .item(
                                        tr("Managed files", "托管文件"),
                                        snapshot.managed.len().to_string(),
                                        1,
                                    ),
                            )
                            .child(
                                h_flex().child(
                                    Button::new("reveal-source")
                                        .outline()
                                        .small()
                                        .label(tr("Show Source Folder", "显示源目录"))
                                        .on_click(move |_, _, cx| cx.reveal_path(&source)),
                                ),
                            ),
                    )
                    .child(
                        v_flex()
                            .gap_3()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(section_title(tr("Diagnostics", "诊断")))
                                    .child(div().flex_1())
                                    .child(
                                        Button::new("run-doctor")
                                            .outline()
                                            .small()
                                            .label(tr("Run Diagnostics", "运行诊断"))
                                            .loading(running)
                                            .disabled(running)
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.model
                                                    .update(cx, |model, cx| model.run_doctor(cx));
                                            })),
                                    ),
                            )
                            .child(doctor),
                    )
                    .child(
                        v_flex()
                            .gap_3()
                            .child(section_title(tr("Config", "chezmoi 配置")))
                            .child(self.config.clone()),
                    ),
            )
            .overflow_y_scrollbar()
            .into_any_element()
    }
}
