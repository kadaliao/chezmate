//! Environment: which chezmoi Chezmate uses, where its files are, its
//! config file (shown and edited in place), and an on-demand
//! `chezmoi doctor` whose results can be copied for an agent.

use std::fmt::Write as _;
use std::path::Path;

use chezmate_core::{CheckResult, Info};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _,
    alert::Alert,
    button::{Button, ButtonVariants as _},
    description_list::DescriptionList,
    h_flex,
    notification::NotificationType,
    scroll::ScrollableElement as _,
    tag::Tag,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _,
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

    /// Puts the diagnostics on the clipboard, written for an agent.
    fn copy_for_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let model = self.model.read(cx);
        let Some(snapshot) = model.snapshot() else {
            return;
        };
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let Some(report) = agent_report(&snapshot.info, model.doctor(), home.as_deref()) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(report));
        window.push_notification(
            (
                NotificationType::Success,
                SharedString::from(tr(
                    "Copied. Paste it into Claude Code, Codex or another agent.",
                    "已复制，可以粘贴给 Claude Code、Codex 等 Agent。",
                )),
            ),
            cx,
        );
    }
}

/// chezmoi's own word for a result, as `chezmoi doctor` prints it.
fn result_word(result: CheckResult) -> &'static str {
    match result {
        CheckResult::Ok => "ok",
        CheckResult::Info => "info",
        CheckResult::Warning => "warning",
        CheckResult::Error => "error",
        CheckResult::Failed => "failed",
        CheckResult::Skipped => "skipped",
        CheckResult::Unknown => "unknown",
    }
}

