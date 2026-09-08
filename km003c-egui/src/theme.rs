use eframe::egui;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::preferences::SkinId;

// KM003C's instrument palette follows one rule: containers are grayscale and
// color belongs to measured channels. Keeping the tokens here prevents local
// widgets from slowly reintroducing decorative blues, greens or oranges.
pub(crate) const BACKPLANE: egui::Color32 = egui::Color32::from_rgb(0x0D, 0x11, 0x17);
pub(crate) const PANEL: egui::Color32 = egui::Color32::from_rgb(0x16, 0x1B, 0x22);
pub(crate) const PANEL_RAISED: egui::Color32 = egui::Color32::from_rgb(0x1C, 0x21, 0x28);
pub(crate) const DIVIDER: egui::Color32 = egui::Color32::from_rgb(0x30, 0x36, 0x3D);
pub(crate) const TEXT_PRIMARY: egui::Color32 = egui::Color32::from_rgb(0xE6, 0xED, 0xF3);
pub(crate) const TEXT_SECONDARY: egui::Color32 = egui::Color32::from_rgb(0x91, 0x98, 0xA1);
pub(crate) const TEXT_MUTED: egui::Color32 = egui::Color32::from_rgb(0x6E, 0x76, 0x81);
pub(crate) const VOLTAGE: egui::Color32 = egui::Color32::from_rgb(0x58, 0xA6, 0xFF);
pub(crate) const CURRENT: egui::Color32 = egui::Color32::from_rgb(0x3F, 0xB9, 0x50);
pub(crate) const POWER: egui::Color32 = egui::Color32::from_rgb(0xD2, 0x99, 0x22);
pub(crate) const ENERGY: egui::Color32 = egui::Color32::from_rgb(0xF7, 0x6E, 0xB6);
pub(crate) const CAPACITY: egui::Color32 = egui::Color32::from_rgb(0xA3, 0x7A, 0xF2);
pub(crate) const RECORDING: egui::Color32 = egui::Color32::from_rgb(0xF8, 0x51, 0x49);

/// Container and text colors for a complete UI skin.  Measurement colors are
/// intentionally not part of this palette: voltage/current/power keep their
/// stable semantic colors when the surrounding skin changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SkinPalette {
    pub(crate) backplane: egui::Color32,
    pub(crate) panel: egui::Color32,
    pub(crate) panel_raised: egui::Color32,
    pub(crate) divider: egui::Color32,
    pub(crate) text_primary: egui::Color32,
    pub(crate) text_secondary: egui::Color32,
    pub(crate) text_muted: egui::Color32,
    pub(crate) accent: egui::Color32,
    pub(crate) accent_soft: egui::Color32,
    pub(crate) hovered: egui::Color32,
    pub(crate) active: egui::Color32,
    /// Opacity of the optional full-window wallpaper. Industrial keeps this
    /// at zero so the default theme remains unchanged.
    pub(crate) wallpaper_alpha: u8,
    /// Dark veil drawn above the wallpaper before translucent panels are
    /// painted. This keeps measurements readable over a photographic image.
    pub(crate) wallpaper_overlay: egui::Color32,
}

