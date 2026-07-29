use std::fmt;
use std::path::{Path, PathBuf};

use cadx_core::{CadDocument, CommandError, Point2};
use cadx_render::{Bounds2, RenderItem, RenderPrimitive, RenderScene, aligned_dimension_geometry};
use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str};

use crate::archive::write_atomically;
use crate::error::ProjectError;

pub const PDF_EXTENSION: &str = "pdf";
pub const MAX_PDF_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_PDF_ENTITIES: usize = 250_000;
pub const MAX_PDF_PATH_SEGMENTS: usize = 1_000_000;
pub const MAX_PDF_TEXT_BYTES: usize = 8 * 1024 * 1024;

const POINTS_PER_MILLIMETER: f32 = 72.0 / 25.4;
const FONT_NAME: Name<'static> = Name(b"F1");

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PdfPageSize {
    #[default]
    A4,
    A3,
    Letter,
}

impl PdfPageSize {
    pub const ALL: [Self; 3] = [Self::A4, Self::A3, Self::Letter];

    pub const fn label(self) -> &'static str {
        match self {
            Self::A4 => "A4",
            Self::A3 => "A3",
            Self::Letter => "Letter",
        }
    }

    const fn millimeters(self) -> (f32, f32) {
        match self {
            Self::A4 => (210.0, 297.0),
            Self::A3 => (297.0, 420.0),
            Self::Letter => (215.9, 279.4),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PdfOrientation {
    Portrait,
    #[default]
    Landscape,
}

impl PdfOrientation {
    pub const ALL: [Self; 2] = [Self::Portrait, Self::Landscape];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Portrait => "Portrait",
            Self::Landscape => "Landscape",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PdfExportOptions {
    pub page_size: PdfPageSize,
    pub orientation: PdfOrientation,
    pub margin_mm: f32,
    pub line_width_points: f32,
    pub text_size_points: f32,
}

impl Default for PdfExportOptions {
    fn default() -> Self {
        Self {
            page_size: PdfPageSize::A4,
            orientation: PdfOrientation::Landscape,
            margin_mm: 12.0,
            line_width_points: 0.7,
            text_size_points: 9.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PdfExportReport {
    pub path: PathBuf,
    pub bytes: u64,
    pub exported_entities: usize,
    pub skipped_entities: usize,
    pub simplified_entities: usize,
    pub page_width_points: f32,
    pub page_height_points: f32,
    pub omitted_parameters: usize,
    pub omitted_constraints: usize,
    pub omitted_locked_layers: usize,
}

#[derive(Debug)]
pub enum PdfExportError {
    Io(std::io::Error),
    Project(ProjectError),
    Command(CommandError),
    InvalidInput(String),
    InvalidPath(PathBuf),
    LimitExceeded { resource: &'static str, limit: u64 },
}

impl From<std::io::Error> for PdfExportError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<ProjectError> for PdfExportError {
    fn from(error: ProjectError) -> Self {
        Self::Project(error)
    }
}

impl From<CommandError> for PdfExportError {
    fn from(error: CommandError) -> Self {
        Self::Command(error)
    }
}

impl fmt::Display for PdfExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Project(error) => write!(formatter, "cannot write PDF atomically: {error}"),
            Self::Command(error) => write!(formatter, "cannot export invalid document: {error}"),
            Self::InvalidInput(message) => formatter.write_str(message),
            Self::InvalidPath(path) => write!(formatter, "invalid PDF path {}", path.display()),
            Self::LimitExceeded { resource, limit } => {
                write!(formatter, "PDF {resource} exceeds the limit of {limit}")
            }
        }
    }
}

impl std::error::Error for PdfExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Project(error) => Some(error),
            Self::Command(error) => Some(error),
            _ => None,
        }
    }
}

/// Writes the visible immutable 2D scene as a bounded single-page vector PDF.
pub fn export_pdf(
    document: &CadDocument,
    path: impl AsRef<Path>,
    options: PdfExportOptions,
) -> Result<PdfExportReport, PdfExportError> {
    document.validate()?;
    validate_options(options)?;
    if document.entities.len() > MAX_PDF_ENTITIES {
        return Err(PdfExportError::LimitExceeded {
            resource: "entity count",
            limit: MAX_PDF_ENTITIES as u64,
        });
    }
    let path = path.as_ref();
    if path.file_name().is_none() {
        return Err(PdfExportError::InvalidPath(path.to_path_buf()));
    }

    let (page_width, page_height) = page_dimensions(options);
    let margin = options.margin_mm * POINTS_PER_MILLIMETER;
    let content_box = PageBox {
        min_x: margin,
        min_y: margin,
        max_x: page_width - margin,
        max_y: page_height - margin,
    };
    let scene = RenderScene::from_document(document);
    preflight_scene(&scene)?;
    let transform = PageTransform::fit(scene.bounds, content_box)?;

    let catalog_id = Ref::new(1);
    let page_tree_id = Ref::new(2);
    let page_id = Ref::new(3);
    let font_id = Ref::new(4);
    let content_id = Ref::new(5);
    let mut pdf = Pdf::new();
    pdf.catalog(catalog_id).pages(page_tree_id);
    pdf.pages(page_tree_id).kids([page_id]).count(1);
    let mut page = pdf.page(page_id);
    page.parent(page_tree_id)
        .media_box(Rect::new(0.0, 0.0, page_width, page_height))
        .contents(content_id);
    page.resources().fonts().pair(FONT_NAME, font_id);
    page.finish();
    pdf.type1_font(font_id).base_font(Name(b"Helvetica"));

    let mut content = Content::new();
    content.set_line_width(options.line_width_points);
    let mut exported_entities = 0;
    let mut skipped_entities = document.entities.len().saturating_sub(scene.items.len());
    let mut simplified_entities = 0;
    for item in &scene.items {
        match draw_item(&mut content, item, transform, content_box, options) {
            DrawOutcome::Exported => exported_entities += 1,
            DrawOutcome::Simplified => {
                exported_entities += 1;
                simplified_entities += 1;
            }
            DrawOutcome::Skipped => skipped_entities += 1,
        }
    }
    pdf.stream(content_id, &content.finish());
    let bytes = pdf.finish();
    if bytes.len() as u64 > MAX_PDF_BYTES {
        return Err(PdfExportError::LimitExceeded {
            resource: "encoded bytes",
            limit: MAX_PDF_BYTES,
        });
    }
    write_atomically(path, &bytes)?;

    Ok(PdfExportReport {
        path: path.to_path_buf(),
        bytes: bytes.len() as u64,
        exported_entities,
        skipped_entities,
        simplified_entities,
        page_width_points: page_width,
        page_height_points: page_height,
        omitted_parameters: document.parameters.len(),
        omitted_constraints: document.constraints.len(),
        omitted_locked_layers: document
            .layers
            .values()
            .filter(|layer| layer.locked)
            .count(),
    })
}

fn validate_options(options: PdfExportOptions) -> Result<(), PdfExportError> {
    let (mut width_mm, mut height_mm) = options.page_size.millimeters();
    if options.orientation == PdfOrientation::Landscape {
        std::mem::swap(&mut width_mm, &mut height_mm);
    }
    if !options.margin_mm.is_finite()
        || options.margin_mm < 0.0
        || options.margin_mm * 2.0 >= width_mm.min(height_mm)
    {
        return Err(PdfExportError::InvalidInput(
            "PDF margin must be finite, nonnegative, and leave a printable page area".into(),
        ));
    }
    if !options.line_width_points.is_finite() || !(0.1..=5.0).contains(&options.line_width_points) {
        return Err(PdfExportError::InvalidInput(
            "PDF line width must be between 0.1 and 5 points".into(),
        ));
    }
    if !options.text_size_points.is_finite() || !(4.0..=72.0).contains(&options.text_size_points) {
        return Err(PdfExportError::InvalidInput(
            "PDF text size must be between 4 and 72 points".into(),
        ));
    }
    Ok(())
}

fn page_dimensions(options: PdfExportOptions) -> (f32, f32) {
    let (mut width, mut height) = options.page_size.millimeters();
    if options.orientation == PdfOrientation::Landscape {
        std::mem::swap(&mut width, &mut height);
    }
    (
        width * POINTS_PER_MILLIMETER,
        height * POINTS_PER_MILLIMETER,
    )
}

fn preflight_scene(scene: &RenderScene) -> Result<(), PdfExportError> {
    let mut segments = 0_usize;
    let mut text_bytes = 0_usize;
    for item in &scene.items {
        segments = segments.checked_add(path_segments(&item.primitive)).ok_or(
            PdfExportError::LimitExceeded {
                resource: "path segment count",
                limit: MAX_PDF_PATH_SEGMENTS as u64,
            },
        )?;
        text_bytes = text_bytes
            .checked_add(primitive_text_bytes(&item.primitive))
            .ok_or(PdfExportError::LimitExceeded {
                resource: "text bytes",
                limit: MAX_PDF_TEXT_BYTES as u64,
            })?;
    }
    if segments > MAX_PDF_PATH_SEGMENTS {
        return Err(PdfExportError::LimitExceeded {
            resource: "path segment count",
            limit: MAX_PDF_PATH_SEGMENTS as u64,
        });
    }
    if text_bytes > MAX_PDF_TEXT_BYTES {
        return Err(PdfExportError::LimitExceeded {
            resource: "text bytes",
            limit: MAX_PDF_TEXT_BYTES as u64,
        });
    }
    Ok(())
}

fn path_segments(primitive: &RenderPrimitive) -> usize {
    match primitive {
        RenderPrimitive::Line { .. } | RenderPrimitive::Wall { .. } => 1,
        RenderPrimitive::Circle { .. } => 4,
        RenderPrimitive::Arc { sweep_angle, .. } => {
            (sweep_angle.abs() / std::f64::consts::FRAC_PI_2).ceil() as usize
        }
        RenderPrimitive::AlignedDimension { .. } => 9,
        RenderPrimitive::Rectangle { .. } => 4,
        RenderPrimitive::SketchProfile { points, closed } => points
            .len()
            .saturating_sub(1)
            .saturating_add(usize::from(*closed && points.len() > 2)),
        RenderPrimitive::Room { boundary } => boundary.len(),
        RenderPrimitive::Extrude { .. } | RenderPrimitive::Text { .. } => 0,
    }
}

fn primitive_text_bytes(primitive: &RenderPrimitive) -> usize {
    match primitive {
        RenderPrimitive::Text { content, .. } => content.len(),
        RenderPrimitive::AlignedDimension { label, .. } => label.len(),
        _ => 0,
    }
}

#[derive(Clone, Copy)]
struct PageBox {
    min_x: f32,
    min_y: f32,
    max_x: f32,
    max_y: f32,
}

#[derive(Clone, Copy)]
struct PageTransform {
    center: Point2,
    page_center_x: f64,
    page_center_y: f64,
    scale: f64,
}

impl PageTransform {
    fn fit(bounds: Option<Bounds2>, page: PageBox) -> Result<Self, PdfExportError> {
        let page_width = f64::from(page.max_x - page.min_x);
        let page_height = f64::from(page.max_y - page.min_y);
        let page_center_x = f64::from((page.min_x + page.max_x) * 0.5);
        let page_center_y = f64::from((page.min_y + page.max_y) * 0.5);
        let Some(bounds) = bounds else {
            return Ok(Self {
                center: Point2::new(0.0, 0.0),
                page_center_x,
                page_center_y,
                scale: 1.0,
            });
        };
        let width = bounds.max.x - bounds.min.x;
        let height = bounds.max.y - bounds.min.y;
        let center = Point2::new(bounds.min.x + width * 0.5, bounds.min.y + height * 0.5);
        if !width.is_finite()
            || !height.is_finite()
            || !center.x.is_finite()
            || !center.y.is_finite()
            || width < 0.0
            || height < 0.0
        {
            return Err(PdfExportError::InvalidInput(
                "visible PDF bounds are not representable".into(),
            ));
        }
        let scale_x = (width > f64::EPSILON).then_some(page_width / width);
        let scale_y = (height > f64::EPSILON).then_some(page_height / height);
        let scale = match (scale_x, scale_y) {
            (Some(x), Some(y)) => x.min(y),
            (Some(x), None) => x,
            (None, Some(y)) => y,
            (None, None) => 1.0,
        };
        if !scale.is_finite() || scale <= 0.0 {
            return Err(PdfExportError::InvalidInput(
                "visible PDF bounds cannot be fitted to the page".into(),
            ));
        }
        Ok(Self {
            center,
            page_center_x,
            page_center_y,
            scale,
        })
    }

    fn point(self, point: Point2) -> (f32, f32) {
        let x = (point.x - self.center.x).mul_add(self.scale, self.page_center_x);
        let y = (point.y - self.center.y).mul_add(self.scale, self.page_center_y);
        (x as f32, y as f32)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DrawOutcome {
    Exported,
    Simplified,
    Skipped,
}

fn draw_item(
    content: &mut Content,
    item: &RenderItem,
    transform: PageTransform,
    page: PageBox,
    options: PdfExportOptions,
) -> DrawOutcome {
    let color = flattened_color(item.color);
    content
        .set_stroke_rgb(color[0], color[1], color[2])
        .set_fill_rgb(color[0], color[1], color[2])
        .set_line_width(options.line_width_points);
    match &item.primitive {
        RenderPrimitive::Line { start, end } => {
            draw_segment(content, transform.point(*start), transform.point(*end));
            DrawOutcome::Exported
        }
        RenderPrimitive::Circle { center, radius } => {
            draw_arc(
                content,
                transform,
                *center,
                *radius,
                0.0,
                std::f64::consts::TAU,
            );
            DrawOutcome::Exported
        }
        RenderPrimitive::Arc {
            center,
            radius,
            start_angle,
            sweep_angle,
        } => {
            draw_arc(
                content,
                transform,
                *center,
                *radius,
                *start_angle,
                *sweep_angle,
            );
            DrawOutcome::Exported
        }
        RenderPrimitive::AlignedDimension {
            start,
            end,
            offset,
            label,
        } => {
            let Some(geometry) = aligned_dimension_geometry(*start, *end, *offset) else {
                return DrawOutcome::Skipped;
            };
            let source_start = transform.point(geometry.start);
            let source_end = transform.point(geometry.end);
            let dimension_start = transform.point(geometry.dimension_start);
            let dimension_end = transform.point(geometry.dimension_end);
            draw_segment(content, source_start, dimension_start);
            draw_segment(content, source_end, dimension_end);
            draw_segment(content, dimension_start, dimension_end);
            draw_arrowhead(content, dimension_start, dimension_end, color);
            draw_arrowhead(content, dimension_end, dimension_start, color);
            let Some(text) = encode_base14_text(label) else {
                return DrawOutcome::Simplified;
            };
            draw_centered_text(
                content,
                transform.point(geometry.dimension_midpoint),
                &text,
                page,
                options.text_size_points,
                color,
                true,
            );
            DrawOutcome::Exported
        }
        RenderPrimitive::Rectangle {
            origin,
            width,
            height,
        } => {
            let points = [
                *origin,
                Point2::new(origin.x + width, origin.y),
                Point2::new(origin.x + width, origin.y + height),
                Point2::new(origin.x, origin.y + height),
            ];
            draw_polyline(content, transform, &points, true);
            DrawOutcome::Exported
        }
        RenderPrimitive::SketchProfile { points, closed } => {
            draw_polyline(content, transform, points, *closed);
            DrawOutcome::Exported
        }
        RenderPrimitive::Extrude { .. } => DrawOutcome::Skipped,
        RenderPrimitive::Wall {
            start,
            end,
            thickness,
        } => {
            content.save_state();
            content.set_line_width(
                ((*thickness * transform.scale) as f32).max(options.line_width_points),
            );
            draw_segment(content, transform.point(*start), transform.point(*end));
            content.restore_state();
            DrawOutcome::Exported
        }
        RenderPrimitive::Room { boundary } => {
            draw_polyline(content, transform, boundary, true);
            DrawOutcome::Exported
        }
        RenderPrimitive::Text {
            position,
            content: text,
        } => {
            let Some(text) = encode_base14_text(text) else {
                return DrawOutcome::Skipped;
            };
            draw_left_text(
                content,
                transform.point(*position),
                &text,
                page,
                options.text_size_points,
                color,
            );
            DrawOutcome::Exported
        }
    }
}

fn flattened_color(color: [u8; 4]) -> [f32; 3] {
    let alpha = f32::from(color[3]) / 255.0;
    [
        (f32::from(color[0]) * alpha + 255.0 * (1.0 - alpha)) / 255.0,
        (f32::from(color[1]) * alpha + 255.0 * (1.0 - alpha)) / 255.0,
        (f32::from(color[2]) * alpha + 255.0 * (1.0 - alpha)) / 255.0,
    ]
}

fn draw_segment(content: &mut Content, start: (f32, f32), end: (f32, f32)) {
    content
        .move_to(start.0, start.1)
        .line_to(end.0, end.1)
        .stroke();
}

fn draw_polyline(content: &mut Content, transform: PageTransform, points: &[Point2], closed: bool) {
    let Some(first) = points.first().copied() else {
        return;
    };
    let first = transform.point(first);
    content.move_to(first.0, first.1);
    for point in &points[1..] {
        let point = transform.point(*point);
        content.line_to(point.0, point.1);
    }
    if closed && points.len() > 2 {
        content.close_path();
    }
    content.stroke();
}

fn draw_arc(
    content: &mut Content,
    transform: PageTransform,
    center: Point2,
    radius: f64,
    start_angle: f64,
    sweep_angle: f64,
) {
    let segments = (sweep_angle.abs() / std::f64::consts::FRAC_PI_2)
        .ceil()
        .max(1.0) as usize;
    let segment_sweep = sweep_angle / segments as f64;
    let start = arc_point(center, radius, start_angle);
    let start = transform.point(start);
    content.move_to(start.0, start.1);
    for index in 0..segments {
        let first_angle = start_angle + segment_sweep * index as f64;
        let second_angle = first_angle + segment_sweep;
        let first = arc_point(center, radius, first_angle);
        let second = arc_point(center, radius, second_angle);
        let tangent_scale = 4.0 / 3.0 * (segment_sweep * 0.25).tan() * radius;
        let control_1 = Point2::new(
            first.x - first_angle.sin() * tangent_scale,
            first.y + first_angle.cos() * tangent_scale,
        );
        let control_2 = Point2::new(
            second.x + second_angle.sin() * tangent_scale,
            second.y - second_angle.cos() * tangent_scale,
        );
        let control_1 = transform.point(control_1);
        let control_2 = transform.point(control_2);
        let second = transform.point(second);
        content.cubic_to(
            control_1.0,
            control_1.1,
            control_2.0,
            control_2.1,
            second.0,
            second.1,
        );
    }
    content.stroke();
}

fn arc_point(center: Point2, radius: f64, angle: f64) -> Point2 {
    Point2::new(
        angle.cos().mul_add(radius, center.x),
        angle.sin().mul_add(radius, center.y),
    )
}

fn draw_arrowhead(content: &mut Content, tip: (f32, f32), target: (f32, f32), color: [f32; 3]) {
    let dx = target.0 - tip.0;
    let dy = target.1 - tip.1;
    let length = dx.hypot(dy);
    if length <= f32::EPSILON {
        return;
    }
    let unit_x = dx / length;
    let unit_y = dy / length;
    let normal_x = -unit_y;
    let normal_y = unit_x;
    content.set_fill_rgb(color[0], color[1], color[2]);
    content
        .move_to(tip.0, tip.1)
        .line_to(
            tip.0 - unit_x * 7.0 + normal_x * 3.0,
            tip.1 - unit_y * 7.0 + normal_y * 3.0,
        )
        .line_to(
            tip.0 - unit_x * 7.0 - normal_x * 3.0,
            tip.1 - unit_y * 7.0 - normal_y * 3.0,
        )
        .close_path()
        .fill_nonzero();
}

fn encode_base14_text(value: &str) -> Option<Vec<u8>> {
    value.is_ascii().then(|| {
        value
            .bytes()
            .map(|byte| if byte.is_ascii_control() { b' ' } else { byte })
            .collect()
    })
}

fn draw_left_text(
    content: &mut Content,
    position: (f32, f32),
    text: &[u8],
    page: PageBox,
    size: f32,
    color: [f32; 3],
) {
    let width = estimated_text_width(text, size);
    let x = position
        .0
        .clamp(page.min_x, (page.max_x - width).max(page.min_x));
    let y = position
        .1
        .clamp(page.min_y, (page.max_y - size).max(page.min_y));
    draw_text(content, x, y, text, size, color);
}

fn draw_centered_text(
    content: &mut Content,
    position: (f32, f32),
    text: &[u8],
    page: PageBox,
    size: f32,
    color: [f32; 3],
    mask_line: bool,
) {
    let width = estimated_text_width(text, size);
    let x = (position.0 - width * 0.5).clamp(page.min_x, (page.max_x - width).max(page.min_x));
    let y = (position.1 - size * 0.35).clamp(page.min_y, (page.max_y - size).max(page.min_y));
    if mask_line {
        content.set_fill_rgb(1.0, 1.0, 1.0);
        content
            .rect(x - 2.0, y - 1.5, width + 4.0, size + 3.0)
            .fill_nonzero();
    }
    draw_text(content, x, y, text, size, color);
}

fn estimated_text_width(text: &[u8], size: f32) -> f32 {
    text.len() as f32 * size * 0.52
}

fn draw_text(content: &mut Content, x: f32, y: f32, text: &[u8], size: f32, color: [f32; 3]) {
    content.set_fill_rgb(color[0], color[1], color[2]);
    content.begin_text();
    content.set_font(FONT_NAME, size);
    content.set_text_matrix([1.0, 0.0, 0.0, 1.0, x, y]);
    content.show(Str(text));
    content.end_text();
}
