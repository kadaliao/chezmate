mod app;
mod assets;
mod changes;
mod config;
mod environment;
mod files;
mod i18n;
mod language;
mod model;
mod overview;
mod repository;
mod viewer;

use chezmate_core::ChezmoiOptions;
use gpui_kit::component::{Theme, TitleBar};
use gpui_kit::{AppContext as _, WindowBounds, WindowOptions, px, size};

fn main() {
    gpui_kit::application()
        .with_assets(assets::AppAssets)
        .run(|cx| {
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
                cx.new(|cx| app::Chezmate::new(ChezmoiOptions::default(), window, cx))
            })
            .expect("failed to open the Chezmate window");
            cx.activate(true);
        });
}

#[cfg(test)]
mod tests;
