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
}

/// Installs the system Chinese font next to egui's own faces, without
/// bundling a font or changing licensing.
///
/// - Proportional text starts with the Chinese system face. PingFang carries
///   Latin glyphs as well and is what native macOS UI draws Chinese text with.
/// - Monospace text keeps egui's Hack first, so every digit of an instrument
///   readout shares one advance; Chinese characters fall back to the system
///   face at the end of the stack. A proportional face in front of Hack made
///   readings shift sideways as their digits changed.
///
/// - The arrows, box-drawing marks, ● and Ⅱ used in labels and chart
///   legends exist in neither PingFang's UI face nor egui's Ubuntu-Light, so
///   proportional text falls back to egui's Hack before the emoji faces;
///   that also keeps ▶ and ● in text rather than emoji presentation.
///
/// Fonts never depend on the skin, so this runs once at startup instead of
/// on every skin change: egui compares replaced font definitions byte by byte.
pub(crate) fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    add_symbol_fallback(&mut fonts);
    #[cfg(target_os = "macos")]
    {
        let mut chinese = macos_fonts::simplified_chinese().into_iter();
        match chinese.next() {
            Some((primary, mut data)) => {
                // Glyphs the Chinese face lacks are drawn by egui's own faces;
                // put the Chinese face on their baseline.
                if let Some(reference) = fonts
                    .families
                    .get(&egui::FontFamily::Proportional)
                    .and_then(|chain| chain.first())
                    .and_then(|name| fonts.font_data.get(name))
                {
                    data.tweak.y_offset_factor += macos_fonts::baseline_shift(&data, reference);
                }
                fonts.font_data.insert(primary.clone(), std::sync::Arc::new(data));
                fonts
                    .families
                    .entry(egui::FontFamily::Proportional)
                    .or_default()
                    .insert(0, primary.clone());
                fonts
                    .families
                    .entry(egui::FontFamily::Monospace)
                    .or_default()
                    .push(primary);
            }
            None => tracing::warn!("No Simplified Chinese system font found; Chinese labels cannot be drawn"),
        }
        // A second Chinese face for characters the first one lacks.
        for (fallback, data) in chinese {
            fonts.font_data.insert(fallback.clone(), std::sync::Arc::new(data));
            insert_before_emoji(
                fonts.families.entry(egui::FontFamily::Proportional).or_default(),
                &fallback,
            );
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push(fallback);
        }
    }
    ctx.set_fonts(fonts);
}

/// Puts egui's monospace face into the proportional stack, ahead of the
/// emoji faces, as the fallback for symbols the text faces lack.
fn add_symbol_fallback(fonts: &mut egui::FontDefinitions) {
    const SYMBOL_FACE: &str = "Hack";
    if fonts.font_data.contains_key(SYMBOL_FACE) {
        insert_before_emoji(
            fonts.families.entry(egui::FontFamily::Proportional).or_default(),
            SYMBOL_FACE,
        );
    }
}

/// Appends `face` to a fallback chain, but ahead of the emoji faces, so
/// characters with a text glyph do not get emoji presentation.
fn insert_before_emoji(chain: &mut Vec<String>, face: &str) {
    if chain.iter().any(|name| name == face) {
        return;
    }
    let before_emoji = chain
        .iter()
        .position(|name| name.to_ascii_lowercase().contains("emoji"))
        .unwrap_or(chain.len());
    chain.insert(before_emoji, face.to_owned());
}

/// Locates a macOS Simplified Chinese system font that egui can draw.
///
/// PingFang left /System/Library/Fonts for a downloadable MobileAsset
/// (macOS 10.15–15). On macOS 26 and later the only PingFang left is
/// FontServices' reserved `PingFangUI.ttc`, whose outlines use a format only
/// CoreText reads: egui measures its glyphs and draws nothing, which blanks
/// every proportional label. A face is therefore used only if it carries
/// TrueType or PostScript outlines; Hiragino Sans GB and Heiti SC, which ship
/// in /System/Library/Fonts, cover the releases without a drawable PingFang.
#[cfg(target_os = "macos")]
mod macos_fonts {
    use std::path::{Path, PathBuf};

    use eframe::egui::FontData;

    /// Families in order of preference: the system face first, then faces
    /// that have shipped in /System/Library/Fonts for many releases.
    pub(super) const FAMILIES: [&str; 3] = ["PingFang SC", "Hiragino Sans GB", "Heiti SC"];

    const SYSTEM_FONTS: [&str; 4] = [
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
    ];
    const ASSETS: &str = "/System/Library/AssetsV2";

