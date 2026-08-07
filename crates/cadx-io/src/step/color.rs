use std::collections::{BTreeMap, HashSet};

use ruststep::ast::{EntityInstance, Parameter, Record};

use super::ast::{
    collect_entity_refs, entity_by_id, entity_records, parameter_list, parameter_ref,
};

/// Result of reducing STEP presentation styles to CADX's per-body color model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StepBodyColor {
    /// The body and its faces have no supported color styles.
    Absent,
    /// One complete, unambiguous body color in source sRGB space.
    Uniform([f32; 4]),
    /// Styles exist but are malformed, conflicting, mixed, or only partially
    /// cover the body's faces and cannot be represented as one body color.
    Unsupported,
}

impl StepBodyColor {
    #[must_use]
    pub const fn color(self) -> Option<[f32; 4]> {
        match self {
            Self::Uniform(color) => Some(color),
            Self::Absent | Self::Unsupported => None,
        }
    }

    #[must_use]
    pub const fn is_unsupported(self) -> bool {
        matches!(self, Self::Unsupported)
    }
}

#[derive(Debug, Default)]
struct StyleEvidence {
    colors: Vec<[f32; 3]>,
    transparencies: Vec<f32>,
    invalid: bool,
}

#[derive(Debug, Default)]
struct TargetStyles {
    roots: Vec<u64>,
    invalid: bool,
}

impl StyleEvidence {
    fn merge(&mut self, mut other: Self) {
        self.colors.append(&mut other.colors);
        self.transparencies.append(&mut other.transparencies);
        self.invalid |= other.invalid;
    }

    fn resolve(self) -> StepBodyColor {
        if self.invalid {
            return StepBodyColor::Unsupported;
        }
        let Some(color) = one_rgb(&self.colors) else {
            return if self.colors.is_empty() {
                StepBodyColor::Absent
            } else {
                StepBodyColor::Unsupported
            };
        };
        let transparency = if self.transparencies.is_empty() {
            0.0
        } else {
            let first = self.transparencies[0];
            if self
                .transparencies
                .iter()
                .any(|value| !channel_matches(*value, first))
            {
                return StepBodyColor::Unsupported;
            }
            first
        };
        StepBodyColor::Uniform([color[0], color[1], color[2], 1.0 - transparency])
    }
}

pub(super) fn resolve_body_color(
    entities: &[EntityInstance],
    body_target: Option<u64>,
    boundary_targets: &[(u64, Option<u64>)],
) -> StepBodyColor {
    let styles = styled_targets(entities);
    let body_color = body_target.map_or(StepBodyColor::Absent, |target| {
        resolve_target_color(entities, &styles, target)
    });
    if matches!(body_color, StepBodyColor::Unsupported) {
        return StepBodyColor::Unsupported;
    }
    let boundary_colors = boundary_targets
        .iter()
        .map(|(shell_id, oriented_target)| {
            resolve_shell_color(entities, &styles, *shell_id, *oriented_target)
        })
        .collect::<Vec<_>>();

    match body_color {
        StepBodyColor::Uniform(color) => {
            if boundary_colors.iter().any(|boundary| match boundary {
                StepBodyColor::Absent => false,
                StepBodyColor::Uniform(candidate) => !rgba_matches(*candidate, color),
                StepBodyColor::Unsupported => true,
            }) {
                StepBodyColor::Unsupported
            } else {
                StepBodyColor::Uniform(color)
            }
        }
        StepBodyColor::Unsupported => StepBodyColor::Unsupported,
        StepBodyColor::Absent => promote_complete_uniform(boundary_colors),
    }
}

fn resolve_shell_color(
    entities: &[EntityInstance],
    styles: &BTreeMap<u64, TargetStyles>,
    shell_id: u64,
    oriented_target: Option<u64>,
) -> StepBodyColor {
    let direct = combine_resolutions(
        std::iter::once(resolve_target_color(entities, styles, shell_id))
            .chain(oriented_target.map(|target| resolve_target_color(entities, styles, target))),
    );
    if matches!(direct, StepBodyColor::Unsupported) {
        return StepBodyColor::Unsupported;
    }
    let Some(shell) = entity_by_id(entities, shell_id) else {
        return StepBodyColor::Absent;
    };
    let face_ids = entity_records(shell)
        .into_iter()
        .find(|record| record.name == "CLOSED_SHELL")
        .map(|record| referenced_entities(&record.parameter))
        .unwrap_or_default();
    if face_ids.is_empty() {
        return StepBodyColor::Absent;
    }

    let face_colors = face_ids
        .into_iter()
        .map(|face_id| resolve_face_color(entities, styles, face_id))
        .collect::<Vec<_>>();
    match direct {
        StepBodyColor::Uniform(color) => {
            if face_colors.iter().any(|face| match face {
                StepBodyColor::Absent => false,
                StepBodyColor::Uniform(candidate) => !rgba_matches(*candidate, color),
                StepBodyColor::Unsupported => true,
            }) {
                StepBodyColor::Unsupported
            } else {
                StepBodyColor::Uniform(color)
            }
        }
        StepBodyColor::Absent => promote_complete_uniform(face_colors),
        StepBodyColor::Unsupported => StepBodyColor::Unsupported,
    }
}

