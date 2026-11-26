mod agent_options;
mod app;
mod audio;
mod library;
mod processing;

use anyhow::{Context as _, Result};
use gpui_kit::{
    AppContext, AssetSource, Bounds, KeyBinding, Menu, MenuItem, QuitMode, SharedString,
    TitlebarOptions, WindowBounds, WindowOptions,
    assets::{Assets, icon_assets},
    px, size,
};
use std::borrow::Cow;

gpui_kit::actions!(reuniones, [Quit]);

icon_assets!(
    AppIcons,
    [
        Pencil,
        Trash,
        BookmarkPlus,
        Sun,
        Moon,
        Check,
        CircleCheck,
        ListTodo,
        CircleQuestionMark,
        ArrowUpToLine,
        Search,
        ChevronUp,
        ChevronDown,
        X
    ]
);

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
        .with_quit_mode(QuitMode::LastWindowClosed)
        .run(move |cx| {
            gpui_kit::init(cx);
            if let Err(error) = set_app_icon() {
                eprintln!("No se pudo cargar el icono de Reuniones: {error:#}");
            }
            cx.bind_keys([
                KeyBinding::new("cmd-q", Quit, None),
                KeyBinding::new("cmd-shift-m", app::AddMarker, Some("Reuniones")),
                KeyBinding::new("cmd-f", app::FindTranscript, Some("Reuniones")),
                KeyBinding::new("cmd-g", app::NextTranscriptMatch, Some("Reuniones")),
                KeyBinding::new(
                    "cmd-shift-g",
                    app::PreviousTranscriptMatch,
                    Some("Reuniones"),
                ),
                KeyBinding::new(
                    "escape",
                    gpui_kit::component::input::Escape,
                    Some("TranscriptSearch"),
                ),
                KeyBinding::new("cmd-alt-up", app::MoveMeetingUp, Some("MeetingRow")),
                KeyBinding::new("cmd-alt-down", app::MoveMeetingDown, Some("MeetingRow")),
            ]);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.set_menus([
                Menu::new("Reuniones").items([MenuItem::action("Salir de Reuniones", Quit)])
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

fn set_app_icon() -> Result<()> {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;

    let main_thread = MainThreadMarker::new().context("El icono requiere el hilo principal")?;
    let data = NSData::with_bytes(include_bytes!("../assets/AppIcon.icns"));
    let image = NSImage::initWithData(NSImage::alloc(), &data)
        .context("El archivo del icono no es válido")?;
    // SAFETY: The image is valid and non-null, and AppKit runs on the main thread.
    unsafe {
        NSApplication::sharedApplication(main_thread).setApplicationIconImage(Some(&image));
    }
    Ok(())
}
