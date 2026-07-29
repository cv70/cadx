use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use ::dxf::entities::{
    Arc as DxfArc, Circle, DimensionBase, Entity as DxfEntity, EntityCommon, EntityType, Line,
    LwPolyline, RotatedDimension, Text,
};
use ::dxf::enums::{AcadVersion, DimensionType, DrawingUnits, Units as DxfUnits};
use ::dxf::tables::Layer as DxfLayer;
use ::dxf::{Color, Drawing, LwPolylineVertex, Point as DxfPoint, Vector};
use cadx_core::{
    CadCommand, CadDocument, CommandError, CommandTransaction, Entity, EntityKind, Layer, LayerId,
    Point2, Units,
};

use crate::archive::write_atomically;
use crate::error::ProjectError;

pub const DXF_EXTENSION: &str = "dxf";
pub const MAX_DXF_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_DXF_ENTITIES: usize = 250_000;
pub const MAX_DXF_LAYERS: usize = 4_096;
pub const MAX_DXF_VERTICES: usize = 1_000_000;

const PLANAR_EPSILON: f64 = 1.0e-9;
const MAX_DXF_LAYER_NAME_CHARS: usize = 255;

#[derive(Clone, Debug, PartialEq)]
pub struct DxfImportPlan {
    pub transaction: CommandTransaction,
    pub report: DxfImportReport,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DxfImportReport {
    pub source_units: String,
    pub scale_factor: f64,
    pub imported_entities: usize,
    pub skipped_entities: usize,
    pub created_layers: usize,
    pub reused_layers: usize,
    pub renamed_layers: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DxfExportReport {
    pub path: PathBuf,
    pub bytes: u64,
    pub exported_entities: usize,
    pub skipped_entities: usize,
    pub simplified_entities: usize,
    pub renamed_layers: usize,
    pub omitted_parameters: usize,
    pub omitted_constraints: usize,
    pub omitted_locked_layers: usize,
}

#[derive(Debug)]
pub enum DxfExchangeError {
    Io(std::io::Error),
    Format(::dxf::DxfError),
    Project(ProjectError),
    Command(CommandError),
    InvalidInput(String),
    InvalidPath(PathBuf),
    LimitExceeded { resource: &'static str, limit: u64 },
    LockedLayer(String),
}

impl From<std::io::Error> for DxfExchangeError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<::dxf::DxfError> for DxfExchangeError {
    fn from(error: ::dxf::DxfError) -> Self {
        Self::Format(error)
    }
}

impl From<ProjectError> for DxfExchangeError {
    fn from(error: ProjectError) -> Self {
        Self::Project(error)
    }
}

impl From<CommandError> for DxfExchangeError {
    fn from(error: CommandError) -> Self {
        Self::Command(error)
    }
}

impl fmt::Display for DxfExchangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Format(error) => write!(formatter, "invalid DXF: {error}"),
            Self::Project(error) => write!(formatter, "cannot write DXF atomically: {error}"),
            Self::Command(error) => write!(formatter, "DXF mapping is invalid: {error}"),
            Self::InvalidInput(message) => formatter.write_str(message),
            Self::InvalidPath(path) => write!(formatter, "invalid DXF path {}", path.display()),
            Self::LimitExceeded { resource, limit } => {
                write!(formatter, "DXF {resource} exceeds the limit of {limit}")
            }
            Self::LockedLayer(name) => {
                write!(formatter, "DXF layer {name:?} maps to a locked CADX layer")
            }
        }
    }
}

impl std::error::Error for DxfExchangeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Format(error) => Some(error),
            Self::Project(error) => Some(error),
            Self::Command(error) => Some(error),
            _ => None,
        }
    }
}

/// Parses a bounded DXF file and materializes it as one preflighted CADX transaction.
/// The caller remains responsible for committing the transaction to semantic history.
pub fn plan_dxf_import(
    document: &CadDocument,
    path: impl AsRef<Path>,
) -> Result<DxfImportPlan, DxfExchangeError> {
    let bytes = read_bounded_regular_file(path.as_ref())?;
    let mut cursor = Cursor::new(bytes);
    let drawing = Drawing::load(&mut cursor)?;
    plan_drawing_import(document, &drawing)
}

