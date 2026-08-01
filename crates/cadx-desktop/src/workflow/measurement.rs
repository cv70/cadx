use cadx_analysis::{
    LengthPrecision, MeasurementEntity, MeasurementError, MeasurementResult, measure,
};
use cadx_core::kernel::EvaluatedScene;
use eframe::egui;

use crate::{CadxApp, MeasurementState, appearance, icon_button};

#[derive(Default)]
pub(crate) struct MeasurementTopology {
    pub(crate) faces: Vec<cadx_core::topology::FaceRef>,
    pub(crate) edges: Vec<cadx_core::topology::EdgeRef>,
    pub(crate) vertices: Vec<cadx_core::topology::VertexRef>,
    pub(crate) guides: Vec<[[f64; 3]; 2]>,
}

impl MeasurementState {
    fn add(&mut self, entity: MeasurementEntity) {
        if self.entities.last() == Some(&entity) {
            return;
        }
        if self
            .entities
            .first()
            .is_some_and(|first| first.kind() != entity.kind())
            || self.entities.len() == 2
        {
            self.entities.clear();
        }
        self.entities.push(entity);
    }

    fn reconcile(&mut self, scene: &EvaluatedScene) {
        self.entities.retain(|entity| match entity {
            MeasurementEntity::Face(reference) => scene.face(reference).is_some(),
            MeasurementEntity::Edge(reference) => scene.edge(reference).is_some(),
            MeasurementEntity::Vertex(reference) => scene.vertex(reference).is_some(),
        });
    }

    pub(crate) fn result(
        &self,
        scene: &EvaluatedScene,
    ) -> Result<MeasurementResult, MeasurementError> {
        measure(scene, &self.entities)
    }

    pub(crate) fn topology(&self, scene: &EvaluatedScene) -> MeasurementTopology {
        if !self.active {
            return MeasurementTopology::default();
        }
        let mut faces = Vec::new();
        let mut edges = Vec::new();
        let mut vertices = Vec::new();
        for entity in &self.entities {
            match entity {
                MeasurementEntity::Face(reference) => faces.push(reference.clone()),
                MeasurementEntity::Edge(reference) => edges.push(reference.clone()),
                MeasurementEntity::Vertex(reference) => vertices.push(reference.clone()),
            }
        }
        let guides = if let [first, second] = vertices.as_slice()
            && let (Some(first), Some(second)) = (scene.vertex(first), scene.vertex(second))
        {
            vec![[first.geometry.position, second.geometry.position]]
        } else {
            Vec::new()
        };
        MeasurementTopology {
            faces,
            edges,
            vertices,
            guides,
        }
    }
}

impl CadxApp {
    pub(crate) fn toggle_measurement(&mut self) {
        self.measurement.active = !self.measurement.active;
        if !self.measurement.active {
            self.measurement.entities.clear();
        }
        self.sync_viewport();
    }

    pub(crate) fn add_measurement_entity(&mut self, entity: MeasurementEntity) {
        self.measurement.add(entity);
    }

    pub(crate) fn reconcile_measurement(&mut self) {
        self.measurement.reconcile(self.session.scene());
    }

    pub(crate) fn clear_measurement(&mut self) {
        self.measurement.entities.clear();
    }

    pub(crate) fn measurement_panel(&mut self, context: &egui::Context) {
        if !self.measurement.active {
            return;
        }
        let translator = self.translator.clone();
        let mut open = true;
        egui::Window::new(translator.text("measurement.title"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(310.0)
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.label(appearance::icon("ruler", 14.0).color(appearance::ACCENT));
                    let selected = self.measurement.entities.len().to_string();
                    ui.label(
                        egui::RichText::new(
                            translator
                                .format("measurement.selection_count", &[("selected", &selected)]),
                        )
                        .size(11.0)
                        .color(appearance::TEXT_MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icon_button(
                            ui,
                            "trash-2",
                            translator.text("measurement.clear"),
                            !self.measurement.entities.is_empty(),
                            false,
                        )
                        .clicked()
                        {
                            self.clear_measurement();
                            self.sync_viewport();
                        }
                    });
                });
                if !self.measurement.entities.is_empty() {
                    ui.add_space(6.0);
                    for (index, entity) in self.measurement.entities.iter().enumerate() {
                        measurement_entity_row(ui, index, entity, &translator);
                    }
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(6.0);
                    match self.measurement.result(self.session.scene()) {
                        Ok(result) => measurement_result(ui, &result, &translator),
                        Err(MeasurementError::UnsupportedSelection) => {}
                        Err(error) => {
                            ui.label(
                                egui::RichText::new(translator.text(measurement_error_key(&error)))
                                    .size(11.0)
                                    .color(appearance::DANGER),
                            );
                        }
                    }
                }
            });
        if !open {
            self.measurement.active = false;
            self.measurement.entities.clear();
            self.sync_viewport();
        }
    }
}

