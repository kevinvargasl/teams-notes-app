use gpui_kit::{
    App, Window,
    component::{Theme, ThemeMode},
    px, rgb, transparent_black,
};

pub(super) fn apply(dark: Option<bool>, window: &mut Window, cx: &mut App) {
    match dark {
        Some(true) => Theme::change(ThemeMode::Dark, Some(window), cx),
        Some(false) => Theme::change(ThemeMode::Light, Some(window), cx),
        None => Theme::sync_system_appearance(Some(window), cx),
    }

    Theme::update(cx, |theme| {
        // The 16-point base makes GPUI's rem scale match Spartan's control proportions.
        theme.font_size = px(16.);
        theme.radius = px(8.);
        theme.radius_lg = px(12.);
        theme.focus_ring = false;

        let dark = theme.mode.is_dark();
        let (canvas, surface, sidebar, foreground, muted, border, input_border, hover) = if dark {
            (
                0x111827, 0x1d2432, 0x171f2d, 0xf9fafb, 0x9ca3af, 0x303a4a, 0x586274, 0x292f3c,
            )
        } else {
            (
                0xffffff, 0xffffff, 0xf9fafb, 0x111827, 0x4b5563, 0xe5e7eb, 0x9ca3af, 0xf3f4f6,
            )
        };
        let [
            canvas,
            surface,
            sidebar,
            foreground,
            muted,
            border,
            input_border,
            hover,
        ] = [
            canvas,
            surface,
            sidebar,
            foreground,
            muted,
            border,
            input_border,
            hover,
        ]
        .map(|hex| rgb(hex).into());

        let accent = rgb(0xff6c0c).into();
        let on_accent = rgb(0xffffff).into();
        let primary_hover = rgb(0xff7e29).into();
        let primary_pressed = rgb(0xda5a0d).into();
        let colors = &mut theme.colors;

        colors.background = canvas;
        colors.foreground = foreground;
        colors.sidebar = sidebar;
        colors.sidebar_foreground = foreground;
        colors.sidebar_border = border;
        colors.border = border;
        colors.input = input_border;
        colors.muted = hover;
        colors.muted_foreground = muted;
        colors.popover = surface;
        colors.popover_foreground = foreground;
        colors.accent = hover;
        colors.accent_foreground = foreground;
        colors.button = surface;
        colors.button_foreground = foreground;
        colors.button_hover = hover;
        colors.button_active = hover;
        colors.secondary_active = transparent_black();
        colors.tab_foreground = muted;
        colors.tab_active_foreground = accent;
        colors.primary = accent;
        colors.primary_foreground = on_accent;
        colors.primary_hover = primary_hover;
        colors.primary_active = primary_pressed;
        colors.button_primary = accent;
        colors.button_primary_foreground = on_accent;
        colors.button_primary_hover = primary_hover;
        colors.button_primary_active = primary_pressed;
        colors.button_danger = rgb(0xdc2626).into();
        colors.button_danger_foreground = rgb(0xffffff).into();
        colors.button_danger_hover = rgb(0xb91c1c).into();
        colors.button_danger_active = rgb(0x991b1b).into();
        colors.slider_bar = accent;
        colors.ring = if dark { accent } else { rgb(0xc03d11).into() };
    });
}