/// Writes the document's supported 2D projection as an atomically replaced DXF file.
pub fn export_dxf(
    document: &CadDocument,
    path: impl AsRef<Path>,
) -> Result<DxfExportReport, DxfExchangeError> {
    document.validate()?;
    if document.entities.len() > MAX_DXF_ENTITIES {
        return Err(DxfExchangeError::LimitExceeded {
            resource: "entity count",
            limit: MAX_DXF_ENTITIES as u64,
        });
    }
    if document.layers.len() > MAX_DXF_LAYERS {
        return Err(DxfExchangeError::LimitExceeded {
            resource: "layer count",
            limit: MAX_DXF_LAYERS as u64,
        });
    }

    let path = path.as_ref();
    if path.file_name().is_none() {
        return Err(DxfExchangeError::InvalidPath(path.to_path_buf()));
    }

    let mut drawing = Drawing::new();
    drawing.header.version = AcadVersion::R2018;
    drawing.header.default_drawing_units = export_units(document.units);
    drawing.header.drawing_units = match document.units {
        Units::Inches => DrawingUnits::English,
        Units::Millimeters | Units::Meters => DrawingUnits::Metric,
    };

    let (layer_names, renamed_layers) = export_layers(document, &mut drawing);
    let mut exported_entities = 0;
    let mut skipped_entities = 0;
    let mut simplified_entities = 0;
    let mut vertex_count = 0;

    for entity in document.entities.values() {
        let Some(layer_name) = layer_names.get(&entity.layer) else {
            return Err(DxfExchangeError::InvalidInput(format!(
                "entity {} references a layer that was not exported",
                entity.id
            )));
        };
        let (specific, simplified, vertices) = match export_entity_kind(&entity.kind) {
            Some(result) => result,
            None => {
                skipped_entities += 1;
                continue;
            }
        };
        vertex_count += vertices;
        if vertex_count > MAX_DXF_VERTICES {
            return Err(DxfExchangeError::LimitExceeded {
                resource: "vertex count",
                limit: MAX_DXF_VERTICES as u64,
            });
        }
        let mut common = EntityCommon::default();
        common.layer.clone_from(layer_name);
        common.is_visible = entity.visible;
        drawing.add_entity(DxfEntity { common, specific });
        exported_entities += 1;
        simplified_entities += usize::from(simplified);
    }

    let mut bytes = Vec::new();
    drawing.save(&mut bytes)?;
    if bytes.len() as u64 > MAX_DXF_BYTES {
        return Err(DxfExchangeError::LimitExceeded {
            resource: "encoded bytes",
            limit: MAX_DXF_BYTES,
        });
    }
    write_atomically(path, &bytes)?;

    Ok(DxfExportReport {
        path: path.to_path_buf(),
        bytes: bytes.len() as u64,
        exported_entities,
        skipped_entities,
        simplified_entities,
        renamed_layers,
        omitted_parameters: document.parameters.len(),
        omitted_constraints: document.constraints.len(),
        omitted_locked_layers: document
            .layers
            .values()
            .filter(|layer| layer.locked)
            .count(),
    })
}

#[derive(Clone)]
struct ImportLayerSpec {
    name: String,
    visible: bool,
    color: [u8; 4],
    renamed: bool,
}

struct PendingEntity {
    source_layer: String,
    name: String,
    visible: bool,
    kind: EntityKind,
}