fn measurement_entity_row(
    ui: &mut egui::Ui,
    index: usize,
    entity: &MeasurementEntity,
    translator: &cadx_i18n::Translator,
) {
    let (key, id, fragment) = match entity {
        MeasurementEntity::Face(reference) => {
            ("measurement.entity_face", reference.feature_id, None)
        }
        MeasurementEntity::Edge(reference) => (
            "measurement.entity_edge",
            reference.feature_id,
            Some(reference.fragment),
        ),
        MeasurementEntity::Vertex(reference) => (
            "measurement.entity_vertex",
            reference.feature_id,
            Some(reference.fragment),
        ),
    };
    let ordinal = (index + 1).to_string();
    let id = id.to_string();
    let fragment = fragment.map_or_else(|| "-".into(), |value| value.to_string());
    ui.label(
        egui::RichText::new(translator.format(
            key,
            &[("index", &ordinal), ("id", &id), ("fragment", &fragment)],
        ))
        .monospace()
        .size(10.0)
        .color(appearance::TEXT_MUTED),
    );
}

fn measurement_result(
    ui: &mut egui::Ui,
    result: &MeasurementResult,
    translator: &cadx_i18n::Translator,
) {
    match result {
        MeasurementResult::EdgeLength {
            length_mm,
            precision,
            ..
        } => {
            measurement_value(
                ui,
                translator.text("measurement.length"),
                &format!("{length_mm:.6} mm"),
            );
            let precision = match precision {
                LengthPrecision::Exact => translator.text("measurement.precision_exact").into(),
                LengthPrecision::Numerical { estimated_error_mm } => translator.format(
                    "measurement.precision_numerical",
                    &[("error", &format!("{estimated_error_mm:.2e}"))],
                ),
            };
            measurement_value(ui, translator.text("measurement.precision"), &precision);
        }
        MeasurementResult::PointDistance {
            delta_mm,
            distance_mm,
            ..
        } => {
            measurement_value(
                ui,
                translator.text("measurement.distance"),
                &format!("{distance_mm:.6} mm"),
            );
            measurement_value(
                ui,
                translator.text("measurement.delta"),
                &format!(
                    "X {:.6}  Y {:.6}  Z {:.6} mm",
                    delta_mm[0], delta_mm[1], delta_mm[2]
                ),
            );
        }
        MeasurementResult::LinearEdgeAngle { angle_degrees, .. } => measurement_value(
            ui,
            translator.text("measurement.angle"),
            &format!("{angle_degrees:.6} deg"),
        ),
        MeasurementResult::PlanarFaceRelationship {
            angle_degrees,
            parallel_distance_mm,
            ..
        } => {
            measurement_value(
                ui,
                translator.text("measurement.angle"),
                &format!("{angle_degrees:.6} deg"),
            );
            if let Some(distance) = parallel_distance_mm {
                measurement_value(
                    ui,
                    translator.text("measurement.plane_spacing"),
                    &format!("{distance:.6} mm"),
                );
            }
        }
    }
}

fn measurement_value(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(
        egui::RichText::new(label)
            .size(9.0)
            .color(appearance::TEXT_FAINT),
    );
    ui.label(
        egui::RichText::new(value)
            .monospace()
            .size(11.0)
            .color(appearance::TEXT),
    );
    ui.add_space(4.0);
}

const fn measurement_error_key(error: &MeasurementError) -> &'static str {
    match error {
        MeasurementError::UnsupportedSelection => "measurement.error_selection",
        MeasurementError::LostTopology { .. } => "measurement.error_lost",
        MeasurementError::AmbiguousTopology { .. } => "measurement.error_ambiguous",
        MeasurementError::LengthAccuracyUnavailable => "measurement.error_accuracy",
        MeasurementError::NonLinearEdge => "measurement.error_non_linear",
        MeasurementError::NonPlanarFace => "measurement.error_non_planar",
        MeasurementError::DegenerateGeometry => "measurement.error_degenerate",
    }
}

#[cfg(test)]
mod tests {
    use cadx_core::topology::{FaceRef, PrimitiveFace};

    use super::*;

    #[test]
    fn measurement_selection_restarts_for_a_new_kind_or_completed_pair() {
        let face = MeasurementEntity::Face(FaceRef::primitive(1, PrimitiveFace::BoxXMin));
        let other_face = MeasurementEntity::Face(FaceRef::primitive(1, PrimitiveFace::BoxXMax));
        let edge = MeasurementEntity::Edge(cadx_core::topology::EdgeRef::new(
            1,
            FaceRef::primitive(1, PrimitiveFace::BoxXMin),
            FaceRef::primitive(1, PrimitiveFace::BoxYMin),
            0,
        ));
        let mut state = MeasurementState::default();
        state.add(face.clone());
        state.add(other_face);
        assert_eq!(state.entities.len(), 2);
        state.add(face);
        assert_eq!(state.entities.len(), 1);
        state.add(edge.clone());
        assert_eq!(state.entities, vec![edge]);
    }
}