fn promote_complete_uniform(colors: Vec<StepBodyColor>) -> StepBodyColor {
    if colors
        .iter()
        .all(|color| matches!(color, StepBodyColor::Absent))
    {
        StepBodyColor::Absent
    } else if colors
        .iter()
        .any(|color| !matches!(color, StepBodyColor::Uniform(_)))
    {
        StepBodyColor::Unsupported
    } else {
        combine_resolutions(colors)
    }
}

fn referenced_entities(parameter: &Parameter) -> Vec<u64> {
    let mut refs = Vec::new();
    collect_entity_refs(parameter, &mut refs);
    refs
}

fn styled_targets(entities: &[EntityInstance]) -> BTreeMap<u64, TargetStyles> {
    let mut styles = BTreeMap::<u64, TargetStyles>::new();
    for entity in entities {
        for record in entity_records(entity) {
            if record.name != "STYLED_ITEM" {
                continue;
            }
            let Some(values) = parameter_list(&record.parameter) else {
                continue;
            };
            let Some(target) = values.get(2).and_then(parameter_ref) else {
                continue;
            };
            let target_styles = styles.entry(target).or_default();
            if values.len() != 3 {
                target_styles.invalid = true;
                continue;
            }
            let Some(style_values) = values.get(1).and_then(parameter_list) else {
                target_styles.invalid = true;
                continue;
            };
            let style_refs = style_values
                .iter()
                .filter_map(parameter_ref)
                .collect::<Vec<_>>();
            target_styles.invalid |=
                style_refs.len() != style_values.len() || style_refs.is_empty();
            target_styles.roots.extend(style_refs);
        }
    }
    styles
}

fn resolve_face_color(
    entities: &[EntityInstance],
    styles: &BTreeMap<u64, TargetStyles>,
    face_id: u64,
) -> StepBodyColor {
    let mut targets = vec![face_id];
    if let Some(entity) = entity_by_id(entities, face_id) {
        for record in entity_records(entity) {
            if record.name == "ORIENTED_FACE" {
                targets.extend(
                    referenced_entities(&record.parameter)
                        .into_iter()
                        .filter(|id| {
                            entity_by_id(entities, *id).is_some_and(|candidate| {
                                entity_records(candidate)
                                    .into_iter()
                                    .any(|candidate_record| {
                                        matches!(
                                            candidate_record.name.as_str(),
                                            "ADVANCED_FACE" | "FACE_SURFACE"
                                        )
                                    })
                            })
                        }),
                );
            }
        }
    }
    combine_resolutions(
        targets
            .into_iter()
            .map(|target| resolve_target_color(entities, styles, target)),
    )
}

fn resolve_target_color(
    entities: &[EntityInstance],
    styles: &BTreeMap<u64, TargetStyles>,
    target: u64,
) -> StepBodyColor {
    let Some(styles) = styles.get(&target) else {
        return StepBodyColor::Absent;
    };
    if styles.invalid {
        return StepBodyColor::Unsupported;
    }
    let mut evidence = StyleEvidence::default();
    for root in &styles.roots {
        evidence.merge(resolve_style_evidence(entities, *root, &mut HashSet::new()));
    }
    match evidence.resolve() {
        StepBodyColor::Absent => StepBodyColor::Unsupported,
        resolved => resolved,
    }
}