impl SkinPalette {
    pub(crate) fn for_skin(skin: SkinId) -> Self {
        match skin {
            SkinId::Industrial => Self {
                backplane: BACKPLANE,
                panel: PANEL,
                panel_raised: PANEL_RAISED,
                divider: DIVIDER,
                text_primary: TEXT_PRIMARY,
                text_secondary: TEXT_SECONDARY,
                text_muted: TEXT_MUTED,
                accent: VOLTAGE,
                accent_soft: TEXT_SECONDARY,
                hovered: egui::Color32::from_rgb(0x24, 0x2A, 0x32),
                active: egui::Color32::from_rgb(0x2A, 0x30, 0x38),
                wallpaper_alpha: 0,
                wallpaper_overlay: egui::Color32::TRANSPARENT,
            },
            SkinId::CleanAnime => Self {
                // Low-saturation blue-gray surfaces keep the measurement
                // colors readable while giving the alternate skin its own
                // atmosphere.
                // The alpha lets the requested character wallpaper remain
                // visible across the whole workbench instead of appearing
                // only in a small decorative slot.
                backplane: egui::Color32::from_rgba_unmultiplied(0x0F, 0x15, 0x22, 0x90),
                panel: egui::Color32::from_rgba_unmultiplied(0x17, 0x21, 0x30, 0xB8),
                panel_raised: egui::Color32::from_rgba_unmultiplied(0x20, 0x2C, 0x3D, 0xD8),
                divider: egui::Color32::from_rgba_unmultiplied(0x3A, 0x4A, 0x60, 0xE0),
                text_primary: egui::Color32::from_rgb(0xEF, 0xF6, 0xFF),
                text_secondary: egui::Color32::from_rgb(0xAF, 0xC1, 0xD9),
                text_muted: egui::Color32::from_rgb(0x77, 0x89, 0xA3),
                accent: egui::Color32::from_rgb(0x78, 0xD9, 0xFF),
                accent_soft: egui::Color32::from_rgb(0xB7, 0xA7, 0xFF),
                hovered: egui::Color32::from_rgb(0x28, 0x3A, 0x52),
                active: egui::Color32::from_rgb(0x31, 0x46, 0x66),
                wallpaper_alpha: 225,
                wallpaper_overlay: egui::Color32::from_rgba_unmultiplied(0x07, 0x0C, 0x16, 0x58),
            },
        }
    }
}

static ACTIVE_SKIN: AtomicU8 = AtomicU8::new(SkinId::Industrial as u8);

pub(crate) fn set_active_skin(skin: SkinId) {
    ACTIVE_SKIN.store(skin as u8, Ordering::Relaxed);
}

pub(crate) fn active_skin() -> SkinId {
    match ACTIVE_SKIN.load(Ordering::Relaxed) {
        value if value == SkinId::CleanAnime as u8 => SkinId::CleanAnime,
        _ => SkinId::Industrial,
    }
}

pub(crate) fn palette() -> SkinPalette {
    SkinPalette::for_skin(active_skin())
}

pub(crate) fn backplane() -> egui::Color32 {
    palette().backplane
}

pub(crate) fn panel() -> egui::Color32 {
    palette().panel
}

pub(crate) fn panel_raised() -> egui::Color32 {
    palette().panel_raised
}

pub(crate) fn divider() -> egui::Color32 {
    palette().divider
}

pub(crate) fn text_primary() -> egui::Color32 {
    palette().text_primary
}

pub(crate) fn text_secondary() -> egui::Color32 {
    palette().text_secondary
}

pub(crate) fn text_muted() -> egui::Color32 {
    palette().text_muted
}

pub(crate) fn muted_text() -> egui::Color32 {
    palette().text_secondary
}

pub(crate) fn accent() -> egui::Color32 {
    palette().accent
}

pub(crate) fn accent_soft() -> egui::Color32 {
    palette().accent_soft
}