fn plan_drawing_import(
    document: &CadDocument,
    drawing: &Drawing,
) -> Result<DxfImportPlan, DxfExchangeError> {
    let source_layer_count = drawing.layers().count();
    if source_layer_count > MAX_DXF_LAYERS {
        return Err(DxfExchangeError::LimitExceeded {
            resource: "layer count",
            limit: MAX_DXF_LAYERS as u64,
        });
    }
    let source_entity_count = drawing.entities().count();
    if source_entity_count > MAX_DXF_ENTITIES {
        return Err(DxfExchangeError::LimitExceeded {
            resource: "entity count",
            limit: MAX_DXF_ENTITIES as u64,
        });
    }

    let (source_units, scale_factor) =
        import_scale(drawing.header.default_drawing_units, document.units);
    let mut layers = BTreeMap::new();
    for (index, layer) in drawing.layers().enumerate() {
        let key = source_layer_key(&layer.name);
        layers.entry(key).or_insert_with(|| {
            let (name, renamed) = import_layer_name(&layer.name, index + 1);
            ImportLayerSpec {
                name,
                visible: layer.is_layer_on,
                color: aci_rgba(layer.color.index().unwrap_or(7)),
                renamed,
            }
        });
    }

    let mut pending = Vec::new();
    let mut skipped_entities = 0;
    let mut vertex_count = 0;
    for (index, entity) in drawing.entities().enumerate() {
        let converted = import_entity_kind(entity, scale_factor, &mut vertex_count)?;
        let Some(kind) = converted else {
            skipped_entities += 1;
            continue;
        };
        let key = source_layer_key(&entity.common.layer);
        if !layers.contains_key(&key) {
            let fallback_index = layers.len() + 1;
            let (name, renamed) = import_layer_name(&entity.common.layer, fallback_index);
            layers.insert(
                key.clone(),
                ImportLayerSpec {
                    name,
                    visible: true,
                    color: aci_rgba(7),
                    renamed,
                },
            );
            if layers.len() > MAX_DXF_LAYERS {
                return Err(DxfExchangeError::LimitExceeded {
                    resource: "layer count",
                    limit: MAX_DXF_LAYERS as u64,
                });
            }
        }
        pending.push(PendingEntity {
            source_layer: key,
            name: imported_entity_name(&kind, index + 1),
            visible: entity.common.is_visible,
            kind,
        });
    }

    let mut commands = Vec::with_capacity(pending.len() + layers.len());
    let mut source_to_layer = BTreeMap::new();
    let mut occupied_names = document
        .layers
        .values()
        .map(|layer| (layer.name.to_ascii_lowercase(), layer.id))
        .collect::<BTreeMap<_, _>>();
    let mut next_layer_id = document.next_layer_id();
    let mut next_entity_id = document.next_entity_id();
    let mut reused_layers = BTreeSet::new();
    let mut created_layers = 0;
    let mut renamed_layers = 0;

    for entity in pending {
        let layer_id = if let Some(id) = source_to_layer.get(&entity.source_layer).copied() {
            id
        } else {
            let spec = layers
                .get(&entity.source_layer)
                .expect("pending entities install their source layer");
            let key = spec.name.to_ascii_lowercase();
            let existing = occupied_names
                .get(&key)
                .and_then(|id| document.layers.get(id));
            let id = if let Some(existing) = existing {
                if existing.locked {
                    return Err(DxfExchangeError::LockedLayer(existing.name.clone()));
                }
                reused_layers.insert(existing.id);
                existing.id
            } else {
                let name = unique_import_layer_name(&spec.name, &occupied_names, next_layer_id);
                renamed_layers += usize::from(spec.renamed || name != spec.name);
                let id = next_layer_id;
                next_layer_id = next_layer_id.checked_add(1).ok_or_else(|| {
                    DxfExchangeError::InvalidInput("CADX layer ID space is exhausted".into())
                })?;
                occupied_names.insert(name.to_ascii_lowercase(), id);
                commands.push(CadCommand::CreateLayer {
                    layer: Layer {
                        id,
                        name,
                        visible: spec.visible,
                        locked: false,
                        color: spec.color,
                    },
                });
                created_layers += 1;
                id
            };
            source_to_layer.insert(entity.source_layer.clone(), id);
            id
        };

        let id = next_entity_id;
        next_entity_id = next_entity_id.checked_add(1).ok_or_else(|| {
            DxfExchangeError::InvalidInput("CADX entity ID space is exhausted".into())
        })?;
        commands.push(CadCommand::CreateEntity {
            entity: Entity {
                id,
                layer: layer_id,
                name: entity.name,
                visible: entity.visible,
                kind: entity.kind,
                parameter_refs: BTreeSet::new(),
            },
        });
    }

    let transaction = CommandTransaction::new(commands);
    transaction.preview(document)?;
    Ok(DxfImportPlan {
        report: DxfImportReport {
            source_units,
            scale_factor,
            imported_entities: source_entity_count - skipped_entities,
            skipped_entities,
            created_layers,
            reused_layers: reused_layers.len(),
            renamed_layers,
        },
        transaction,
    })
}