    /// Existing files that may contain one of [`FAMILIES`]. Listing a few
    /// known locations is far cheaper than scanning every installed font.
    pub(super) fn candidate_files() -> Vec<PathBuf> {
        let mut files = downloadable_pingfang();
        files.extend(SYSTEM_FONTS.iter().map(PathBuf::from));
        files.retain(|path| path.is_file());
        files
    }

    /// How far, as a fraction of the font size, epaint draws a fallback face's
    /// baseline below that of the `chinese` face leading the stack, for a
    /// fallback with the line box of `reference`.
    ///
    /// epaint centres each fallback face's line box inside the leading face's
    /// (`text_layout.rs`: `pos.y = face ascent + ½(row height − face row
    /// height)`). Hiragino Sans GB has a 0.5 em line gap, so every glyph it
    /// lacks (µ, − and ▶ in this UI) came out about 0.23 em low and "D−"
    /// read as "D_". egui's faces, Hack and Heiti SC have nearly the same
    /// line box, so moving the Chinese face down by this amount puts it on
    /// their shared baseline. That holds in the proportional stack, which it
    /// leads, and in the monospace stack, where it trails Hack and sat as far
    /// too high; it also centres Chinese text in its tall rows. A FontTweak
    /// offset moves the drawn glyphs only, not the layout.
    pub(super) fn baseline_shift(chinese: &FontData, reference: &FontData) -> f32 {
        match (LineBox::of(chinese), LineBox::of(reference)) {
            (Some(chinese), Some(reference)) => {
                reference.ascent + 0.5 * (chinese.height() - reference.height()) - chinese.ascent
            }
            _ => 0.0,
        }
    }

    /// Vertical line metrics in ems, chosen as skrifa (and so epaint) does:
    /// OS/2 typographic metrics when the face asks for them, else `hhea`.
    struct LineBox {
        ascent: f32,
        descent: f32,
        line_gap: f32,
    }

    impl LineBox {
        fn of(data: &FontData) -> Option<Self> {
            let face = ttf_parser::Face::parse(&data.font, data.index).ok()?;
            let em = f32::from(face.units_per_em());
            Some(Self {
                ascent: f32::from(face.ascender()) / em,
                descent: f32::from(face.descender()) / em,
                line_gap: f32::from(face.line_gap()) / em,
            })
        }

        fn height(&self) -> f32 {
            self.ascent - self.descent + self.line_gap
        }
    }

    /// Whether the face has outlines egui can rasterise: TrueType (`glyf`)
    /// or PostScript (`CFF `, `CFF2`).
    pub(super) fn has_drawable_outlines(data: &[u8], index: u32) -> bool {
        ttf_parser::RawFace::parse(data, index).is_ok_and(|face| {
            [b"glyf", b"CFF ", b"CFF2"]
                .iter()
                .any(|tag| face.table(ttf_parser::Tag::from_bytes(tag)).is_some())
        })
    }

