mod app;
mod audio;
mod library;
mod processing;

use anyhow::Result;
use gpui_kit::{
    AppContext, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, px, size,
};

fn main() -> Result<()> {
    let library = library::Library::open()?;
    let settings = library.settings()?;
    let meetings = library.meetings()?;
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.bind_keys([KeyBinding::new(
                "cmd-shift-m",
                app::AddMarker,
                Some("Reuniones"),
            )]);
            let bounds = Bounds::centered(None, size(px(1000.), px(720.)), cx);
            let options = WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("Reuniones".into()),
                    appears_transparent: false,
                    traffic_light_position: None,
                }),
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(740.), px(500.))),
                ..Default::default()
            };
            if let Err(error) = gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| app::Workspace::new(library, settings, meetings, window, cx))
            }) {
                eprintln!("No se pudo abrir Reuniones: {error:#}");
                cx.quit();
                return;
            }
            cx.activate(true);
        });
    Ok(())
}