fn import_entity_kind(
    entity: &DxfEntity,
    scale: f64,
    vertex_count: &mut usize,
) -> Result<Option<EntityKind>, DxfExchangeError> {
    if entity.common.is_in_paper_space {
        return Ok(None);
    }
    let kind = match &entity.specific {
        EntityType::Line(line)
            if point_is_planar(&line.p1)
                && point_is_planar(&line.p2)
                && vector_is_positive_z(&line.extrusion_direction) =>
        {
            EntityKind::Line {
                start: scaled_point(&line.p1, scale)?,
                end: scaled_point(&line.p2, scale)?,
            }
        }
        EntityType::Circle(circle)
            if point_is_planar(&circle.center) && vector_is_positive_z(&circle.normal) =>
        {
            let radius = circle.radius * scale;
            if !radius.is_finite() || radius <= 0.0 {
                return Ok(None);
            }
            EntityKind::Circle {
                center: scaled_point(&circle.center, scale)?,
                radius,
            }
        }
        EntityType::Arc(arc)
            if point_is_planar(&arc.center) && vector_is_positive_z(&arc.normal) =>
        {
            let radius = arc.radius * scale;
            let sweep_angle = (arc.end_angle - arc.start_angle)
                .rem_euclid(360.0)
                .to_radians();
            if !radius.is_finite()
                || radius <= 0.0
                || !arc.start_angle.is_finite()
                || !arc.end_angle.is_finite()
                || sweep_angle <= 0.0
            {
                return Ok(None);
            }
            EntityKind::Arc {
                center: scaled_point(&arc.center, scale)?,
                radius,
                start_angle: arc.start_angle.to_radians(),
                sweep_angle,
            }
        }
        EntityType::RotatedDimension(dimension)
            if dimension.dimension_base.dimension_type == DimensionType::Aligned
                && point_is_planar(&dimension.dimension_base.definition_point_1)
                && point_is_planar(&dimension.definition_point_2)
                && point_is_planar(&dimension.definition_point_3)
                && vector_is_positive_z(&dimension.dimension_base.normal) =>
        {
            let start = scaled_point(&dimension.definition_point_2, scale)?;
            let end = scaled_point(&dimension.definition_point_3, scale)?;
            let line_point = scaled_point(&dimension.dimension_base.definition_point_1, scale)?;
            let Some(offset) = dimension_offset(start, end, line_point) else {
                return Ok(None);
            };
            if offset.abs() <= PLANAR_EPSILON {
                return Ok(None);
            }
            let text = &dimension.dimension_base.text;
            EntityKind::AlignedDimension {
                start,
                end,
                offset,
                text_override: (!text.is_empty() && text != "<>").then(|| text.clone()),
            }
        }
        EntityType::LwPolyline(polyline)
            if near_zero(entity.common.elevation)
                && vector_is_positive_z(&polyline.extrusion_direction)
                && polyline
                    .vertices
                    .iter()
                    .all(|vertex| near_zero(vertex.bulge)) =>
        {
            let points = polyline
                .vertices
                .iter()
                .map(|vertex| scaled_xy(vertex.x, vertex.y, scale))
                .collect::<Result<Vec<_>, _>>()?;
            import_profile(points, polyline.is_closed(), vertex_count)?
        }
        EntityType::Polyline(polyline)
            if !polyline.is_3d_polyline()
                && !polyline.is_3d_polygon_mesh()
                && !polyline.is_polyface_mesh()
                && !polyline.curve_fit_vertices_added()
                && !polyline.spline_fit_vertices_added()
                && point_is_planar(&polyline.location)
                && vector_is_positive_z(&polyline.normal)
                && polyline
                    .vertices()
                    .all(|vertex| point_is_planar(&vertex.location) && near_zero(vertex.bulge)) =>
        {
            let points = polyline
                .vertices()
                .map(|vertex| scaled_point(&vertex.location, scale))
                .collect::<Result<Vec<_>, _>>()?;
            import_profile(points, polyline.is_closed(), vertex_count)?
        }
        EntityType::Text(text)
            if point_is_planar(&text.location) && vector_is_positive_z(&text.normal) =>
        {
            if text.value.trim().is_empty() {
                return Ok(None);
            }
            EntityKind::Text {
                position: scaled_point(&text.location, scale)?,
                content: text.value.clone(),
            }
        }
        _ => return Ok(None),
    };
    Ok(Some(kind))
}

