mod agent_options;
mod app;
mod audio;
mod library;
mod processing;

use anyhow::Result;
use gpui_kit::{
    AppContext, AssetSource, Bounds, KeyBinding, SharedString, TitlebarOptions, WindowBounds,
    WindowOptions,
    assets::{Assets, icon_assets},
    px, size,
};
use std::borrow::Cow;

icon_assets!(AppIcons, [Pencil, Trash, BookmarkPlus, Sun, Moon, Check]);

struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(icon) = AppIcons.load(path)? {
            return Ok(Some(icon));
        }
        Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = Assets.list(path)?;
        paths.extend(AppIcons.list(path)?);
        Ok(paths)
    }
}

fn main() -> Result<()> {
    let library = library::Library::open()?;
    let settings = library.settings()?;
    let meetings = library.meetings()?;
    gpui_kit::application()
        .with_assets(AppAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.bind_keys([
                KeyBinding::new("cmd-shift-m", app::AddMarker, Some("Reuniones")),
                KeyBinding::new("cmd-alt-up", app::MoveMeetingUp, Some("MeetingRow")),
                KeyBinding::new("cmd-alt-down", app::MoveMeetingDown, Some("MeetingRow")),
            ]);
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
