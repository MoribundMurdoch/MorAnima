#![allow(non_snake_case)]

mod app;
mod effects;

use dioxus::desktop::tao::dpi::LogicalSize;
use dioxus::desktop::tao::window::Icon;
use dioxus::desktop::{Config, WindowBuilder};

fn window_icon() -> Option<Icon> {
    let img = image::load_from_memory(include_bytes!("../assets/icons/moranima-128.png"))
        .ok()?
        .to_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).ok()
}

fn main() {
    let cfg = Config::new()
        .with_menu(None)
        .with_window(
            WindowBuilder::new()
                .with_title("MorAnima")
                .with_inner_size(LogicalSize::new(1280.0, 840.0))
                .with_decorations(false)
                .with_window_icon(window_icon()),
        );
    dioxus::LaunchBuilder::new().with_cfg(cfg).launch(app::App);
}