pub(crate) fn apply(ctx: &egui::Context, skin: SkinId) {
    set_active_skin(skin);
    let palette = SkinPalette::for_skin(skin);
    let mut visuals = egui::Visuals::dark();
    visuals.dark_mode = true;
    visuals.window_fill = palette.panel;
    visuals.panel_fill = palette.panel;
    visuals.extreme_bg_color = palette.backplane;
    visuals.faint_bg_color = palette.backplane;
    visuals.code_bg_color = palette.backplane;
    visuals.window_stroke = egui::Stroke::new(1.0, palette.divider);
    visuals.widgets.noninteractive.bg_fill = palette.panel;
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, palette.text_primary);
    visuals.widgets.inactive.bg_fill = palette.panel_raised;
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, palette.text_primary);
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, palette.divider);
    visuals.widgets.hovered.bg_fill = palette.hovered;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, palette.text_muted);
    visuals.widgets.active.bg_fill = palette.active;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, palette.text_secondary);
    visuals.widgets.open.bg_fill = palette.panel_raised;
    visuals.widgets.open.bg_stroke = egui::Stroke::new(1.0, palette.text_muted);
    visuals.selection.bg_fill = palette.text_secondary.gamma_multiply(0.22);
    visuals.selection.stroke = egui::Stroke::new(1.0, palette.text_secondary);
    visuals.hyperlink_color = palette.text_primary;
    ctx.set_visuals(visuals);

    ctx.set_theme(egui::Theme::Dark);
    ctx.global_style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 6.0);
        style.visuals.window_corner_radius = egui::CornerRadius::same(8);
        style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
        style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
        style.visuals.widgets.active.corner_radius = egui::CornerRadius::same(6);
        style.visuals.widgets.open.corner_radius = egui::CornerRadius::same(6);
        style
            .text_styles
            .insert(egui::TextStyle::Heading, egui::FontId::proportional(16.0));
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(13.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(13.0));
        style
            .text_styles
            .insert(egui::TextStyle::Monospace, egui::FontId::monospace(13.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, egui::FontId::proportional(11.0));
    });

    // egui-system-fonts resolves the platform's installed fallback stack. On
    // macOS this includes PingFang SC before the Latin fallback, which keeps
    // Chinese labels readable without bundling a font or changing licensing.
    #[cfg(target_os = "macos")]
    {
        egui_system_fonts::set_with_presets(
            ctx,
            [
                egui_system_fonts::FontPreset::SimplifiedChinese,
                egui_system_fonts::FontPreset::Latin,
            ],
            egui_system_fonts::FontStyle::Sans,
        );
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn simplified_chinese_system_font_is_available() {
        let ctx = egui::Context::default();
        let found = egui_system_fonts::set_with_presets(
            &ctx,
            [
                egui_system_fonts::FontPreset::SimplifiedChinese,
                egui_system_fonts::FontPreset::Latin,
            ],
            egui_system_fonts::FontStyle::Sans,
        );
        assert!(!found.is_empty(), "macOS font resolver returned no candidates");
    }

    #[test]
    fn instrument_design_tokens_do_not_drift() {
        assert_eq!(BACKPLANE, egui::Color32::from_rgb(0x0D, 0x11, 0x17));
        assert_eq!(PANEL, egui::Color32::from_rgb(0x16, 0x1B, 0x22));
        assert_eq!(DIVIDER, egui::Color32::from_rgb(0x30, 0x36, 0x3D));
        assert_eq!(VOLTAGE, egui::Color32::from_rgb(0x58, 0xA6, 0xFF));
        assert_eq!(CURRENT, egui::Color32::from_rgb(0x3F, 0xB9, 0x50));
        assert_eq!(POWER, egui::Color32::from_rgb(0xD2, 0x99, 0x22));
        assert_eq!(ENERGY, egui::Color32::from_rgb(0xF7, 0x6E, 0xB6));
        assert_eq!(CAPACITY, egui::Color32::from_rgb(0xA3, 0x7A, 0xF2));
        assert_eq!(RECORDING, egui::Color32::from_rgb(0xF8, 0x51, 0x49));
    }

    #[test]
    fn skins_only_change_containers_not_measurement_semantics() {
        let industrial = SkinPalette::for_skin(SkinId::Industrial);
        let clean = SkinPalette::for_skin(SkinId::CleanAnime);
        assert_eq!(industrial.backplane, BACKPLANE);
        assert_eq!(industrial.panel, PANEL);
        assert_eq!(industrial.divider, DIVIDER);
        assert_ne!(clean.backplane, industrial.backplane);
        assert_ne!(clean.panel_raised, industrial.panel_raised);
        assert_eq!(industrial.wallpaper_alpha, 0);
        assert!(clean.wallpaper_alpha > 0);
        assert_eq!(VOLTAGE, egui::Color32::from_rgb(0x58, 0xA6, 0xFF));
        assert_eq!(CURRENT, egui::Color32::from_rgb(0x3F, 0xB9, 0x50));
        assert_eq!(POWER, egui::Color32::from_rgb(0xD2, 0x99, 0x22));
    }
}