fn import_profile(
    mut points: Vec<Point2>,
    closed: bool,
    vertex_count: &mut usize,
) -> Result<EntityKind, DxfExchangeError> {
    if closed
        && points.len() > 1
        && point_distance_squared(points[0], points[points.len() - 1]) <= PLANAR_EPSILON.powi(2)
    {
        points.pop();
    }
    let minimum = if closed { 3 } else { 2 };
    if points.len() < minimum {
        return Err(DxfExchangeError::InvalidInput(format!(
            "DXF polyline has fewer than {minimum} usable vertices"
        )));
    }
    *vertex_count =
        vertex_count
            .checked_add(points.len())
            .ok_or(DxfExchangeError::LimitExceeded {
                resource: "vertex count",
                limit: MAX_DXF_VERTICES as u64,
            })?;
    if *vertex_count > MAX_DXF_VERTICES {
        return Err(DxfExchangeError::LimitExceeded {
            resource: "vertex count",
            limit: MAX_DXF_VERTICES as u64,
        });
    }
    Ok(EntityKind::SketchProfile { points, closed })
}

fn export_layers(
    document: &CadDocument,
    drawing: &mut Drawing,
) -> (BTreeMap<LayerId, String>, usize) {
    let mut used = BTreeSet::new();
    let mut names = BTreeMap::new();
    let mut renamed = 0;
    for layer in document.layers.values() {
        let base = sanitized_dxf_layer_name(&layer.name, layer.id);
        let name = unique_export_layer_name(&base, layer.id, &used);
        renamed += usize::from(name != layer.name);
        used.insert(name.to_ascii_lowercase());
        names.insert(layer.id, name.clone());

        let exported = DxfLayer {
            name: name.clone(),
            color: Color::from_index(nearest_aci(layer.color)),
            is_layer_on: layer.visible,
            ..Default::default()
        };
        if name == "0" {
            if let Some(default_layer) =
                drawing.layers_mut().find(|candidate| candidate.name == "0")
            {
                *default_layer = exported;
            }
        } else {
            drawing.add_layer(exported);
        }
    }
    (names, renamed)
}

fn export_entity_kind(kind: &EntityKind) -> Option<(EntityType, bool, usize)> {
    match kind {
        EntityKind::Line { start, end } => Some((
            EntityType::Line(Line::new(dxf_point(*start), dxf_point(*end))),
            false,
            0,
        )),
        EntityKind::Circle { center, radius } => Some((
            EntityType::Circle(Circle::new(dxf_point(*center), *radius)),
            false,
            0,
        )),
        EntityKind::Arc {
            center,
            radius,
            start_angle,
            sweep_angle,
        } => Some((
            EntityType::Arc(DxfArc::new(
                dxf_point(*center),
                *radius,
                start_angle.to_degrees().rem_euclid(360.0),
                (start_angle + sweep_angle).to_degrees().rem_euclid(360.0),
            )),
            false,
            0,
        )),
        EntityKind::AlignedDimension {
            start,
            end,
            offset,
            text_override,
        } => {
            let (dimension_start, dimension_end) = dimension_line(*start, *end, *offset)?;
            let rotation_angle = (end.y - start.y).atan2(end.x - start.x).to_degrees();
            Some((
                EntityType::RotatedDimension(RotatedDimension {
                    dimension_base: DimensionBase {
                        definition_point_1: dxf_point(dimension_start),
                        text_mid_point: dxf_point(Point2::new(
                            (dimension_start.x + dimension_end.x) * 0.5,
                            (dimension_start.y + dimension_end.y) * 0.5,
                        )),
                        dimension_type: DimensionType::Aligned,
                        text: text_override.clone().unwrap_or_else(|| "<>".into()),
                        ..Default::default()
                    },
                    definition_point_2: dxf_point(*start),
                    definition_point_3: dxf_point(*end),
                    rotation_angle,
                    ..Default::default()
                }),
                false,
                0,
            ))
        }
        EntityKind::Rectangle {
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
            Some((export_polyline(&points, true), true, points.len()))
        }
        EntityKind::SketchProfile { points, closed } if points.len() >= 2 => {
            Some((export_polyline(points, *closed), true, points.len()))
        }
        EntityKind::Wall { start, end, .. } => Some((
            EntityType::Line(Line::new(dxf_point(*start), dxf_point(*end))),
            true,
            0,
        )),
        EntityKind::Room { boundary, .. } => {
            Some((export_polyline(boundary, true), true, boundary.len()))
        }
        EntityKind::Text { position, content } => Some((
            EntityType::Text(Text {
                location: dxf_point(*position),
                value: content.clone(),
                ..Default::default()
            }),
            false,
            0,
        )),
        EntityKind::Extrude { .. } | EntityKind::SketchProfile { .. } => None,
    }
}

