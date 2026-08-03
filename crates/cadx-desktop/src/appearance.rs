use std::{fs, sync::Arc};

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, RichText};
use iconflow::{Pack, Size, Style, fonts, try_icon};

pub const BACKGROUND: Color32 = Color32::from_rgb(14, 15, 17);
pub const SURFACE: Color32 = Color32::from_rgb(25, 26, 29);
pub const SURFACE_RAISED: Color32 = Color32::from_rgb(31, 33, 36);
pub const FILL: Color32 = Color32::from_rgb(36, 38, 42);
pub const BORDER: Color32 = Color32::from_rgb(55, 57, 63);
pub const BORDER_SOFT: Color32 = Color32::from_rgb(45, 47, 52);
pub const TEXT: Color32 = Color32::from_rgb(239, 240, 242);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(151, 153, 161);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(103, 105, 113);
pub const ACCENT: Color32 = Color32::from_rgb(67, 184, 174);
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(38, 76, 73);
pub const WARNING: Color32 = Color32::from_rgb(230, 164, 91);
pub const DANGER: Color32 = Color32::from_rgb(224, 103, 103);

const CJK_FONT_CANDIDATES: &[&str] = &[
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    "C:\\Windows\\Fonts\\msyh.ttc",
    "C:\\Windows\\Fonts\\simhei.ttf",
];

pub fn configure(context: &egui::Context, configured_cjk_font: Option<&std::path::Path>) {
    install_fonts(context, configured_cjk_font);
    context.set_theme(egui::Theme::Dark);

    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = SURFACE;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = BACKGROUND;
    visuals.text_edit_bg_color = Some(BACKGROUND);
    visuals.faint_bg_color = SURFACE_RAISED;
    visuals.weak_text_color = Some(TEXT_MUTED);
    visuals.selection.bg_fill = ACCENT_SOFT;
    visuals.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, BORDER_SOFT);
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT);
    visuals.widgets.inactive.bg_fill = FILL;
    visuals.widgets.inactive.weak_bg_fill = FILL;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, BORDER_SOFT);
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, TEXT);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(46, 49, 53);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(46, 49, 53);
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0, BORDER);
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, TEXT);
    visuals.widgets.active.bg_fill = ACCENT_SOFT;
    visuals.widgets.active.weak_bg_fill = ACCENT_SOFT;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0, ACCENT);
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, TEXT);
    visuals.widgets.open.bg_fill = SURFACE_RAISED;
    visuals.widgets.open.weak_bg_fill = SURFACE_RAISED;
    for widgets in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        widgets.corner_radius = egui::CornerRadius::same(5);
    }
    context.set_visuals(visuals);

    context.style_mut_of(egui::Theme::Dark, |style| {
        style.spacing.item_spacing = egui::vec2(7.0, 7.0);
        style.spacing.button_padding = egui::vec2(9.0, 5.0);
        style.spacing.interact_size.y = 28.0;
        style.spacing.menu_margin = egui::Margin::same(6);
        style.animation_time = 0.12;
    });
}

fn install_fonts(context: &egui::Context, configured_cjk_font: Option<&std::path::Path>) {
    let mut definitions = FontDefinitions::default();

    for font in fonts() {
        let family_name = font.family.to_owned();
        definitions.font_data.insert(
            family_name.clone(),
            Arc::new(FontData::from_static(font.bytes)),
        );
        definitions
            .families
            .insert(FontFamily::Name(font.family.into()), vec![family_name]);
    }

    if let Some(bytes) = load_cjk_font(configured_cjk_font) {
        let family_name = "cadx-cjk".to_owned();
        definitions
            .font_data
            .insert(family_name.clone(), Arc::new(FontData::from_owned(bytes)));
        definitions
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .push(family_name.clone());
        definitions
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .push(family_name);
    }

    context.set_fonts(definitions);
}

fn load_cjk_font(configured: Option<&std::path::Path>) -> Option<Vec<u8>> {
    configured.and_then(|path| fs::read(path).ok()).or_else(|| {
        CJK_FONT_CANDIDATES
            .iter()
            .find_map(|path| fs::read(path).ok())
    })
}

#[must_use]
pub fn icon(name: &str, size: f32) -> RichText {
    let icon = try_icon(Pack::Lucide, name, Style::Regular, Size::Regular).unwrap_or_else(|_| {
        try_icon(Pack::Lucide, "circle", Style::Regular, Size::Regular)
            .expect("fallback Lucide icon must exist")
    });
    let glyph = char::from_u32(icon.codepoint).unwrap_or('?');
    RichText::new(glyph.to_string()).font(FontId::new(size, FontFamily::Name(icon.family.into())))
}

pub fn panel_frame(fill: Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .inner_margin(egui::Margin::same(10))
        .stroke(egui::Stroke::new(1.0, BORDER_SOFT))
}

pub fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(10.0).strong().color(TEXT_MUTED));
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadx_domain_api::DomainPack;

    #[test]
    fn referenced_lucide_icons_exist() {
        let names = [
            "activity",
            "arrow-down",
            "arrow-up",
            "box",
            "boxes",
            "check",
            "circle",
            "circle-dot",
            "combine",
            "copy",
            "crosshair",
            "cylinder",
            "download",
            "file-input",
            "file-plus",
            "focus",
            "folder-open",
            "layers",
            "octagon",
            "panel-left",
            "panel-right",
            "pencil",
            "plus",
            "radius",
            "redo-2",
            "rotate-cw",
            "ruler",
            "save",
            "send",
            "sparkles",
            "trash-2",
            "triangle",
            "triangle-alert",
            "undo-2",
            "x",
        ];

        for name in names {
            assert!(
                try_icon(Pack::Lucide, name, Style::Regular, Size::Regular).is_ok(),
                "missing Lucide icon: {name}"
            );
        }
    }

    #[test]
    fn built_in_domain_pack_icons_exist() {
        let mcad = cadx_mcad::McadPack;
        let aec = cadx_aec::AecPack;
        let ecad = cadx_ecad::EcadPack;
        let packs: [&dyn DomainPack; 3] = [&mcad, &aec, &ecad];

        for pack in packs {
            for tool in pack.tools() {
                assert!(
                    try_icon(Pack::Lucide, tool.icon, Style::Regular, Size::Regular).is_ok(),
                    "missing Lucide icon for {} tool {}: {}",
                    pack.manifest().name,
                    tool.id,
                    tool.icon
                );
            }
        }
    }
}
