//! Minimal, light "note app" look for egui.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Vec2,
};

#[derive(Clone, Copy)]
pub struct Palette {
    pub canvas: Color32,
    pub surface: Color32,
    pub surface_alt: Color32,
    pub border: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub danger: Color32,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            canvas: Color32::from_rgb(18, 18, 21),
            surface: Color32::from_rgb(32, 33, 37),
            surface_alt: Color32::from_rgb(44, 45, 51),
            border: Color32::from_rgb(52, 53, 60),
            text: Color32::from_rgb(236, 236, 240),
            weak: Color32::from_rgb(150, 152, 162),
            accent: Color32::from_rgb(110, 155, 255),
            accent_soft: Color32::from_rgb(40, 52, 82),
            danger: Color32::from_rgb(255, 110, 100),
        }
    } else {
        Palette {
            canvas: Color32::from_rgb(238, 239, 243),
            surface: Color32::WHITE,
            surface_alt: Color32::from_rgb(244, 245, 248),
            border: Color32::from_rgb(226, 227, 232),
            text: Color32::from_rgb(30, 31, 36),
            weak: Color32::from_rgb(132, 135, 145),
            accent: Color32::from_rgb(40, 104, 245),
            accent_soft: Color32::from_rgb(228, 236, 255),
            danger: Color32::from_rgb(214, 54, 44),
        }
    }
}

pub fn apply(ctx: &egui::Context, dark: bool) {
    let pal = palette(dark);
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.panel_fill = pal.surface;
    v.window_fill = pal.surface;
    v.extreme_bg_color = pal.surface_alt;
    v.faint_bg_color = pal.surface_alt;
    v.override_text_color = Some(pal.text);
    v.window_stroke = Stroke::new(1.0, pal.border);
    v.window_corner_radius = CornerRadius::same(16);
    v.menu_corner_radius = CornerRadius::same(12);
    let shadow = Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 120 } else { 38 }),
    };
    v.window_shadow = shadow;
    v.popup_shadow = shadow;
    v.selection.bg_fill = pal.accent.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.5, pal.accent);
    v.hyperlink_color = pal.accent;
    v.slider_trailing_fill = true;
    v.text_cursor.stroke = Stroke::new(2.0, pal.accent);
    let r = CornerRadius::same(10);
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, pal.border);
    w.noninteractive.corner_radius = r;
    w.noninteractive.fg_stroke = Stroke::new(1.0, pal.text);
    for (state, bg) in [
        (&mut w.inactive, pal.surface_alt),
        (&mut w.hovered, pal.border),
        (&mut w.active, pal.accent_soft),
        (&mut w.open, pal.surface_alt),
    ] {
        state.bg_fill = bg;
        state.weak_bg_fill = bg;
        state.bg_stroke = Stroke::NONE;
        state.corner_radius = r;
        state.fg_stroke = Stroke::new(1.5, pal.text);
        state.expansion = 0.0;
    }
    w.hovered.bg_stroke = Stroke::NONE;
    ctx.set_visuals(v);

    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = Vec2::new(12.0, 6.0);
        s.spacing.interact_size.y = 30.0;
        s.spacing.menu_margin = Margin::same(8);
        s.spacing.window_margin = Margin::same(16);
        s.spacing.slider_width = 180.0;
        s.text_styles = [
            (
                TextStyle::Heading,
                FontId::new(20.0, FontFamily::Proportional),
            ),
            (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
            (
                TextStyle::Button,
                FontId::new(14.5, FontFamily::Proportional),
            ),
            (
                TextStyle::Small,
                FontId::new(12.0, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(13.0, FontFamily::Monospace),
            ),
        ]
        .into();
    });
}