    /// PingFang delivered as a MobileAsset:
    /// `AssetsV2/com_apple_MobileAsset_Font*/<asset>/AssetData/PingFang*.ttc`.
    fn downloadable_pingfang() -> Vec<PathBuf> {
        let children = |dir: &Path| {
            std::fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .collect::<Vec<_>>()
        };
        let name_starts_with = |path: &Path, prefix: &str| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(prefix))
        };
        children(Path::new(ASSETS))
            .into_iter()
            .filter(|kind| name_starts_with(kind, "com_apple_MobileAsset_Font"))
            .flat_map(|kind| children(&kind))
            .flat_map(|asset| children(&asset.join("AssetData")))
            .filter(|font| name_starts_with(font, "PingFang"))
            .collect()
    }

    /// Up to two available faces in [`FAMILIES`] order, named for egui's
    /// font definitions: the primary face and a fuller fallback.
    pub(crate) fn simplified_chinese() -> Vec<(String, FontData)> {
        let mut database = fontdb::Database::new();
        for file in candidate_files() {
            if let Err(error) = database.load_font_file(&file) {
                tracing::debug!("Skipping font {}: {error}", file.display());
            }
        }
        FAMILIES
            .iter()
            .filter_map(|&family| {
                let id = database.query(&fontdb::Query {
                    families: &[fontdb::Family::Name(family)],
                    ..fontdb::Query::default()
                })?;
                if !database.with_face_data(id, has_drawable_outlines)? {
                    tracing::info!(family, "Skipping Chinese font without drawable outlines");
                    return None;
                }
                let (source, index) = database.face_source(id)?;
                let path = match source {
                    fontdb::Source::File(path) | fontdb::Source::SharedFile(path, _) => path,
                    fontdb::Source::Binary(_) => return None,
                };
                let mut data = FontData::from_static(map_font_file(&path)?);
                data.index = index;
                tracing::info!(family, path = %path.display(), index, "Using Chinese system font");
                Some((format!("macos-system:{family}"), data))
            })
            .take(2)
            .collect()
    }

    /// Maps a font collection instead of copying it to the heap. PingFang is
    /// a ~60 MB collection; egui only touches the tables and glyphs it draws,
    /// so clean, file-backed pages replace that much resident memory. The
    /// mapping is leaked on purpose: installed fonts live as long as the app.
    fn map_font_file(path: &Path) -> Option<&'static [u8]> {
        let file = std::fs::File::open(path).ok()?;
        // SAFETY: these are system font files on the sealed, read-only system
        // volume, or MobileAsset files that updates replace rather than
        // rewrite. Their contents cannot change while mapped.
        let map = unsafe { memmap2::Mmap::map(&file) }
            .inspect_err(|error| tracing::debug!("Could not map font {}: {error}", path.display()))
            .ok()?;
        Some(&Box::leak(Box::new(map))[..])
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// Labels the workbench shows in its default Simplified Chinese UI.
    const CHINESE_SAMPLE: &str = "工作台电压电流功率录制设置";

    /// The characters of `text` that the `font_id` stack cannot draw.
    ///
    /// `has_glyph` is not used: egui answers it by comparing the owning face
    /// with the face holding the replacement character, so every character of
    /// that face (Hack, for monospace) reads as missing. A missing character
    /// resolves to the replacement face without a glyph and has zero width.
    fn missing_glyphs(ctx: &egui::Context, font_id: &egui::FontId, text: &str) -> String {
        // Pending font changes are applied at the start of the next pass.
        let _ = ctx.run_ui(egui::RawInput::default(), |_| {});
        ctx.fonts_mut(|fonts| {
            text.chars()
                .filter(|&c| c != ' ' && fonts.glyph_width(font_id, c) <= 0.0)
                .collect()
        })
    }

    /// "A font was found" is not enough: on macOS 26+ the resolver found only
    /// Latin faces and every Chinese label rendered without glyphs.
    #[test]
    fn installed_fonts_draw_the_chinese_and_latin_ui() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        apply(&ctx, SkinId::Industrial);
        for font_id in [egui::FontId::proportional(13.0), egui::FontId::monospace(13.0)] {
            for text in [CHINESE_SAMPLE, "Voltage 12.345 V · 1.234 A"] {
                let missing = missing_glyphs(&ctx, &font_id, text);
                assert!(missing.is_empty(), "{font_id:?} has no glyph for: {missing}");
            }
        }
    }

    /// Every non-ASCII character the UI sources can show (comment lines
    /// skipped) must have a glyph in both stacks: symbols such as µ, ·, ●, ○
    /// or arrows have no Latin system font to fall back on any more.
    #[test]
    fn every_ui_character_has_a_glyph() {
        const SOURCES: [&str; 9] = [
            include_str!("main.rs"),
            include_str!("i18n.rs"),
            include_str!("preferences.rs"),
            include_str!("measurement.rs"),
            include_str!("chart_view.rs"),
            include_str!("pd_decoder.rs"),
            include_str!("pd_trace_view.rs"),
            include_str!("offline_view.rs"),
            include_str!("recording_session.rs"),
        ];
        let mut characters: Vec<char> = SOURCES
            .iter()
            .flat_map(|source| source.lines())
            .filter(|line| !line.trim_start().starts_with("//"))
            .flat_map(str::chars)
            .filter(|character| !character.is_ascii())
            .collect();
        characters.sort_unstable();
        characters.dedup();

        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |_| {});
        let text: String = characters.into_iter().collect();
        for font_id in [egui::FontId::proportional(13.0), egui::FontId::monospace(13.0)] {
            let missing = missing_glyphs(&ctx, &font_id, &text);
            assert!(missing.is_empty(), "{font_id:?} has no glyph for: {missing}");
        }
    }

    /// Instrument readouts use the monospace style. Every character a reading
    /// can contain must share one advance, or a changing value shifts sideways.
    #[test]
    fn monospace_readout_characters_share_one_advance() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |_| {});
        let font_id = egui::FontId::monospace(34.0);
        let widths: Vec<(char, f32)> = ctx.fonts_mut(|fonts| {
            "0123456789.-+ "
                .chars()
                .map(|c| (c, fonts.glyph_width(&font_id, c)))
                .collect()
        });
        let first = widths[0].1;
        assert!(
            widths.iter().all(|(_, width)| (width - first).abs() < 0.01),
            "monospace advances differ: {widths:?}"
        );
    }

    #[test]
    fn skin_changes_keep_the_installed_fonts() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        apply(&ctx, SkinId::Industrial);
        let font_id = egui::FontId::proportional(13.0);
        assert_eq!(missing_glyphs(&ctx, &font_id, CHINESE_SAMPLE), "");
        apply(&ctx, SkinId::CleanAnime);
        assert_eq!(missing_glyphs(&ctx, &font_id, CHINESE_SAMPLE), "");
    }

    /// Faces come in preference order, and a primary plus a fallback are
    /// always available: Hiragino Sans GB and Heiti SC ship with macOS.
    #[test]
    fn chinese_faces_follow_the_preference_order() {
        let names: Vec<String> = macos_fonts::simplified_chinese()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names.len(), 2, "expected a primary and a fallback face, got {names:?}");
        let rank = |name: &String| {
            macos_fonts::FAMILIES
                .iter()
                .position(|family| name.ends_with(family))
                .expect("a known family")
        };
        assert!(rank(&names[0]) < rank(&names[1]), "faces out of order: {names:?}");
    }

    /// Measuring is not drawing: PingFang's UI face on macOS 26+ reported
    /// glyph widths yet produced no pixels, blanking every proportional label.
    /// Every glyph of real labels must reach the font atlas.
    #[test]
    fn chinese_latin_and_symbol_labels_rasterize() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |_| {});
        let text = format!("{CHINESE_SAMPLE} Voltage 12.345 V · ● Ⅱ ↓ ┄");
        for font_id in [egui::FontId::proportional(13.0), egui::FontId::monospace(13.0)] {
            let galley =
                ctx.fonts_mut(|fonts| fonts.layout_no_wrap(text.clone(), font_id.clone(), egui::Color32::WHITE));
            let blank: String = galley
                .rows
                .iter()
                .flat_map(|row| row.row.glyphs.iter())
                .filter(|glyph| {
                    !glyph.chr.is_whitespace() && (glyph.uv_rect.size.x <= 0.0 || glyph.uv_rect.size.y <= 0.0)
                })
                .map(|glyph| glyph.chr)
                .collect();
            assert!(blank.is_empty(), "{font_id:?} drew nothing for: {blank}");
        }
    }

    /// Characters the Chinese face lacks come from fallback faces; they and
    /// the Chinese glyphs must share one baseline in both stacks. Hiragino
    /// Sans GB's line gap once dropped the fallbacks about 0.23 em, so the
    /// minus of "D−" sat on the baseline and the label read "D_", while
    /// Chinese text in the monospace stack sat as far too high.
    #[test]
    fn fallback_symbols_and_chinese_share_the_text_baseline() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |_| {});
        for font_id in [
            egui::FontId::proportional(13.0),
            egui::FontId::proportional(34.0),
            egui::FontId::monospace(13.0),
            egui::FontId::monospace(34.0),
        ] {
            let size = font_id.size;
            let galley =
                ctx.fonts_mut(|fonts| fonts.layout_no_wrap("D−▶电".to_owned(), font_id.clone(), egui::Color32::WHITE));
            let drawn = |chr: char| {
                let glyph = galley.rows[0].row.glyphs.iter().find(|glyph| glyph.chr == chr).unwrap();
                let top = glyph.pos.y + glyph.uv_rect.offset.y;
                (top, top + glyph.uv_rect.size.y)
            };
            // Pixel snapping moves glyphs by up to a pixel.
            let tolerance = 0.1 * size + 0.5;
            let (capital_top, baseline) = drawn('D');
            let quarter = (baseline - capital_top) / 4.0;
            let (top, bottom) = drawn('−');
            assert!(
                (capital_top + quarter..=baseline - quarter).contains(&((top + bottom) / 2.0)),
                "{font_id:?}: − is drawn at {top}..{bottom}, D at {capital_top}..{baseline}"
            );
            let (top, bottom) = drawn('▶');
            assert!(
                (bottom - baseline).abs() <= tolerance,
                "{font_id:?}: ▶ is drawn at {top}..{bottom}, the baseline is {baseline}"
            );
            // An ideograph's design box runs from about -0.12 to 0.88 em.
            let (top, bottom) = drawn('电');
            let centre_above_baseline = (baseline - (top + bottom) / 2.0) / size;
            assert!(
                (0.28..=0.48).contains(&centre_above_baseline),
                "{font_id:?}: 电 is drawn at {top}..{bottom}, the baseline is {baseline}"
            );
        }
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
