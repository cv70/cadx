use std::sync::Arc;

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, Stroke, Vec2};

const CJK_FONT_NAME: &str = "Droid Sans Fallback";
const CJK_FONT_BYTES: &[u8] = include_bytes!("../assets/DroidSansFallback.ttf");

pub(crate) fn configure_style(context: &egui::Context) {
    configure_fonts(context);
    // CADX has a deliberately dark drafting surface and workbench chrome. Pinning the
    // preference prevents egui from replacing this palette when macOS is in light mode.
    context.set_theme(egui::Theme::Dark);
    let mut style = (*context.style()).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(10.0, 6.0);
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = Color32::from_rgb(20, 25, 29);
    style.visuals.window_fill = Color32::from_rgb(24, 31, 35);
    style.visuals.extreme_bg_color = Color32::from_rgb(14, 18, 21);
    style.visuals.faint_bg_color = Color32::from_rgb(23, 30, 34);
    style.visuals.window_stroke = Stroke::new(1.0, Color32::from_rgb(53, 66, 71));
    style.visuals.widgets.noninteractive.fg_stroke =
        Stroke::new(1.0, Color32::from_rgb(224, 232, 232));
    style.visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(27, 35, 39);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(33, 43, 48);
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(47, 71, 74);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(59, 113, 108);
    style.visuals.selection.bg_fill = Color32::from_rgb(36, 120, 111);
    context.set_style(style);
}

fn configure_fonts(context: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        CJK_FONT_NAME.into(),
        Arc::new(FontData::from_static(CJK_FONT_BYTES)),
    );
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push(CJK_FONT_NAME.into());
    }
    context.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_fallback_covers_representative_simplified_chinese_ui_text() {
        let context = egui::Context::default();
        configure_fonts(&context);
        let mut covered = false;
        let _ = context.run(egui::RawInput::default(), |context| {
            covered = context.fonts(|fonts| {
                fonts.has_glyphs(
                    &egui::FontId::proportional(14.0),
                    "简体中文工程恢复约束图层",
                )
            });
        });

        assert!(covered);
    }
}
