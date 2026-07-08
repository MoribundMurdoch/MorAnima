#![allow(non_snake_case)]

mod app;
mod effects;

use dioxus::desktop::tao::dpi::LogicalSize;
use dioxus::desktop::{Config, WindowBuilder};

fn main() {
    let cfg = Config::new()
        .with_menu(None)
        .with_window(
            WindowBuilder::new()
                .with_title("MorAnima")
                .with_inner_size(LogicalSize::new(1280.0, 840.0))
                .with_decorations(false),
        );
    dioxus::LaunchBuilder::new().with_cfg(cfg).launch(app::App);
}