/// What "Copy for Agent" puts on the clipboard: a request, which chezmoi
/// and where its files are, what needs attention, and the full
/// `chezmoi doctor` table. Paths in the home directory are written with
/// `~`. `None` until diagnostics have run.
pub fn agent_report(info: &Info, state: &DoctorState, home: Option<&Path>) -> Option<String> {
    let home = home.and_then(Path::to_str).filter(|home| home.len() > 1);
    let short = |text: &str| match home {
        Some(home) => text.replace(home, "~"),
        None => text.to_string(),
    };
    let path = |path: &Path| short(&path.display().to_string());

    let mut s = String::new();
    s.push_str(match state {
        DoctorState::Done(_) => tr(
            "Please look at this `chezmoi doctor` result from my computer: what needs fixing, and how? Before changing any config or file, tell me what you would change and why.",
            "请帮我看看这份 `chezmoi doctor` 的结果：哪些需要处理、怎么处理？要改配置或文件之前，先告诉我改什么、为什么。",
        ),
        DoctorState::Failed(_) => tr(
            "Running `chezmoi doctor --no-network` on my computer failed. Please help me find out why.",
            "我在电脑上运行 `chezmoi doctor --no-network` 失败了，请帮我看看原因。",
        ),
        DoctorState::Idle | DoctorState::Running => return None,
    });
    s.push_str("\n\n");

    let chezmate = env!("CARGO_PKG_VERSION");
    let binary = path(&info.binary);
    let _ = writeln!(
        s,
        "{}",
        tr(
            format!("- Chezmate {chezmate}, chezmoi {} ({binary})", info.version),
            format!(
                "- Chezmate {chezmate}，chezmoi {}（{binary}）",
                info.version
            ),
        )
    );
    let source = path(&info.paths.source_dir);
    let git = if info.source_is_git_repo {
        tr(" (a Git repository)", "（Git 仓库）")
    } else {
        ""
    };
    let _ = writeln!(
        s,
        "{}",
        tr(
            format!("- Source directory: {source}{git}"),
            format!("- 源目录：{source}{git}"),
        )
    );
    let config = path(&info.paths.config_file);
    let mut config_line = tr(
        format!("- Config file: {config}"),
        format!("- 配置文件：{config}"),
    );
    if !info.config_exists {
        config_line.push_str(tr(" (not created)", "（尚未创建）"));
    }
    if let Some(template) = &info.config_template {
        let template = path(template);
        config_line.push_str(&tr(
            format!(", made by `chezmoi init` from {template}"),
            format!("，由 `chezmoi init` 按 {template} 生成"),
        ));
    }
    let _ = writeln!(s, "{config_line}");
    s.push('\n');

    match state {
        DoctorState::Done(checks) => {
            let problems: Vec<_> = checks
                .iter()
                .filter(|c| {
                    matches!(
                        c.result,
                        CheckResult::Warning | CheckResult::Error | CheckResult::Failed
                    )
                })
                .collect();
            if problems.is_empty() {
                let _ = writeln!(s, "{}", tr("No warnings or errors.", "没有警告或错误。"));
            } else {
                let _ = writeln!(
                    s,
                    "{}",
                    tr(
                        format!("Needs attention ({}):", problems.len()),
                        format!("需要注意（{} 项）：", problems.len()),
                    )
                );
                for check in problems {
                    let _ = writeln!(
                        s,
                        "- {} {}: {}",
                        result_word(check.result),
                        check.check,
                        short(&check.message)
                    );
                }
            }
            let _ = writeln!(
                s,
                "\n{}\n```text",
                tr(
                    "Full output of `chezmoi doctor --no-network`:",
                    "`chezmoi doctor --no-network` 的完整结果：",
                )
            );
            let width = checks
                .iter()
                .map(|c| c.check.chars().count())
                .chain(["CHECK".len()])
                .max()
                .unwrap_or_default();
            let _ = writeln!(s, "{:<8} {:<width$}  MESSAGE", "RESULT", "CHECK");
            for check in checks {
                let line = format!(
                    "{:<8} {:<width$}  {}",
                    result_word(check.result),
                    check.check,
                    short(&check.message)
                );
                let _ = writeln!(s, "{}", line.trim_end());
            }
            s.push_str("```\n");
        }
        DoctorState::Failed(error) => {
            let _ = writeln!(
                s,
                "{}\n```text\n{}\n```",
                tr("Error:", "错误："),
                short(error)
            );
        }
        DoctorState::Idle | DoctorState::Running => {}
    }
    Some(s)
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
        let ran = matches!(
            model.doctor(),
            DoctorState::Done(_) | DoctorState::Failed(_)
        );
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
                                    .when(ran, |row| {
                                        row.child(
                                            Button::new("copy-doctor")
                                                .ghost()
                                                .small()
                                                .icon(Icon::new(Lucide::Copy))
                                                .label(tr("Copy for Agent", "复制给 Agent"))
                                                .tooltip(tr(
                                                    "Copies the results with a short request, ready to paste into Claude Code, Codex or another agent. Paths in your home folder are written with ~.",
                                                    "复制诊断结果和一段说明，可以直接粘贴给 Claude Code、Codex 等 Agent。家目录里的路径写成 ~。",
                                                ))
                                                .on_click(cx.listener(|this, _, window, cx| {
                                                    this.copy_for_agent(window, cx)
                                                })),
                                        )
                                    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use chezmate_core::{DoctorCheck, Paths};
    use std::path::PathBuf;

    fn info() -> Info {
        Info {
            binary: "/opt/homebrew/bin/chezmoi".into(),
            version: "2.73.3".into(),
            paths: Paths {
                source_dir: "/Users/demo/.local/share/chezmoi".into(),
                dest_dir: "/Users/demo".into(),
                working_tree: "/Users/demo/.local/share/chezmoi".into(),
                config_file: "/Users/demo/.config/chezmoi/chezmoi.toml".into(),
            },
            source_exists: true,
            config_exists: true,
            config_template: Some("/Users/demo/.local/share/chezmoi/.chezmoi.toml.tmpl".into()),
            source_is_git_repo: true,
        }
    }

    fn check(result: CheckResult, check: &str, message: &str) -> DoctorCheck {
        DoctorCheck {
            result,
            check: check.into(),
            message: message.into(),
        }
    }

    #[test]
    fn the_agent_report_is_ready_to_paste() {
        let home = PathBuf::from("/Users/demo");
        assert_eq!(agent_report(&info(), &DoctorState::Idle, Some(&home)), None);

        let state = DoctorState::Done(vec![
            check(CheckResult::Ok, "version", "v2.73.3"),
            check(
                CheckResult::Warning,
                "suspicious-entries",
                "/Users/demo/.local/share/chezmoi/dot_zshrc.orig",
            ),
            check(CheckResult::Info, "diff-command", ""),
        ]);
        let report = agent_report(&info(), &state, Some(&home)).unwrap();
        assert!(!report.contains("/Users/demo"), "{report}");
        assert!(report.contains("~/.local/share/chezmoi"), "{report}");
        assert!(
            report.contains("~/.local/share/chezmoi/.chezmoi.toml.tmpl"),
            "{report}"
        );
        assert!(
            report
                .contains("- warning suspicious-entries: ~/.local/share/chezmoi/dot_zshrc.orig\n"),
            "{report}"
        );
        assert!(
            report.contains(concat!(
                "```text\n",
                "RESULT   CHECK               MESSAGE\n",
                "ok       version             v2.73.3\n",
                "warning  suspicious-entries  ~/.local/share/chezmoi/dot_zshrc.orig\n",
                "info     diff-command\n",
                "```\n",
            )),
            "{report}"
        );

        let failed = DoctorState::Failed("`chezmoi doctor` timed out after 30s".into());
        let report = agent_report(&info(), &failed, Some(&home)).unwrap();
        assert!(report.contains("```text\n`chezmoi doctor` timed out after 30s\n```"));
    }
}