fn export_polyline(points: &[Point2], closed: bool) -> EntityType {
    let mut polyline = LwPolyline {
        vertices: points
            .iter()
            .map(|point| LwPolylineVertex {
                x: point.x,
                y: point.y,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    polyline.set_is_closed(closed);
    EntityType::LwPolyline(polyline)
}

fn read_bounded_regular_file(path: &Path) -> Result<Vec<u8>, DxfExchangeError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(DxfExchangeError::InvalidPath(path.to_path_buf()));
    }
    if metadata.len() > MAX_DXF_BYTES {
        return Err(DxfExchangeError::LimitExceeded {
            resource: "input bytes",
            limit: MAX_DXF_BYTES,
        });
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)?
        .take(MAX_DXF_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_DXF_BYTES {
        return Err(DxfExchangeError::LimitExceeded {
            resource: "input bytes",
            limit: MAX_DXF_BYTES,
        });
    }
    Ok(bytes)
}

fn import_scale(source: DxfUnits, target: Units) -> (String, f64) {
    let source_name = dxf_units_label(source).to_owned();
    let target_metres = match target {
        Units::Millimeters => 0.001,
        Units::Meters => 1.0,
        Units::Inches => 0.0254,
    };
    let source_metres = match source {
        DxfUnits::Unitless => target_metres,
        DxfUnits::Inches => 0.0254,
        DxfUnits::Feet => 0.3048,
        DxfUnits::Miles => 1_609.344,
        DxfUnits::Millimeters => 0.001,
        DxfUnits::Centimeters => 0.01,
        DxfUnits::Meters => 1.0,
        DxfUnits::Kilometers => 1_000.0,
        DxfUnits::Microinches => 0.000_000_025_4,
        DxfUnits::Mils => 0.000_025_4,
        DxfUnits::Yards => 0.9144,
        DxfUnits::Angstroms => 1.0e-10,
        DxfUnits::Nanometers => 1.0e-9,
        DxfUnits::Microns => 1.0e-6,
        DxfUnits::Decimeters => 0.1,
        DxfUnits::Decameters => 10.0,
        DxfUnits::Hectometers => 100.0,
        DxfUnits::Gigameters => 1.0e9,
        DxfUnits::AstronomicalUnits => 149_597_870_700.0,
        DxfUnits::LightYears => 9_460_730_472_580_800.0,
        DxfUnits::Parsecs => 30_856_775_814_913_672.0,
        DxfUnits::USSurveyFeet => 1_200.0 / 3_937.0,
        DxfUnits::USSurveyInch => 100.0 / 3_937.0,
        DxfUnits::USSurveyYard => 3_600.0 / 3_937.0,
        DxfUnits::USSurveyMile => 6_336_000.0 / 3_937.0,
    };
    (source_name, source_metres / target_metres)
}

fn dxf_units_label(units: DxfUnits) -> &'static str {
    match units {
        DxfUnits::Unitless => "unitless (assumed document units)",
        DxfUnits::Inches => "inches",
        DxfUnits::Feet => "feet",
        DxfUnits::Miles => "miles",
        DxfUnits::Millimeters => "millimeters",
        DxfUnits::Centimeters => "centimeters",
        DxfUnits::Meters => "meters",
        DxfUnits::Kilometers => "kilometers",
        DxfUnits::Microinches => "microinches",
        DxfUnits::Mils => "mils",
        DxfUnits::Yards => "yards",
        DxfUnits::Angstroms => "angstroms",
        DxfUnits::Nanometers => "nanometers",
        DxfUnits::Microns => "microns",
        DxfUnits::Decimeters => "decimeters",
        DxfUnits::Decameters => "decameters",
        DxfUnits::Hectometers => "hectometers",
        DxfUnits::Gigameters => "gigameters",
        DxfUnits::AstronomicalUnits => "astronomical units",
        DxfUnits::LightYears => "light years",
        DxfUnits::Parsecs => "parsecs",
        DxfUnits::USSurveyFeet => "US survey feet",
        DxfUnits::USSurveyInch => "US survey inches",
        DxfUnits::USSurveyYard => "US survey yards",
        DxfUnits::USSurveyMile => "US survey miles",
    }
}

fn export_units(units: Units) -> DxfUnits {
    match units {
        Units::Millimeters => DxfUnits::Millimeters,
        Units::Meters => DxfUnits::Meters,
        Units::Inches => DxfUnits::Inches,
    }
}

fn scaled_point(point: &DxfPoint, scale: f64) -> Result<Point2, DxfExchangeError> {
    scaled_xy(point.x, point.y, scale)
}

fn scaled_xy(x: f64, y: f64, scale: f64) -> Result<Point2, DxfExchangeError> {
    let point = Point2::new(x * scale, y * scale);
    if point.x.is_finite() && point.y.is_finite() {
        Ok(point)
    } else {
        Err(DxfExchangeError::InvalidInput(
            "DXF coordinate is non-finite after unit conversion".into(),
        ))
    }
}

fn dxf_point(point: Point2) -> DxfPoint {
    DxfPoint::new(point.x, point.y, 0.0)
}

fn point_is_planar(point: &DxfPoint) -> bool {
    point.x.is_finite() && point.y.is_finite() && point.z.is_finite() && near_zero(point.z)
}

fn vector_is_positive_z(vector: &Vector) -> bool {
    vector.x.is_finite()
        && vector.y.is_finite()
        && vector.z.is_finite()
        && near_zero(vector.x)
        && near_zero(vector.y)
        && (vector.z - 1.0).abs() <= PLANAR_EPSILON
}

fn near_zero(value: f64) -> bool {
    value.is_finite() && value.abs() <= PLANAR_EPSILON
}

fn point_distance_squared(left: Point2, right: Point2) -> f64 {
    (left.x - right.x).powi(2) + (left.y - right.y).powi(2)
}

fn dimension_offset(start: Point2, end: Point2, line_point: Point2) -> Option<f64> {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let measurement = dx.hypot(dy);
    if !measurement.is_finite() || measurement <= f64::EPSILON {
        return None;
    }
    let normal_x = -dy / measurement;
    let normal_y = dx / measurement;
    let offset = (line_point.x - start.x).mul_add(normal_x, (line_point.y - start.y) * normal_y);
    offset.is_finite().then_some(offset)
}

fn dimension_line(start: Point2, end: Point2, offset: f64) -> Option<(Point2, Point2)> {
    let measurement = (end.x - start.x).hypot(end.y - start.y);
    if !measurement.is_finite() || measurement <= f64::EPSILON || !offset.is_finite() {
        return None;
    }
    let normal = Point2::new(
        -(end.y - start.y) / measurement,
        (end.x - start.x) / measurement,
    );
    let delta = Point2::new(normal.x * offset, normal.y * offset);
    Some((
        Point2::new(start.x + delta.x, start.y + delta.y),
        Point2::new(end.x + delta.x, end.y + delta.y),
    ))
}

fn imported_entity_name(kind: &EntityKind, index: usize) -> String {
    let kind_name = match kind {
        EntityKind::Line { .. } => "Line",
        EntityKind::Circle { .. } => "Circle",
        EntityKind::Arc { .. } => "Arc",
        EntityKind::AlignedDimension { .. } => "Dimension",
        EntityKind::SketchProfile { .. } => "Polyline",
        EntityKind::Text { .. } => "Text",
        _ => "Entity",
    };
    format!("DXF {kind_name} {index}")
}

fn source_layer_key(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

fn import_layer_name(name: &str, fallback_index: usize) -> (String, bool) {
    let trimmed = name.trim();
    if is_valid_dxf_layer_name(trimmed) {
        (trimmed.to_owned(), trimmed != name)
    } else {
        (format!("DXF Layer {fallback_index}"), true)
    }
}

fn unique_import_layer_name(
    base: &str,
    occupied: &BTreeMap<String, LayerId>,
    id: LayerId,
) -> String {
    if !occupied.contains_key(&base.to_ascii_lowercase()) {
        return base.to_owned();
    }
    let fallback = format!("{base} ({id})");
    if !occupied.contains_key(&fallback.to_ascii_lowercase()) {
        return fallback;
    }
    let mut suffix = 2_u64;
    loop {
        let candidate = format!("{base} ({id}-{suffix})");
        if !occupied.contains_key(&candidate.to_ascii_lowercase()) {
            return candidate;
        }
        suffix += 1;
    }
}

fn is_valid_dxf_layer_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_DXF_LAYER_NAME_CHARS
        && !name
            .chars()
            .any(|character| character.is_control() || "<>/\\\":;?*|=".contains(character))
}

fn sanitized_dxf_layer_name(name: &str, id: LayerId) -> String {
    let mut value = name
        .trim()
        .chars()
        .map(|character| {
            if character.is_control() || "<>/\\\":;?*|=".contains(character) {
                '_'
            } else {
                character
            }
        })
        .take(MAX_DXF_LAYER_NAME_CHARS)
        .collect::<String>();
    if value.is_empty() {
        value = format!("CADX Layer {id}");
    }
    value
}

fn unique_export_layer_name(base: &str, id: LayerId, used: &BTreeSet<String>) -> String {
    if !used.contains(&base.to_ascii_lowercase()) {
        return base.to_owned();
    }
    let mut suffix = format!("_{id}");
    let keep = MAX_DXF_LAYER_NAME_CHARS.saturating_sub(suffix.chars().count());
    let mut candidate = format!("{}{suffix}", base.chars().take(keep).collect::<String>());
    let mut attempt = 2;
    while used.contains(&candidate.to_ascii_lowercase()) {
        suffix = format!("_{id}_{attempt}");
        let keep = MAX_DXF_LAYER_NAME_CHARS.saturating_sub(suffix.chars().count());
        candidate = format!("{}{suffix}", base.chars().take(keep).collect::<String>());
        attempt += 1;
    }
    candidate
}

fn aci_rgba(index: u8) -> [u8; 4] {
    let [red, green, blue] = aci_rgb(index);
    [red, green, blue, 255]
}

fn aci_rgb(index: u8) -> [u8; 3] {
    match index {
        1 => [255, 0, 0],
        2 => [255, 255, 0],
        3 => [0, 255, 0],
        4 => [0, 255, 255],
        5 => [0, 0, 255],
        6 => [255, 0, 255],
        7 => [255, 255, 255],
        8 => [128, 128, 128],
        9 => [192, 192, 192],
        10..=249 => {
            let offset = usize::from(index - 10);
            let hue = ((offset / 10) as f64) * 15.0;
            let variant = offset % 10;
            let saturation = if variant % 2 == 0 { 1.0 } else { 0.5 };
            let value = match variant / 2 {
                0 => 1.0,
                1 => 0.65,
                2 => 0.5,
                3 => 0.3,
                _ => 0.15,
            };
            hsv_rgb(hue, saturation, value)
        }
        250 => [51, 51, 51],
        251 => [80, 80, 80],
        252 => [105, 105, 105],
        253 => [130, 130, 130],
        254 => [190, 190, 190],
        255 => [255, 255, 255],
        _ => [255, 255, 255],
    }
}

fn hsv_rgb(hue: f64, saturation: f64, value: f64) -> [u8; 3] {
    let chroma = value * saturation;
    let sector = hue / 60.0;
    let secondary = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let (red, green, blue) = match sector as u8 {
        0 => (chroma, secondary, 0.0),
        1 => (secondary, chroma, 0.0),
        2 => (0.0, chroma, secondary),
        3 => (0.0, secondary, chroma),
        4 => (secondary, 0.0, chroma),
        _ => (chroma, 0.0, secondary),
    };
    let match_value = value - chroma;
    [
        ((red + match_value) * 255.0).round() as u8,
        ((green + match_value) * 255.0).round() as u8,
        ((blue + match_value) * 255.0).round() as u8,
    ]
}

fn nearest_aci(color: [u8; 4]) -> u8 {
    (1_u8..=255)
        .min_by_key(|index| {
            let candidate = aci_rgb(*index);
            let red = i32::from(color[0]) - i32::from(candidate[0]);
            let green = i32::from(color[1]) - i32::from(candidate[1]);
            let blue = i32::from(color[2]) - i32::from(candidate[2]);
            red * red + green * green + blue * blue
        })
        .expect("the ACI palette is non-empty")
}