fn resolve_style_evidence(
    entities: &[EntityInstance],
    id: u64,
    visiting: &mut HashSet<u64>,
) -> StyleEvidence {
    if !visiting.insert(id) {
        return StyleEvidence {
            invalid: true,
            ..StyleEvidence::default()
        };
    }
    let Some(entity) = entity_by_id(entities, id) else {
        visiting.remove(&id);
        return StyleEvidence {
            invalid: true,
            ..StyleEvidence::default()
        };
    };
    let mut evidence = StyleEvidence::default();
    for record in entity_records(entity) {
        match record.name.as_str() {
            "COLOUR_RGB" => match rgb(record) {
                Some(color) => evidence.colors.push(color),
                None => evidence.invalid = true,
            },
            "DRAUGHTING_PRE_DEFINED_COLOUR" => match predefined_color(record) {
                Some(color) => evidence.colors.push(color),
                None => evidence.invalid = true,
            },
            "SURFACE_STYLE_TRANSPARENT" => match direct_numbers(record).first().copied() {
                Some(value) if (0.0..=1.0).contains(&value) => {
                    evidence.transparencies.push(value);
                }
                _ => evidence.invalid = true,
            },
            "SURFACE_STYLE_RENDERING" | "SURFACE_STYLE_RENDERING_WITH_PROPERTIES" => {
                let Some(values) = parameter_list(&record.parameter) else {
                    evidence.invalid = true;
                    continue;
                };
                if let Some(color_id) = values.first().and_then(parameter_ref) {
                    evidence.merge(resolve_style_evidence(entities, color_id, visiting));
                } else {
                    evidence.invalid = true;
                }
                if let Some(transparency) = values.get(1).and_then(direct_number) {
                    if (0.0..=1.0).contains(&transparency) {
                        evidence.transparencies.push(transparency);
                    } else {
                        evidence.invalid = true;
                    }
                }
            }
            name if is_style_container(name) => {
                for child in referenced_entities(&record.parameter) {
                    evidence.merge(resolve_style_evidence(entities, child, visiting));
                }
            }
            _ => {}
        }
    }
    visiting.remove(&id);
    evidence
}

fn is_style_container(name: &str) -> bool {
    matches!(
        name,
        "PRESENTATION_STYLE_ASSIGNMENT"
            | "SURFACE_STYLE_USAGE"
            | "SURFACE_SIDE_STYLE"
            | "SURFACE_STYLE_FILL_AREA"
            | "FILL_AREA_STYLE"
            | "FILL_AREA_STYLE_COLOUR"
            | "SURFACE_STYLE_SHADING"
    )
}

fn direct_numbers(record: &Record) -> Vec<f32> {
    parameter_list(&record.parameter)
        .into_iter()
        .flatten()
        .filter_map(direct_number)
        .collect()
}

#[allow(clippy::cast_possible_truncation)]
fn direct_number(parameter: &Parameter) -> Option<f32> {
    match parameter {
        Parameter::Real(value) if value.is_finite() => Some(*value as f32),
        Parameter::Integer(value) => i16::try_from(*value).ok().map(f32::from),
        _ => None,
    }
}

fn rgb(record: &Record) -> Option<[f32; 3]> {
    let values = direct_numbers(record);
    let color: [f32; 3] = values.try_into().ok()?;
    color
        .iter()
        .all(|channel| (0.0..=1.0).contains(channel))
        .then_some(color)
}

fn predefined_color(record: &Record) -> Option<[f32; 3]> {
    let name = parameter_list(&record.parameter)?
        .iter()
        .find_map(|parameter| match parameter {
            Parameter::String(name) => Some(name.to_ascii_lowercase()),
            _ => None,
        })?;
    Some(match name.as_str() {
        "black" => [0.0, 0.0, 0.0],
        "white" => [1.0, 1.0, 1.0],
        "red" => [1.0, 0.0, 0.0],
        "green" => [0.0, 1.0, 0.0],
        "blue" => [0.0, 0.0, 1.0],
        "yellow" => [1.0, 1.0, 0.0],
        "magenta" => [1.0, 0.0, 1.0],
        "cyan" => [0.0, 1.0, 1.0],
        "grey" | "gray" => [0.5, 0.5, 0.5],
        _ => return None,
    })
}

fn combine_resolutions(values: impl IntoIterator<Item = StepBodyColor>) -> StepBodyColor {
    let mut color = None;
    for value in values {
        match value {
            StepBodyColor::Absent => {}
            StepBodyColor::Unsupported => return StepBodyColor::Unsupported,
            StepBodyColor::Uniform(candidate) => {
                if color.is_some_and(|existing| !rgba_matches(existing, candidate)) {
                    return StepBodyColor::Unsupported;
                }
                color = Some(candidate);
            }
        }
    }
    color.map_or(StepBodyColor::Absent, StepBodyColor::Uniform)
}

fn one_rgb(colors: &[[f32; 3]]) -> Option<[f32; 3]> {
    let first = *colors.first()?;
    colors
        .iter()
        .all(|color| rgb_matches(*color, first))
        .then_some(first)
}

fn rgba_matches(left: [f32; 4], right: [f32; 4]) -> bool {
    left.into_iter()
        .zip(right)
        .all(|(left, right)| channel_matches(left, right))
}

fn rgb_matches(left: [f32; 3], right: [f32; 3]) -> bool {
    left.into_iter()
        .zip(right)
        .all(|(left, right)| channel_matches(left, right))
}

