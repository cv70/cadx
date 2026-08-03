//! Textual PCB manufacturing export contracts.

use cadx_ecad_layout::PcbBoard;
use std::fmt::Write as _;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportFile {
    pub name: String,
    pub contents: String,
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum ManufacturingExportError {
    #[error("board layout is invalid: {0}")]
    InvalidBoard(#[from] cadx_ecad_layout::LayoutError),
}

/// Validates the board and emits copper, edge-cut, and drill files.
///
/// # Errors
///
/// Returns a layout validation error before any manufacturing file is emitted.
pub fn manufacturing_bundle(board: &PcbBoard) -> Result<Vec<ExportFile>, ManufacturingExportError> {
    board.validate()?;
    Ok(gerber_bundle(board))
}

#[must_use]
pub fn gerber_bundle(board: &PcbBoard) -> Vec<ExportFile> {
    let mut files = Vec::new();
    let outline = format!(
        "G04 CADX board outline*\n%FSLAX46Y46*%\n%MOMM*%\nX0Y0D02*\nX{}Y0D01*\nX{}Y{}D01*\nX0Y{}D01*\nX0Y0D01*\nM02*\n",
        scaled(board.width_mm),
        scaled(board.width_mm),
        scaled(board.height_mm),
        scaled(board.height_mm)
    );
    files.push(ExportFile {
        name: "board.Edge_Cuts.gm1".into(),
        contents: outline,
    });
    for layer in board
        .layers
        .iter()
        .filter(|layer| matches!(layer.kind, cadx_ecad_layout::LayerKind::Copper))
    {
        let mut contents = format!(
            "G04 CADX {} copper*\n%FSLAX46Y46*%\n%MOMM*%\n%ADD10C,0.150000*%\nD10*\n",
            layer.name
        );
        for trace in board
            .traces
            .iter()
            .filter(|trace| trace.layer == layer.name)
        {
            let _ = writeln!(
                contents,
                "G04 NET {} WIDTH {:.6}*",
                trace.net, trace.width_mm
            );
            if let Some(first) = trace.points_mm.first() {
                let _ = writeln!(contents, "X{}Y{}D02*", scaled(first[0]), scaled(first[1]));
                for point in trace.points_mm.iter().skip(1) {
                    let _ = writeln!(contents, "X{}Y{}D01*", scaled(point[0]), scaled(point[1]));
                }
            }
        }
        for pad in board
            .pads
            .iter()
            .filter(|pad| pad.layers.is_empty() || pad.layers.contains(&layer.name))
        {
            let _ = writeln!(
                contents,
                "G04 PAD {}.{} NET {} SIZE {:.6}x{:.6}*\nX{}Y{}D03*",
                pad.reference,
                pad.number,
                pad.net,
                pad.size_mm[0],
                pad.size_mm[1],
                scaled(pad.position_mm[0]),
                scaled(pad.position_mm[1])
            );
        }
        contents.push_str("M02*\n");
        files.push(ExportFile {
            name: format!("board.{}.gbr", layer.name.replace(['/', ' '], "_")),
            contents,
        });
    }
    let mut drill = "; CADX Excellon drill file\nM48\nMETRIC\n".to_string();
    let mut tool = 1_u32;
    for pad in board.pads.iter().filter(|pad| pad.drill_mm.is_some()) {
        let diameter = pad.drill_mm.unwrap_or_default();
        let _ = writeln!(drill, "T{tool:02}C{diameter:.4}\n%\nT{tool:02}");
        let _ = writeln!(
            drill,
            "X{}Y{}",
            scaled(pad.position_mm[0]),
            scaled(pad.position_mm[1])
        );
        tool = tool.saturating_add(1);
    }
    for via in &board.vias {
        let _ = writeln!(drill, "T{tool:02}C{:.4}\n%\nT{tool:02}", via.drill_mm);
        let _ = writeln!(
            drill,
            "X{}Y{}",
            scaled(via.position_mm[0]),
            scaled(via.position_mm[1])
        );
        tool = tool.saturating_add(1);
    }
    drill.push_str("M30\n");
    files.push(ExportFile {
        name: "board.drl".into(),
        contents: drill,
    });
    files
}

#[must_use]
pub fn step_board_outline(board: &PcbBoard) -> String {
    format!(
        "ISO-10303-21;\nHEADER; FILE_DESCRIPTION(('CADX PCB board'),'2;1'); ENDSEC;\nDATA;\n/* board {} x {} x {} mm */\nENDSEC;\nEND-ISO-10303-21;\n",
        board.width_mm, board.height_mm, board.thickness_mm
    )
}

fn scaled(value: f64) -> String {
    format!("{:.0}", value * 1_000_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_contains_edge_cuts_and_drill_file() {
        let files = gerber_bundle(&PcbBoard::demo());
        assert!(files.iter().any(|file| file.name.contains("Edge_Cuts")));
        assert!(files.iter().any(|file| {
            std::path::Path::new(&file.name)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("drl"))
        }));
        assert!(step_board_outline(&PcbBoard::demo()).contains("ISO-10303-21"));
        assert!(manufacturing_bundle(&PcbBoard::demo()).is_ok());
    }
}
