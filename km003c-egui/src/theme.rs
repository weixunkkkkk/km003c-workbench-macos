use eframe::egui;

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
pub(crate) const MUTED_TEXT: egui::Color32 = TEXT_SECONDARY;
pub(crate) const VOLTAGE: egui::Color32 = egui::Color32::from_rgb(0x58, 0xA6, 0xFF);
pub(crate) const CURRENT: egui::Color32 = egui::Color32::from_rgb(0x3F, 0xB9, 0x50);
pub(crate) const POWER: egui::Color32 = egui::Color32::from_rgb(0xD2, 0x99, 0x22);
pub(crate) const RECORDING: egui::Color32 = egui::Color32::from_rgb(0xF8, 0x51, 0x49);

pub(crate) fn apply(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.dark_mode = true;
    visuals.window_fill = PANEL;
    visuals.panel_fill = PANEL;
    visuals.extreme_bg_color = BACKPLANE;
    visuals.faint_bg_color = BACKPLANE;
    visuals.code_bg_color = BACKPLANE;
    visuals.window_stroke = egui::Stroke::new(1.0, DIVIDER);
    visuals.widgets.noninteractive.bg_fill = PANEL;
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.inactive.bg_fill = PANEL_RAISED;
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, TEXT_PRIMARY);
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, DIVIDER);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(0x24, 0x2A, 0x32);
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, TEXT_MUTED);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(0x2A, 0x30, 0x38);
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, TEXT_SECONDARY);
    visuals.widgets.open.bg_fill = PANEL_RAISED;
    visuals.widgets.open.bg_stroke = egui::Stroke::new(1.0, TEXT_MUTED);
    visuals.selection.bg_fill = TEXT_SECONDARY.gamma_multiply(0.22);
    visuals.selection.stroke = egui::Stroke::new(1.0, TEXT_SECONDARY);
    visuals.hyperlink_color = TEXT_PRIMARY;
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
        assert_eq!(RECORDING, egui::Color32::from_rgb(0xF8, 0x51, 0x49));
    }
}