fn channel_matches(left: f32, right: f32) -> bool {
    (left - right).abs() <= 1.0e-6
}

#[cfg(test)]
mod tests {
    use super::StepBodyColor;
    use crate::step::test_support::{VALID_STEP, read_source};

    #[test]
    fn shell_style_on_only_one_boundary_is_not_flattened() {
        let source = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            "#20=CLOSED_SHELL('outer',(#90));\n\
             #21=CLOSED_SHELL('void',(#91));\n\
             #22=ORIENTED_CLOSED_SHELL('',*,#21,.T.);\n\
             #23=BREP_WITH_VOIDS('Partially styled',#20,(#22));\n\
             #30=COLOUR_RGB('',0.15,0.3,0.75);\n\
             #31=SURFACE_STYLE_SHADING(#30);\n\
             #32=PRESENTATION_STYLE_ASSIGNMENT((#31));\n\
             #33=STYLED_ITEM('',(#32),#20);",
        );
        let imported = read_source(&source);
        assert_eq!(imported.bodies.len(), 1);
        assert_eq!(imported.bodies[0].color, StepBodyColor::Unsupported);
    }

    #[test]
    fn reads_solid_level_ap214_color_and_transparency() {
        let source = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            "#20=CLOSED_SHELL('',(#99));\n\
             #21=MANIFOLD_SOLID_BREP('Painted housing',#20);\n\
             #30=COLOUR_RGB('Supplier blue',0.1,0.2,0.8);\n\
             #31=SURFACE_STYLE_RENDERING(#30,0.25);\n\
             #32=PRESENTATION_STYLE_ASSIGNMENT((#31));\n\
             #33=STYLED_ITEM('',(#32),#21);",
        );
        let imported = read_source(&source);
        assert_eq!(
            imported.bodies[0].color,
            StepBodyColor::Uniform([0.1, 0.2, 0.8, 0.75])
        );
    }

    #[test]
    fn promotes_only_complete_uniform_face_color_to_a_body_color() {
        let style = "#30=COLOUR_RGB('',0.8,0.3,0.1);\n\
                     #31=FILL_AREA_STYLE_COLOUR('',#30);\n\
                     #32=FILL_AREA_STYLE('',(#31));\n\
                     #33=SURFACE_STYLE_FILL_AREA(#32);\n\
                     #34=SURFACE_SIDE_STYLE('',(#33));\n\
                     #35=SURFACE_STYLE_USAGE(.BOTH.,#34);\n\
                     #36=PRESENTATION_STYLE_ASSIGNMENT((#35));";
        let geometry = "#20=CLOSED_SHELL('',(#21,#22));\n\
                        #21=ADVANCED_FACE('',(#91),#92,.T.);\n\
                        #22=ADVANCED_FACE('',(#93),#94,.T.);\n\
                        #23=MANIFOLD_SOLID_BREP('Uniform faces',#20);";
        let complete = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            &format!(
                "{geometry}\n{style}\n#37=STYLED_ITEM('',(#36),#21);\n#38=STYLED_ITEM('',(#36),#22);"
            ),
        );
        let partial = VALID_STEP.replace(
            "#1=CARTESIAN_POINT('',(0.,0.,0.));",
            &format!("{geometry}\n{style}\n#37=STYLED_ITEM('',(#36),#21);"),
        );

        assert_eq!(
            read_source(&complete).bodies[0].color,
            StepBodyColor::Uniform([0.8, 0.3, 0.1, 1.0])
        );
        assert_eq!(
            read_source(&partial).bodies[0].color,
            StepBodyColor::Unsupported
        );
    }

    #[test]
    fn preserves_malformed_or_unrecognized_style_attachments_as_unsupported() {
        let geometry = "#20=CLOSED_SHELL('',(#21));\n\
                        #21=ADVANCED_FACE('',(#91),#92,.T.);\n\
                        #22=MANIFOLD_SOLID_BREP('Styled body',#20);";
        let style_cases = [
            "#30=STYLED_ITEM('',('not a style reference'),#22);",
            "#30=STYLED_ITEM('',(#999),#22,'extra');",
            "#30=CURVE_STYLE('',#99,$,#98);\n#31=STYLED_ITEM('',(#30),#22);",
            "#30=PRESENTATION_STYLE_ASSIGNMENT((#999));\n#31=STYLED_ITEM('',(#30),#22);",
        ];

        for style in style_cases {
            let source = VALID_STEP.replace(
                "#1=CARTESIAN_POINT('',(0.,0.,0.));",
                &format!("{geometry}\n{style}"),
            );
            assert_eq!(
                read_source(&source).bodies[0].color,
                StepBodyColor::Unsupported
            );
        }
    }
}
