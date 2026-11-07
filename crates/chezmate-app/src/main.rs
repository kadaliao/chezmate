mod app;
mod assets;
mod changes;
mod config;
mod demo;
mod environment;
mod files;
mod i18n;
mod language;
mod model;
mod overview;
mod repository;
mod setup;
mod viewer;

use chezmate_core::ChezmoiOptions;
use gpui_kit::component::{Theme, TitleBar};
use gpui_kit::{AppContext as _, WindowBounds, WindowOptions, px, size};

fn main() {
    // `--demo` (or CHEZMATE_DEMO=1) opens a made-up setup in a temporary
    // folder instead of the real one.
    let demo = std::env::args().any(|arg| arg == "--demo")
        || std::env::var_os("CHEZMATE_DEMO").is_some_and(|v| v == "1");
    let chezmoi = if demo {
        match demo::create() {
            Ok(options) => options,
            Err(err) => {
                eprintln!("chezmate: could not set up demo mode: {err:#}");
                std::process::exit(1);
            }
        }
    } else {
        ChezmoiOptions {
            fallback_binary: bundled_chezmoi(),
            ..Default::default()
        }
    };
    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            i18n::init();
            app::init(cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1180.), px(760.)), cx)),
                window_min_size: Some(size(px(860.), px(520.))),
                app_id: Some("io.github.kadaliao.chezmate".into()),
                ..TitleBar::window_options()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                Theme::sync_system_appearance(Some(window), cx);
                cx.new(|cx| app::Chezmate::new(chezmoi.clone(), window, cx).demo(demo))
            })
            .expect("failed to open the Chezmate window");
            cx.activate(true);
        });
}

/// The chezmoi shipped inside Chezmate.app (`Contents/Helpers/chezmoi`),
/// used when none is installed.
fn bundled_chezmoi() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    let helper = exe.parent()?.parent()?.join("Helpers/chezmoi");
    helper.is_file().then_some(helper)
}

#[cfg(test)]
mod tests;
