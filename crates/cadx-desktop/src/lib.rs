mod appearance;
mod workflow;

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Arc, mpsc},
    time::Duration,
};

use eframe::egui;

use cadx_aec::AecPack;
use cadx_ai::{
    AiAssistant, AiError, AiPlan, GenAiAssistant,
    context::{ContextCollector, ContextSnapshot},
    intent::IntentDiff,
    tools::ToolRegistry,
};
use cadx_analysis::{MeasurementEntity, analyze_scene};
use cadx_app::{CoreBus, DocumentState, TransactionSource};
use cadx_config::{ConfigStore, Preferences, ProviderConfig, Settings};
use cadx_core::{
    assembly::{
        Assembly, AssemblyMate, AssemblyMateKind, AssemblyMateLimits, AssemblyTransform,
        ComponentOccurrenceId, StepEntityRef,
    },
    diagnostics::{
        BooleanDiagnostic, BooleanFailureReason, BooleanFailureStage, EdgeModifierDiagnostic,
        EdgeModifierFailureReason, EdgeModifierFailureStage, EdgeModifierParameter,
        SketchConstraintDiagnostic, SketchConstraintFailureReason,
    },
    domain::{
        BooleanOperation, CadDocument, Constraint, Feature, FeatureId, MAX_CONSTRUCTION_SEGMENTS,
        MAX_LOFT_SECTIONS, ModelCommand, Primitive, SketchDimensionKind, SketchLoop2D, SketchPlane,
        SketchRegion2D, SketchSegment2D, construction_point_ids, construction_segment_id,
    },
    kernel::{
        CadKernel, EdgeCountSupport, EdgeModifierCapability, EvaluatedScene, ExchangeKernel,
        InterferenceAnalysis, InterferenceFailureReason, InterferenceFailureStage,
        InterferencePairOutcome, SketchSolveDiagnostic,
    },
    topology::{EdgeRef, FaceRef, VertexRef},
};
use cadx_domain_api::{
    DomainAction, DomainArtifact, DomainContext, DomainExecution, DomainFieldKind,
    DomainFieldSchema, DomainFieldValue, DomainId, DomainParameters, DomainRegistry, DomainRoute,
    DomainToolRequest,
};
use cadx_ecad::{EcadPack, drc as pcb_drc, export as pcb_export, layout::PcbBoard};
use cadx_i18n::{Language, Translator};
use cadx_kernel_truck::TruckKernel;
use cadx_mcad::{
    McadPack, bom as mechanical_bom, dfm as mechanical_dfm, standards as mechanical_standards,
};
use cadx_render::{self as render, OrbitCamera, ViewportScene};

pub struct CadxApp {
    session: CoreBus,
    exchange_kernel: Arc<dyn ExchangeKernel>,
    viewport_scene: ViewportScene,
    camera: OrbitCamera,
    selected: Option<FeatureId>,
    selected_face: Option<FaceRef>,
    selected_edges: Vec<EdgeRef>,
    selected_vertex: Option<VertexRef>,
    selection_mode: SelectionMode,
    measurement: MeasurementState,
    status: StatusMessage,
    assistant: Arc<dyn AiAssistant>,
    runtime: tokio::runtime::Runtime,
    ai_sender: mpsc::Sender<Result<AiPlan, AiError>>,
    ai_receiver: mpsc::Receiver<Result<AiPlan, AiError>>,
    ai_input: String,
    ai_pending: bool,
    pending_ai_plan: Option<AiPlan>,
    conversation: Vec<ConversationEntry>,
    translator: Translator,
    toolbar_tab: ToolbarTab,
    model_panel_open: bool,
    ai_panel_open: bool,
    document_path: Option<PathBuf>,
    loft_dialog: Option<LoftDialogState>,
    boolean_dialog: Option<BooleanDialogState>,
    edge_modifier_dialog: Option<EdgeModifierDialogState>,
    interference_dialog: Option<InterferenceDialogState>,
    last_boolean_failure: Option<BooleanDiagnostic>,
    last_edge_modifier_failure: Option<EdgeModifierDiagnostic>,
    last_sketch_failure: Option<SketchConstraintDiagnostic>,
    last_sketch_failure_feature: Option<FeatureId>,
    sketch_dimension_editor: Option<SketchDimensionEditor>,
    preferences: Preferences,
    config_store: Option<ConfigStore>,
    active_domain: DomainId,
    domain_bus: DomainRegistry,
    ai_tools: ToolRegistry,
    context_collector: ContextCollector,
    domain_tool_form: Option<(DomainId, String)>,
    domain_form_values: BTreeMap<(DomainId, String), DomainParameters>,
    pcb_board: PcbBoard,
    domain_report: Option<DomainReport>,
    last_intent_diff: Option<IntentDiff>,
}

#[derive(Debug, Clone, Copy)]
enum Speaker {
    User,
    Assistant,
    Error,
}

enum LocalizedText {
    Key(&'static str),
    Text(String),
}

impl LocalizedText {
    fn resolve<'a>(&'a self, translator: &'a Translator) -> &'a str {
        match self {
            Self::Key(key) => translator.text(key),
            Self::Text(text) => text,
        }
    }
}

type StatusMessage = LocalizedText;

struct ConversationEntry {
    speaker: Speaker,
    content: LocalizedText,
}

#[derive(Debug, Clone)]
enum DomainReport {
    MechanicalDfm(mechanical_dfm::DfmReport),
    MechanicalDrawing(Vec<mechanical_standards::StandardsIssue>),
    MechanicalBom(Vec<mechanical_bom::BomItem>),
    PcbDrc(pcb_drc::DrcReport),
    PcbBom(Vec<(String, String, String, u32)>),
    ExportPreview(Vec<pcb_export::ExportFile>),
    Artifacts(Vec<DomainArtifact>),
}

struct InterferenceDialogState {
    result: Result<InterferenceAnalysis, String>,
}

#[derive(Clone)]
struct AssemblyInspectorContext {
    assembly_id: u64,
    assembly_name: String,
    occurrence_id: ComponentOccurrenceId,
    occurrence_name: String,
    parent_occurrence_id: Option<ComponentOccurrenceId>,
    occurrence_transform: AssemblyTransform,
    occurrence_suppressed: bool,
    occurrence_effectively_suppressed: bool,
    occurrence_source: Option<StepEntityRef>,
    definition_name: Option<String>,
    definition_source: Option<StepEntityRef>,
    mate: Option<AssemblyMate>,
    next_mate_id: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolbarTab {
    Create,
    Design,
    View,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionMode {
    Face,
    Edge,
    Vertex,
}

fn update_edge_selection(selected: &mut Vec<EdgeRef>, picked: Option<&EdgeRef>, additive: bool) {
    if !additive {
        selected.clear();
    }
    let Some(picked) = picked else {
        return;
    };
    if selected
        .first()
        .is_some_and(|edge| edge.feature_id != picked.feature_id)
    {
        selected.clear();
    }
    if let Some(index) = selected.iter().position(|edge| edge == picked) {
        if additive {
            selected.remove(index);
        }
    } else {
        selected.push(picked.clone());
        selected.sort_unstable();
    }
}

fn edge_modifier_tool_enabled(capability: EdgeModifierCapability, edge_count: usize) -> bool {
    edge_count > 0
        && match capability.edge_count {
            EdgeCountSupport::Unsupported => false,
            EdgeCountSupport::Single => edge_count == 1,
            EdgeCountSupport::Multiple => true,
        }
}

#[derive(Debug, Default)]
struct MeasurementState {
    active: bool,
    entities: Vec<MeasurementEntity>,
}

#[derive(Debug, Clone)]
struct BooleanDialogState {
    operation: BooleanOperation,
    left: Option<FeatureId>,
    right: Option<FeatureId>,
    diagnostic: Option<BooleanDiagnostic>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct LoftDialogState {
    sketch_ids: Vec<FeatureId>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct EdgeModifierDialogState {
    kind: EdgeModifierKind,
    edges: Vec<EdgeRef>,
    size: f64,
    diagnostic: Option<EdgeModifierDiagnostic>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
struct SketchDimensionEditor {
    feature_id: FeatureId,
    constraint_index: u32,
    kind: SketchDimensionKind,
    value: f64,
    position: egui::Pos2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EdgeModifierKind {
    Chamfer,
    Fillet,
}

#[derive(Debug, Clone, Copy)]
struct MaterialPreset {
    name: &'static str,
    translation_key: &'static str,
    density_kg_m3: f64,
}

const MATERIAL_PRESETS: [MaterialPreset; 7] = [
    MaterialPreset {
        name: "Aluminum",
        translation_key: "material.aluminum",
        density_kg_m3: 2_700.0,
    },
    MaterialPreset {
        name: "Steel",
        translation_key: "material.steel",
        density_kg_m3: 7_850.0,
    },
    MaterialPreset {
        name: "Stainless steel",
        translation_key: "material.stainless_steel",
        density_kg_m3: 8_000.0,
    },
    MaterialPreset {
        name: "Brass",
        translation_key: "material.brass",
        density_kg_m3: 8_500.0,
    },
    MaterialPreset {
        name: "Titanium",
        translation_key: "material.titanium",
        density_kg_m3: 4_500.0,
    },
    MaterialPreset {
        name: "ABS",
        translation_key: "material.abs",
        density_kg_m3: 1_040.0,
    },
    MaterialPreset {
        name: "PLA",
        translation_key: "material.pla",
        density_kg_m3: 1_240.0,
    },
];

impl CadxApp {
    /// Creates the desktop application and registers its wgpu callback resources.
    ///
    /// # Panics
    ///
    /// Panics when the process cannot create the Tokio runtime used for AI requests.
    #[must_use]
    pub fn new(context: &eframe::CreationContext<'_>) -> Self {
        let (config_store, settings) = match ConfigStore::discover() {
            Ok(store) => {
                let settings = store.load();
                (Some(store), settings)
            }
            Err(error) => {
                let mut settings = Settings::default();
                settings.warnings.push(error.to_string());
                (None, settings)
            }
        };
        let language = settings
            .preferences
            .language
            .as_deref()
            .map_or(Language::English, Language::from_locale);
        appearance::configure(&context.egui_ctx, settings.preferences.cjk_font.as_deref());
        if let Some(render_state) = &context.wgpu_render_state {
            render::register_renderer(render_state);
        }

        let truck_kernel = Arc::new(TruckKernel::default());
        let kernel: Arc<dyn CadKernel> = truck_kernel.clone();
        let exchange_kernel: Arc<dyn ExchangeKernel> = truck_kernel;
        let document = CadDocument::demo();
        let session = CoreBus::new(kernel, document)
            .expect("built-in demo document must be accepted by the CAD kernel");
        let viewport_scene = ViewportScene::default();
        let selected = session.document().features.last().map(|feature| feature.id);
        viewport_scene.update_with_face(session.scene(), selected, None);
        let (assistant, mut startup_warnings): (Arc<dyn AiAssistant>, Vec<String>) =
            match GenAiAssistant::from_provider_config(&settings.config.provider) {
                Ok(assistant) => (Arc::new(assistant), settings.warnings.clone()),
                Err(error) => {
                    let mut warnings = settings.warnings.clone();
                    warnings.push(error.to_string());
                    (
                        Arc::new(
                            GenAiAssistant::from_provider_config(&ProviderConfig::default())
                                .expect("built-in AI configuration must be valid"),
                        ),
                        warnings,
                    )
                }
            };
        let startup_status = startup_warnings
            .pop()
            .map_or_else(|| StatusMessage::Key("status.ready"), StatusMessage::Text);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("cadx-ai")
            .build()
            .expect("failed to create AI runtime");
        let (ai_sender, ai_receiver) = mpsc::channel();

        let mut domain_bus = DomainRegistry::new();
        domain_bus.register(Arc::new(McadPack));
        domain_bus.register(Arc::new(AecPack));
        domain_bus.register(Arc::new(EcadPack));
        let mut ai_tools = ToolRegistry::default();
        for pack in domain_bus.enabled_packs() {
            ai_tools.register_pack(pack.as_ref());
        }

        Self {
            session,
            exchange_kernel,
            viewport_scene,
            camera: OrbitCamera::default(),
            selected,
            selected_face: None,
            selected_edges: Vec::new(),
            selected_vertex: None,
            selection_mode: SelectionMode::Face,
            measurement: MeasurementState::default(),
            status: startup_status,
            assistant,
            runtime,
            ai_sender,
            ai_receiver,
            ai_input: String::new(),
            ai_pending: false,
            pending_ai_plan: None,
            conversation: vec![ConversationEntry {
                speaker: Speaker::Assistant,
                content: LocalizedText::Key("ai.welcome"),
            }],
            translator: Translator::new(language),
            toolbar_tab: ToolbarTab::Create,
            model_panel_open: true,
            ai_panel_open: true,
            document_path: None,
            loft_dialog: None,
            boolean_dialog: None,
            edge_modifier_dialog: None,
            interference_dialog: None,
            last_boolean_failure: None,
            last_edge_modifier_failure: None,
            last_sketch_failure: None,
            last_sketch_failure_feature: None,
            sketch_dimension_editor: None,
            preferences: settings.preferences,
            config_store,
            active_domain: DomainId::Mcad,
            domain_bus,
            ai_tools,
            context_collector: ContextCollector::default(),
            domain_tool_form: None,
            domain_form_values: BTreeMap::new(),
            pcb_board: PcbBoard::demo(),
            domain_report: None,
            last_intent_diff: None,
        }
    }

    fn header(&mut self, root: &mut egui::Ui) {
        let translator = self.translator.clone();
        egui::Panel::top("header")
            // The two tool rows (36 + 42) plus the separator, item spacing,
            // and frame need a little over 100 points. Keep the panel large
            // enough that the second row remains fully interactive.
            .exact_size(112.0)
            .frame(
                egui::Frame::new()
                    .fill(appearance::SURFACE)
                    .inner_margin(egui::Margin::ZERO)
                    .stroke(egui::Stroke::new(1.0, appearance::BORDER_SOFT)),
            )
            .show(root, |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 36.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add_space(12.0);
                        ui.label(appearance::icon("boxes", 17.0).color(appearance::ACCENT));
                        ui.label(
                            egui::RichText::new("CADX")
                                .size(15.0)
                                .strong()
                                .color(appearance::TEXT),
                        );
                        ui.separator();
                        let manifests = self.domain_bus.registered_manifests();
                        let active_manifest = manifests
                            .iter()
                            .find(|manifest| manifest.id == self.active_domain)
                            .cloned();
                        let mut requested_domain = None;
                        let domain_label =
                            active_manifest.map_or("DOMAIN", |manifest| manifest.name);
                        let domain_menu = ui.menu_button(
                            (
                                appearance::icon(domain_icon(self.active_domain), 13.0),
                                egui::RichText::new(domain_label).size(11.0),
                            ),
                            |ui| {
                                ui.label(
                                    egui::RichText::new(translator.text("domain.switch"))
                                        .size(10.0)
                                        .color(appearance::TEXT_MUTED),
                                );
                                for manifest in &manifests {
                                    let mut enabled = self.domain_bus.is_enabled(manifest.id);
                                    if ui.checkbox(&mut enabled, manifest.name).changed() {
                                        self.domain_bus.set_enabled(manifest.id, enabled);
                                        if !enabled && self.active_domain == manifest.id {
                                            requested_domain = manifests
                                                .iter()
                                                .find(|candidate| {
                                                    candidate.id != manifest.id
                                                        && self.domain_bus.is_enabled(candidate.id)
                                                })
                                                .map(|candidate| candidate.id);
                                        }
                                    }
                                    if enabled
                                        && ui
                                            .selectable_label(
                                                self.active_domain == manifest.id,
                                                format!("{}  v{}", manifest.name, manifest.version),
                                            )
                                            .clicked()
                                    {
                                        requested_domain = Some(manifest.id);
                                        ui.close();
                                    }
                                }
                            },
                        );
                        domain_menu
                            .response
                            .on_hover_text(translator.text("domain.switch"));
                        if let Some(domain) = requested_domain {
                            self.active_domain = domain;
                            self.domain_tool_form = None;
                            self.domain_report = None;
                            self.status = StatusMessage::Text(
                                self.active_domain.slug().replace('-', " ").to_uppercase(),
                            );
                        }
                        if icon_button(ui, "file-plus", translator.text("tool.new"), true, false)
                            .clicked()
                        {
                            self.new_document();
                        }
                        if icon_button(ui, "folder-open", translator.text("tool.open"), true, false)
                            .clicked()
                        {
                            self.open_document();
                        }
                        if icon_button(
                            ui,
                            "file-input",
                            translator.text("tool.import_step"),
                            true,
                            false,
                        )
                        .clicked()
                        {
                            self.import_step();
                        }
                        if icon_button(ui, "save", translator.text("tool.save"), true, false)
                            .clicked()
                        {
                            self.save_document(false);
                        }
                        let mut export_step = false;
                        let mut export_stl = false;
                        let mut export_3mf = false;
                        ui.add_enabled_ui(!self.session.scene().parts.is_empty(), |ui| {
                            let menu = ui.menu_button(appearance::icon("download", 14.0), |ui| {
                                if ui.button(translator.text("tool.export_step")).clicked() {
                                    export_step = true;
                                    ui.close();
                                }
                                if ui.button(translator.text("tool.export_stl")).clicked() {
                                    export_stl = true;
                                    ui.close();
                                }
                                if ui.button(translator.text("tool.export_3mf")).clicked() {
                                    export_3mf = true;
                                    ui.close();
                                }
                            });
                            menu.response.on_hover_text(translator.text("tool.export"));
                        });
                        if export_step {
                            self.export_step();
                        } else if export_stl {
                            self.export_stl();
                        } else if export_3mf {
                            self.export_3mf();
                        }
                        ui.separator();
                        let document_name = self
                            .document_path
                            .as_ref()
                            .and_then(|path| path.file_name())
                            .and_then(|name| name.to_str())
                            .map_or_else(
                                || {
                                    if self.session.document().name == "Untitled" {
                                        translator.text("app.untitled").to_owned()
                                    } else {
                                        self.session.document().name.clone()
                                    }
                                },
                                str::to_owned,
                            );
                        let document_name = if self.session.state() == DocumentState::Dirty {
                            format!("{document_name} *")
                        } else {
                            document_name
                        };
                        ui.label(
                            egui::RichText::new(document_name)
                                .size(12.0)
                                .color(appearance::TEXT_MUTED),
                        );

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.add_space(8.0);
                            let mut requested_language = None;
                            let menu = ui.menu_button(
                                egui::RichText::new(self.translator.language().short_name())
                                    .size(11.0),
                                |ui| {
                                    for language in Language::ALL {
                                        let selected = language == self.translator.language();
                                        if ui
                                            .selectable_label(selected, language.native_name())
                                            .clicked()
                                        {
                                            requested_language = Some(language);
                                            ui.close();
                                        }
                                    }
                                },
                            );
                            menu.response
                                .on_hover_text(translator.text("language.label"));
                            if let Some(language) = requested_language {
                                self.translator.set_language(language);
                                self.preferences.language = Some(language.code().into());
                                match &self.config_store {
                                    Some(store) => {
                                        if let Err(error) =
                                            store.save_preferences(&self.preferences)
                                        {
                                            self.status = StatusMessage::Text(error.to_string());
                                        }
                                    }
                                    None => {
                                        self.status = StatusMessage::Text(
                                            "home directory is unavailable".into(),
                                        );
                                    }
                                }
                            }

                            if icon_button(
                                ui,
                                "panel-right",
                                translator.text("tool.toggle_ai"),
                                true,
                                self.ai_panel_open,
                            )
                            .clicked()
                            {
                                self.ai_panel_open = !self.ai_panel_open;
                            }
                            if icon_button(
                                ui,
                                "panel-left",
                                translator.text("tool.toggle_model"),
                                true,
                                self.model_panel_open,
                            )
                            .clicked()
                            {
                                self.model_panel_open = !self.model_panel_open;
                            }

                            ui.separator();
                            ui.label(
                                egui::RichText::new(self.session.kernel_name())
                                    .monospace()
                                    .size(10.0)
                                    .color(appearance::TEXT_MUTED),
                            );
                            ui.label(appearance::icon("activity", 12.0).color(appearance::ACCENT));
                        });
                    },
                );

                ui.separator();
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 42.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        if self.active_domain == DomainId::Aec {
                            self.aec_toolbar(ui, &translator);
                        } else {
                            ui.add_space(8.0);
                            if tab_button(
                                ui,
                                "box",
                                translator.text("toolbar.create"),
                                self.toolbar_tab == ToolbarTab::Create,
                            )
                            .clicked()
                            {
                                self.toolbar_tab = ToolbarTab::Create;
                            }
                            if tab_button(
                                ui,
                                "orbit",
                                translator.text("toolbar.view"),
                                self.toolbar_tab == ToolbarTab::View,
                            )
                            .clicked()
                            {
                                self.toolbar_tab = ToolbarTab::View;
                            }
                            if tab_button(
                                ui,
                                "combine",
                                translator.text("toolbar.design"),
                                self.toolbar_tab == ToolbarTab::Design,
                            )
                            .clicked()
                            {
                                self.toolbar_tab = ToolbarTab::Design;
                            }
                            ui.separator();

                            if self.active_domain == DomainId::Ecad {
                                self.pcb_toolbar(ui, &translator);
                            } else {
                                match self.toolbar_tab {
                                    ToolbarTab::Create => {
                                        let selected_face = planar_face_selection(
                                            self.session.scene(),
                                            self.selected_face.as_ref(),
                                        );
                                        let selected_datum = self.selected.filter(|id| {
                                            self.session.document().feature(*id).is_some_and(
                                                |feature| {
                                                    matches!(
                                                        feature.primitive,
                                                        Primitive::DatumPlane { .. }
                                                    )
                                                },
                                            )
                                        });
                                        let sketch_tool = if selected_datum.is_some() {
                                            translator.text("tool.sketch_on_datum")
                                        } else if selected_face.is_some() {
                                            translator.text("tool.sketch_on_face")
                                        } else {
                                            translator.text("tool.sketch")
                                        };
                                        if tool_button(ui, "pencil", sketch_tool, sketch_tool, true)
                                            .clicked()
                                        {
                                            let _ = self.execute(
                                                vec![ModelCommand::CreateSketch {
                                                    plane: selected_datum.map_or_else(
                                                        || {
                                                            selected_face.clone().map_or(
                                                                SketchPlane::WorldXy,
                                                                |face| SketchPlane::PlanarFace {
                                                                    face,
                                                                },
                                                            )
                                                        },
                                                        |datum_id| SketchPlane::DatumPlane {
                                                            datum_id,
                                                        },
                                                    ),
                                                    name: translator
                                                        .text("primitive.sketch")
                                                        .into(),
                                                    profile: vec![
                                                        [-15.0, -10.0],
                                                        [15.0, -10.0],
                                                        [15.0, 10.0],
                                                        [-15.0, 10.0],
                                                    ],
                                                    holes: Vec::new(),
                                                    constraints: Vec::new(),
                                                    position: [0.0; 3],
                                                }],
                                                StatusMessage::Key("status.created_sketch"),
                                            );
                                        }
                                        if tool_button(
                                            ui,
                                            "circle-dot",
                                            translator.text("tool.circle_sketch"),
                                            translator.text("tool.circle_sketch"),
                                            true,
                                        )
                                        .clicked()
                                        {
                                            let _ = self.execute(
                                                vec![ModelCommand::CreateSketchRegion {
                                                    plane: selected_datum.map_or_else(
                                                        || {
                                                            selected_face.clone().map_or(
                                                                SketchPlane::WorldXy,
                                                                |face| SketchPlane::PlanarFace {
                                                                    face,
                                                                },
                                                            )
                                                        },
                                                        |datum_id| SketchPlane::DatumPlane {
                                                            datum_id,
                                                        },
                                                    ),
                                                    name: translator
                                                        .text("primitive.circle_sketch")
                                                        .into(),
                                                    region: SketchRegion2D {
                                                        profile: exact_circle_loop(
                                                            [0.0, 0.0],
                                                            12.0,
                                                        ),
                                                        holes: Vec::new(),
                                                    },
                                                    construction: Vec::new(),
                                                    constraints: Vec::new(),
                                                    position: [0.0; 3],
                                                }],
                                                StatusMessage::Key("status.created_sketch"),
                                            );
                                        }
                                        let selected_sketch = self.selected.filter(|id| {
                                            self.session.document().feature(*id).is_some_and(
                                                |feature| {
                                                    matches!(
                                                        feature.primitive,
                                                        Primitive::Sketch { .. }
                                                    )
                                                },
                                            )
                                        });
                                        if let Some(sketch_id) = selected_sketch
                                            && tool_button(
                                                ui,
                                                "layers",
                                                translator.text("tool.extrude_sketch"),
                                                translator.text("tool.extrude_sketch"),
                                                true,
                                            )
                                            .clicked()
                                        {
                                            let _ = self.execute(
                                                vec![ModelCommand::CreateExtrusionFromSketch {
                                                    name: translator
                                                        .text("primitive.extrusion")
                                                        .into(),
                                                    sketch_id,
                                                    height: 20.0,
                                                    position: [0.0; 3],
                                                }],
                                                StatusMessage::Key("status.created_extrusion"),
                                            );
                                        }
                                        if let Some(sketch_id) = selected_sketch
                                            && tool_button(
                                                ui,
                                                "rotate-cw",
                                                translator.text("tool.revolve_sketch"),
                                                if self
                                                    .session
                                                    .document()
                                                    .feature(sketch_id)
                                                    .is_some_and(|feature| {
                                                        matches!(
                                                            feature.primitive,
                                                            Primitive::Sketch { ref region, .. }
                                                                if !region.holes.is_empty()
                                                        )
                                                    })
                                                {
                                                    translator
                                                        .text("tool.revolve_holes_unsupported")
                                                } else {
                                                    translator.text("tool.revolve_sketch")
                                                },
                                                self.session
                                                    .document()
                                                    .feature(sketch_id)
                                                    .is_some_and(|feature| {
                                                        matches!(
                                                            feature.primitive,
                                                            Primitive::Sketch { ref region, .. }
                                                                if region.holes.is_empty()
                                                        )
                                                    }),
                                            )
                                            .clicked()
                                        {
                                            let axis_x = self
                                                .session
                                                .document()
                                                .feature(sketch_id)
                                                .map_or(5.0, |feature| match &feature.primitive {
                                                    Primitive::Sketch { region, .. } => {
                                                        region
                                                            .profile
                                                            .sampled_points(
                                                                std::f64::consts::PI / 36.0,
                                                            )
                                                            .into_iter()
                                                            .map(|point| point[0])
                                                            .fold(f64::NEG_INFINITY, f64::max)
                                                            + 5.0
                                                    }
                                                    _ => 5.0,
                                                });
                                            let _ = self.execute(
                                                vec![ModelCommand::CreateRevolveFromSketch {
                                                    name: translator
                                                        .text("primitive.revolve")
                                                        .into(),
                                                    sketch_id,
                                                    axis_origin: [axis_x, 0.0],
                                                    axis_direction: [0.0, 1.0],
                                                    angle: 360.0,
                                                    position: [0.0; 3],
                                                }],
                                                StatusMessage::Key("status.created_revolve"),
                                            );
                                        }
                                        let loft_sketch_ids = self
                                            .session
                                            .document()
                                            .features
                                            .iter()
                                            .filter_map(|feature| match &feature.primitive {
                                                Primitive::Sketch { region, .. }
                                                    if region.holes.is_empty() =>
                                                {
                                                    Some(feature.id)
                                                }
                                                _ => None,
                                            })
                                            .collect::<Vec<_>>();
                                        if tool_button(
                                            ui,
                                            "layers",
                                            translator.text("tool.loft"),
                                            translator.text("tool.loft"),
                                            loft_sketch_ids.len() >= 2,
                                        )
                                        .clicked()
                                        {
                                            self.loft_dialog = Some(LoftDialogState {
                                                sketch_ids: loft_sketch_ids
                                                    .into_iter()
                                                    .take(MAX_LOFT_SECTIONS)
                                                    .collect(),
                                                error: None,
                                            });
                                            self.boolean_dialog = None;
                                            self.edge_modifier_dialog = None;
                                        }
                                        if tool_button(
                                            ui,
                                            "box",
                                            translator.text("tool.box"),
                                            translator.text("tool.box"),
                                            true,
                                        )
                                        .clicked()
                                        {
                                            let _ = self.execute(
                                                vec![ModelCommand::CreateBox {
                                                    name: translator.text("primitive.box").into(),
                                                    size: [20.0, 20.0, 20.0],
                                                    position: [-10.0, -10.0, 0.0],
                                                }],
                                                StatusMessage::Key("status.created_box"),
                                            );
                                        }
                                        if tool_button(
                                            ui,
                                            "cylinder",
                                            translator.text("tool.cylinder"),
                                            translator.text("tool.cylinder"),
                                            true,
                                        )
                                        .clicked()
                                        {
                                            let _ = self.execute(
                                                vec![ModelCommand::CreateCylinder {
                                                    name: translator
                                                        .text("primitive.cylinder")
                                                        .into(),
                                                    radius: 10.0,
                                                    height: 24.0,
                                                    position: [0.0, 0.0, 0.0],
                                                }],
                                                StatusMessage::Key("status.created_cylinder"),
                                            );
                                        }
                                        if tool_button(
                                            ui,
                                            "circle",
                                            translator.text("tool.sphere"),
                                            translator.text("tool.sphere"),
                                            true,
                                        )
                                        .clicked()
                                        {
                                            let _ = self.execute(
                                                vec![ModelCommand::CreateSphere {
                                                    name: translator
                                                        .text("primitive.sphere")
                                                        .into(),
                                                    radius: 10.0,
                                                    position: [0.0, 0.0, 10.0],
                                                }],
                                                StatusMessage::Key("status.created_sphere"),
                                            );
                                        }
                                        if tool_button(
                                            ui,
                                            "triangle",
                                            translator.text("tool.cone"),
                                            translator.text("tool.cone"),
                                            true,
                                        )
                                        .clicked()
                                        {
                                            let _ = self.execute(
                                                vec![ModelCommand::CreateCone {
                                                    name: translator.text("primitive.cone").into(),
                                                    bottom_radius: 10.0,
                                                    top_radius: 4.0,
                                                    height: 24.0,
                                                    position: [0.0; 3],
                                                }],
                                                StatusMessage::Key("status.created_cone"),
                                            );
                                        }
                                        if tool_button(
                                            ui,
                                            "circle-dot",
                                            translator.text("tool.torus"),
                                            translator.text("tool.torus"),
                                            true,
                                        )
                                        .clicked()
                                        {
                                            let _ = self.execute(
                                                vec![ModelCommand::CreateTorus {
                                                    name: translator.text("primitive.torus").into(),
                                                    major_radius: 14.0,
                                                    minor_radius: 4.0,
                                                    position: [0.0, 0.0, 12.0],
                                                }],
                                                StatusMessage::Key("status.created_torus"),
                                            );
                                        }
                                        if tool_button(
                                            ui,
                                            "layers",
                                            translator.text("tool.extrusion"),
                                            translator.text("tool.extrusion"),
                                            true,
                                        )
                                        .clicked()
                                        {
                                            let _ = self.execute(
                                                vec![ModelCommand::CreateExtrusion {
                                                    name: translator
                                                        .text("primitive.extrusion")
                                                        .into(),
                                                    profile: vec![
                                                        [-10.0, -10.0],
                                                        [10.0, -10.0],
                                                        [10.0, 10.0],
                                                        [-10.0, 10.0],
                                                    ],
                                                    height: 20.0,
                                                    position: [0.0; 3],
                                                }],
                                                StatusMessage::Key("status.created_extrusion"),
                                            );
                                        }
                                    }
                                    ToolbarTab::Design => {
                                        let capabilities = self.session.kernel_capabilities();
                                        let edge_count = self.selected_edges.len();
                                        if tool_button(
                                            ui,
                                            "combine",
                                            translator.text("tool.boolean"),
                                            translator.text("tool.boolean"),
                                            self.session
                                                .document()
                                                .features
                                                .iter()
                                                .filter(|feature| {
                                                    !feature.primitive.is_reference_geometry()
                                                })
                                                .count()
                                                >= 2,
                                        )
                                        .clicked()
                                        {
                                            let left = self.selected.filter(|id| {
                                                self.session.document().feature(*id).is_some_and(
                                                    |feature| {
                                                        !feature.primitive.is_reference_geometry()
                                                    },
                                                )
                                            });
                                            let right =
                                                self.session.document().features.iter().find_map(
                                                    |feature| {
                                                        (Some(feature.id) != left
                                                            && !feature
                                                                .primitive
                                                                .is_reference_geometry())
                                                        .then_some(feature.id)
                                                    },
                                                );
                                            self.boolean_dialog = Some(BooleanDialogState {
                                                operation: BooleanOperation::Union,
                                                left,
                                                right,
                                                diagnostic: None,
                                                error: None,
                                            });
                                            self.loft_dialog = None;
                                            self.edge_modifier_dialog = None;
                                        }
                                        if tool_button(
                                            ui,
                                            "octagon",
                                            translator.text("tool.chamfer"),
                                            translator.text("tool.chamfer"),
                                            edge_modifier_tool_enabled(
                                                capabilities.chamfer,
                                                edge_count,
                                            ),
                                        )
                                        .clicked()
                                            && let Some(edge) = self.selected_edges.last()
                                        {
                                            let size = self
                                                .session
                                                .scene()
                                                .edge(edge)
                                                .map_or(1.0, |edge| {
                                                    (edge.geometry.length * 0.1).clamp(0.1, 5.0)
                                                });
                                            self.edge_modifier_dialog =
                                                Some(EdgeModifierDialogState {
                                                    kind: EdgeModifierKind::Chamfer,
                                                    edges: self.selected_edges.clone(),
                                                    size,
                                                    diagnostic: None,
                                                    error: None,
                                                });
                                            self.loft_dialog = None;
                                            self.boolean_dialog = None;
                                        }
                                        if tool_button(
                                            ui,
                                            "radius",
                                            translator.text("tool.fillet"),
                                            translator.text("tool.fillet"),
                                            edge_modifier_tool_enabled(
                                                capabilities.fillet,
                                                edge_count,
                                            ),
                                        )
                                        .clicked()
                                            && let Some(edge) = self.selected_edges.last()
                                        {
                                            let size = self
                                                .session
                                                .scene()
                                                .edge(edge)
                                                .map_or(1.0, |edge| {
                                                    (edge.geometry.length * 0.1).clamp(0.1, 5.0)
                                                });
                                            self.edge_modifier_dialog =
                                                Some(EdgeModifierDialogState {
                                                    kind: EdgeModifierKind::Fillet,
                                                    edges: self.selected_edges.clone(),
                                                    size,
                                                    diagnostic: None,
                                                    error: None,
                                                });
                                            self.loft_dialog = None;
                                            self.boolean_dialog = None;
                                        }
                                        ui.separator();
                                        if tool_button(
                                            ui,
                                            "scan-search",
                                            translator.text("tool.interference"),
                                            translator.text("tool.interference"),
                                            capabilities.interference_analysis,
                                        )
                                        .clicked()
                                        {
                                            self.run_interference_analysis();
                                        }
                                    }
                                    ToolbarTab::View => {
                                        if tool_button(
                                            ui,
                                            "focus",
                                            translator.text("tool.frame"),
                                            translator.text("tool.frame"),
                                            true,
                                        )
                                        .clicked()
                                        {
                                            self.camera.frame_scene(self.session.scene());
                                        }
                                        ui.separator();
                                        for (mode, icon, tooltip) in [
                                            (
                                                SelectionMode::Face,
                                                "panel-top",
                                                translator.text("tool.select_face"),
                                            ),
                                            (
                                                SelectionMode::Edge,
                                                "minus",
                                                translator.text("tool.select_edge"),
                                            ),
                                            (
                                                SelectionMode::Vertex,
                                                "circle-dot",
                                                translator.text("tool.select_vertex"),
                                            ),
                                        ] {
                                            if icon_button(
                                                ui,
                                                icon,
                                                tooltip,
                                                true,
                                                self.selection_mode == mode,
                                            )
                                            .clicked()
                                            {
                                                self.selection_mode = mode;
                                                self.clear_topology_selection();
                                                self.measurement.entities.clear();
                                                self.sync_viewport();
                                            }
                                        }
                                        ui.separator();
                                        if icon_button(
                                            ui,
                                            "ruler",
                                            translator.text("tool.measure"),
                                            true,
                                            self.measurement.active,
                                        )
                                        .clicked()
                                        {
                                            self.toggle_measurement();
                                        }
                                    }
                                }
                            }
                        }

                        ui.separator();
                        if icon_button(
                            ui,
                            "undo-2",
                            translator.text("tool.undo"),
                            self.session.can_undo(),
                            false,
                        )
                        .clicked()
                        {
                            self.undo();
                        }
                        if icon_button(
                            ui,
                            "redo-2",
                            translator.text("tool.redo"),
                            self.session.can_redo(),
                            false,
                        )
                        .clicked()
                        {
                            self.redo();
                        }
                    },
                );
            });
    }

    fn aec_toolbar(&mut self, ui: &mut egui::Ui, translator: &Translator) {
        const TOOLS: [&str; 10] = [
            "wall",
            "slab",
            "opening",
            "levels",
            "space",
            "bim-attrs",
            "schedule",
            "quantity-takeoff",
            "clash",
            "ifc",
        ];

        egui::ScrollArea::horizontal()
            .id_salt("aec_toolbar")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    for tool_id in TOOLS {
                        let label = domain_tool_label(tool_id, DomainId::Aec, translator);
                        if tool_button(ui, aec_tool_icon(tool_id), label, label, true).clicked() {
                            self.open_domain_tool(tool_id);
                        }
                    }
                });
            });
    }

    fn pcb_toolbar(&mut self, ui: &mut egui::Ui, translator: &Translator) {
        if tool_button(
            ui,
            "box",
            translator.text("pcb.tool.board"),
            translator.text("pcb.tool.board"),
            true,
        )
        .clicked()
        {
            self.create_pcb_board();
        }
        if tool_button(
            ui,
            "circle-dot",
            translator.text("pcb.tool.component"),
            translator.text("pcb.tool.component"),
            self.pcb_board.validate().is_ok(),
        )
        .clicked()
        {
            self.place_pcb_component();
        }
        if tool_button(
            ui,
            "combine",
            translator.text("pcb.tool.routing"),
            translator.text("pcb.tool.routing"),
            true,
        )
        .clicked()
        {
            self.status = StatusMessage::Key("pcb.status.routing_ready");
        }
        if tool_button(
            ui,
            "triangle-alert",
            translator.text("pcb.tool.drc"),
            translator.text("pcb.tool.drc"),
            true,
        )
        .clicked()
        {
            self.domain_report = Some(DomainReport::PcbDrc(pcb_drc::run(&self.pcb_board)));
        }
        if tool_button(
            ui,
            "layers",
            translator.text("pcb.tool.stackup"),
            translator.text("pcb.tool.stackup"),
            true,
        )
        .clicked()
        {
            self.status = StatusMessage::Text(format!(
                "{}: {}",
                translator.text("pcb.status.stackup"),
                self.pcb_board.layers.len()
            ));
        }
        if tool_button(
            ui,
            "layers",
            translator.text("pcb.tool.bom"),
            translator.text("pcb.tool.bom"),
            true,
        )
        .clicked()
        {
            self.domain_report = Some(DomainReport::PcbBom(self.pcb_board.bom()));
        }
        if tool_button(
            ui,
            "file-input",
            translator.text("pcb.tool.gerber"),
            translator.text("pcb.tool.gerber"),
            true,
        )
        .clicked()
        {
            self.domain_report = Some(DomainReport::ExportPreview(pcb_export::gerber_bundle(
                &self.pcb_board,
            )));
        }
    }

    fn create_pcb_board(&mut self) {
        self.create_pcb_board_from(TransactionSource::Ui);
    }

    fn create_pcb_board_from(&mut self, source: TransactionSource) {
        let board = PcbBoard::demo();
        let _ = self.execute_from(
            vec![ModelCommand::CreateBox {
                name: board.name.clone(),
                size: [board.width_mm, board.height_mm, board.thickness_mm],
                position: [-board.width_mm * 0.5, -board.height_mm * 0.5, 0.0],
            }],
            StatusMessage::Key("pcb.status.board_created"),
            source,
        );
        self.pcb_board = board;
    }

    fn place_pcb_component(&mut self) {
        self.place_pcb_component_from(TransactionSource::Ui);
    }

    fn place_pcb_component_from(&mut self, source: TransactionSource) {
        let index = u32::try_from(self.pcb_board.components.len())
            .unwrap_or(u32::MAX)
            .saturating_add(1);
        let reference = format!("U{index}");
        let position = [
            14.0 + (f64::from(index) * 13.0) % (self.pcb_board.width_mm - 28.0),
            14.0 + (f64::from(index) * 9.0) % (self.pcb_board.height_mm - 28.0),
        ];
        let size = [8.0, 8.0];
        let _ = self.execute_from(
            vec![ModelCommand::CreateBox {
                name: format!("{reference} MCU"),
                size: [size[0], size[1], 1.0],
                position: [
                    -self.pcb_board.width_mm * 0.5 + position[0] - size[0] * 0.5,
                    -self.pcb_board.height_mm * 0.5 + position[1] - size[1] * 0.5,
                    self.pcb_board.thickness_mm,
                ],
            }],
            StatusMessage::Key("pcb.status.component_placed"),
            source,
        );
        let linked_feature_id = self.selected;
        self.pcb_board
            .components
            .push(cadx_ecad::layout::PcbComponent {
                reference,
                value: "MCU".into(),
                footprint: "QFN-32".into(),
                position_mm: position,
                size_mm: size,
                height_mm: 1.0,
                rotation_deg: 0.0,
                side: cadx_ecad::layout::ComponentSide::Top,
                model_3d: Some("QFN-32.step".into()),
                linked_feature_id,
            });
    }

    fn domain_context(&self) -> DomainContext {
        let spatial_entities = self
            .session
            .scene()
            .parts
            .iter()
            .filter_map(|part| {
                let (minimum_mm, maximum_mm) = part.mesh.positions.iter().fold(
                    ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]),
                    |(minimum, maximum), position| {
                        let position = position.map(f64::from);
                        (
                            std::array::from_fn(|axis| minimum[axis].min(position[axis])),
                            std::array::from_fn(|axis| maximum[axis].max(position[axis])),
                        )
                    },
                );
                minimum_mm
                    .iter()
                    .chain(maximum_mm.iter())
                    .all(|value| value.is_finite())
                    .then(|| cadx_domain_api::DomainSpatialEntity {
                        feature_id: part.feature_id,
                        name: part.name.clone(),
                        minimum_mm,
                        maximum_mm,
                    })
            })
            .collect();
        DomainContext {
            document_name: self.session.document().name.clone(),
            selected_feature_ids: self.selected.into_iter().collect(),
            visible_solid_count: self.session.scene().parts.len(),
            active_feature_count: self.session.document().features.len(),
            selected_feature_name: self.selected.and_then(|id| {
                self.session
                    .document()
                    .feature(id)
                    .map(|feature| feature.name.clone())
            }),
            spatial_entities,
        }
    }

    fn sync_domain_state(&mut self) {
        let Some(value) = self
            .session
            .document()
            .domain_data
            .get("ecad.layout")
            .and_then(|values| values.get("board"))
        else {
            self.pcb_board = PcbBoard::demo();
            return;
        };
        match serde_json::from_value::<PcbBoard>(value.clone()) {
            Ok(board) if board.validate().is_ok() => self.pcb_board = board,
            Ok(_) => {
                self.status = StatusMessage::Text("Stored ECAD board data is invalid".into());
            }
            Err(error) => self.status = StatusMessage::Text(error.to_string()),
        }
    }

    fn route_domain_prompt(&mut self, prompt: &str) -> Option<DomainRoute> {
        let normalized = prompt.to_ascii_lowercase();
        let pcb_keyword = normalized.contains("pcb")
            || normalized.contains("ecad")
            || normalized.contains("gerber")
            || normalized.contains("drc")
            || normalized.contains("board")
            || normalized.contains("netlist")
            || normalized.contains("routing")
            || normalized.contains("footprint")
            || prompt.contains("电路")
            || prompt.contains("网表")
            || prompt.contains("布线")
            || prompt.contains("封装");
        let aec_keyword = normalized.contains("aec")
            || normalized.contains("bim")
            || normalized.contains("ifc")
            || normalized.contains("wall")
            || normalized.contains("slab")
            || normalized.contains("building")
            || prompt.contains("建筑")
            || prompt.contains("墙")
            || prompt.contains("楼板");
        let mechanical_keyword = normalized.contains("dfm")
            || normalized.contains("mcad")
            || normalized.contains("feature tree")
            || normalized.contains("extrude")
            || normalized.contains("fillet")
            || normalized.contains("chamfer")
            || normalized.contains("bom")
            || prompt.contains("制造")
            || prompt.contains("工程图")
            || prompt.contains("特征树")
            || prompt.contains("拉伸")
            || prompt.contains("倒角")
            || prompt.contains("圆角")
            || prompt.contains("物料");
        let domain_keyword = pcb_keyword || aec_keyword || mechanical_keyword;
        if !domain_keyword {
            return None;
        }
        let routed_domain = if pcb_keyword {
            DomainId::Ecad
        } else if aec_keyword {
            DomainId::Aec
        } else {
            DomainId::Mcad
        };
        self.active_domain = routed_domain;
        let domain_context = self.domain_context();
        let pack = self
            .domain_bus
            .enabled_packs()
            .into_iter()
            .find(|pack| pack.manifest().id == routed_domain)?;
        let mut context_collector = ContextCollector::new(ContextSnapshot {
            domain: Some(routed_domain),
            document_name: domain_context.document_name.clone(),
            selected_feature_ids: domain_context.selected_feature_ids.clone(),
            visible_solid_count: domain_context.visible_solid_count,
            active_feature_count: domain_context.active_feature_count,
            ..ContextSnapshot::default()
        });
        if let Ok(serde_json::Value::Object(schema)) = serde_json::to_value(pack.inspector_schema())
        {
            context_collector.set_domain_schema(schema);
        }
        self.context_collector = context_collector;
        Some(pack.route_natural_language(prompt, &domain_context))
    }

    fn dispatch_domain_route(&mut self, route: DomainRoute) {
        self.dispatch_domain_execution(DomainExecution::with_action(route.rationale, route.action));
    }

    fn dispatch_domain_execution(&mut self, execution: DomainExecution) {
        let DomainExecution {
            summary,
            actions,
            issues,
            artifacts,
        } = execution;
        let mut commands = Vec::new();
        let mut deferred = Vec::new();
        let mut board_update = None;
        let mut component_updates = Vec::new();

        for action in actions {
            match action {
                DomainAction::CreateSolidBox {
                    name,
                    size_mm,
                    position_mm,
                } => commands.push(ModelCommand::CreateBox {
                    name,
                    size: size_mm,
                    position: position_mm,
                }),
                DomainAction::CreateSolidCylinder {
                    name,
                    radius_mm,
                    height_mm,
                    position_mm,
                } => commands.push(ModelCommand::CreateCylinder {
                    name,
                    radius: radius_mm,
                    height: height_mm,
                    position: position_mm,
                }),
                DomainAction::CreateProfileExtrusion {
                    name,
                    profile_mm,
                    height_mm,
                    position_mm,
                } => commands.push(ModelCommand::CreateExtrusion {
                    name,
                    profile: profile_mm,
                    height: height_mm,
                    position: position_mm,
                }),
                DomainAction::CreatePcbBoard {
                    name,
                    width_mm,
                    height_mm,
                    thickness_mm,
                    layers,
                } => match PcbBoard::rectangular(
                    name.clone(),
                    width_mm,
                    height_mm,
                    thickness_mm,
                    layers,
                ) {
                    Ok(board) => {
                        commands.push(ModelCommand::CreateBox {
                            name,
                            size: [width_mm, height_mm, thickness_mm],
                            position: [-width_mm * 0.5, -height_mm * 0.5, 0.0],
                        });
                        board_update = Some(board);
                    }
                    Err(error) => {
                        self.status = StatusMessage::Text(error.to_string());
                        return;
                    }
                },
                DomainAction::PlacePcbComponent {
                    reference,
                    value,
                    footprint,
                    position_mm,
                    rotation_deg,
                    side,
                    model_3d,
                } => {
                    let descriptor = cadx_ecad::footprint_library()
                        .iter()
                        .find(|candidate| candidate.package.eq_ignore_ascii_case(&footprint))
                        .unwrap_or(&cadx_ecad::footprint_library()[0]);
                    let size = descriptor.body_size_mm;
                    commands.push(ModelCommand::CreateBox {
                        name: format!("{reference} {value}"),
                        size: [size[0], size[1], descriptor.default_height_mm],
                        position: [
                            -self.pcb_board.width_mm * 0.5 + position_mm[0] - size[0] * 0.5,
                            -self.pcb_board.height_mm * 0.5 + position_mm[1] - size[1] * 0.5,
                            self.pcb_board.thickness_mm,
                        ],
                    });
                    component_updates.push(cadx_ecad::layout::PcbComponent {
                        reference,
                        value,
                        footprint,
                        position_mm,
                        size_mm: size,
                        height_mm: descriptor.default_height_mm,
                        rotation_deg,
                        side: if side.eq_ignore_ascii_case("bottom") {
                            cadx_ecad::layout::ComponentSide::Bottom
                        } else {
                            cadx_ecad::layout::ComponentSide::Top
                        },
                        model_3d,
                        linked_feature_id: None,
                    });
                }
                DomainAction::UpsertDomainMetadata {
                    entity_key,
                    namespace,
                    values,
                } => {
                    let value = match serde_json::to_value(values) {
                        Ok(value) => value,
                        Err(error) => {
                            self.status = StatusMessage::Text(error.to_string());
                            return;
                        }
                    };
                    commands.push(ModelCommand::SetDomainData {
                        namespace,
                        entity_key,
                        value,
                    });
                }
                action => deferred.push(action),
            }
        }

        let mut persisted_board = None;
        if board_update.is_some() || !component_updates.is_empty() {
            let mut board = board_update.unwrap_or_else(|| self.pcb_board.clone());
            board.components.extend(component_updates);
            if let Err(error) = board.validate() {
                self.status = StatusMessage::Text(error.to_string());
                return;
            }
            let value = match serde_json::to_value(&board) {
                Ok(value) => value,
                Err(error) => {
                    self.status = StatusMessage::Text(error.to_string());
                    return;
                }
            };
            commands.push(ModelCommand::SetDomainData {
                namespace: "ecad.layout".into(),
                entity_key: "board".into(),
                value,
            });
            persisted_board = Some(board);
        }

        if !commands.is_empty()
            && self
                .execute_from(
                    commands,
                    StatusMessage::Text(summary.clone()),
                    TransactionSource::DomainPack,
                )
                .is_err()
        {
            return;
        }
        if let Some(board) = persisted_board {
            self.pcb_board = board;
        }
        for action in deferred {
            self.dispatch_non_geometry_domain_action(action);
        }
        if !artifacts.is_empty() {
            self.domain_report = Some(DomainReport::Artifacts(artifacts));
        }
        if !issues.is_empty() {
            let issue_summary = issues
                .iter()
                .map(|issue| format!("{}: {}", issue.code, issue.message))
                .collect::<Vec<_>>()
                .join("; ");
            self.status = StatusMessage::Text(issue_summary);
        } else if self.domain_report.is_none() {
            self.status = StatusMessage::Text(summary);
        }
    }

    fn dispatch_non_geometry_domain_action(&mut self, action: DomainAction) {
        match action {
            DomainAction::RunCheck { check } => match check.as_str() {
                "drc" => {
                    self.domain_report = Some(DomainReport::PcbDrc(pcb_drc::run(&self.pcb_board)));
                }
                "dfm" => self.run_mechanical_dfm(),
                "drawing" => self.run_mechanical_drawing_check(),
                "interference" | "clash" => self.run_interference_analysis(),
                _ => self.status = StatusMessage::Text(format!("Domain check: {check}")),
            },
            DomainAction::GenerateBom => match self.active_domain {
                DomainId::Ecad => {
                    self.domain_report = Some(DomainReport::PcbBom(self.pcb_board.bom()));
                }
                DomainId::Aec => self.run_domain_tool("schedule"),
                DomainId::Mcad => self.run_mechanical_bom(),
            },
            DomainAction::Export { format } if format == "gerber" => {
                match pcb_export::manufacturing_bundle(&self.pcb_board) {
                    Ok(files) => self.domain_report = Some(DomainReport::ExportPreview(files)),
                    Err(error) => self.status = StatusMessage::Text(error.to_string()),
                }
            }
            DomainAction::Export { format } if format == "ifc" => self.run_domain_tool("ifc"),
            DomainAction::Export { format } => {
                self.status = StatusMessage::Text(format!("Domain export: {format}"));
            }
            DomainAction::OpenPanel { panel } => self.open_domain_panel(&panel),
            DomainAction::CreateSolidBox { .. }
            | DomainAction::CreateSolidCylinder { .. }
            | DomainAction::CreateProfileExtrusion { .. }
            | DomainAction::CreatePcbBoard { .. }
            | DomainAction::PlacePcbComponent { .. }
            | DomainAction::UpsertDomainMetadata { .. } => {
                unreachable!("geometry actions are extracted before deferred dispatch")
            }
        }
    }

    fn run_mechanical_dfm(&mut self) {
        let parts = self
            .session
            .scene()
            .parts
            .iter()
            .map(|part| {
                let (minimum, maximum) = part.mesh.positions.iter().fold(
                    ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]),
                    |(minimum, maximum), position| {
                        let position = position.map(f64::from);
                        (
                            [
                                minimum[0].min(position[0]),
                                minimum[1].min(position[1]),
                                minimum[2].min(position[2]),
                            ],
                            [
                                maximum[0].max(position[0]),
                                maximum[1].max(position[1]),
                                maximum[2].max(position[2]),
                            ],
                        )
                    },
                );
                mechanical_dfm::PartCheckInput {
                    id: part.feature_id.to_string(),
                    name: part.name.clone(),
                    bbox_mm: [
                        (maximum[0] - minimum[0]).max(0.001),
                        (maximum[1] - minimum[1]).max(0.001),
                        (maximum[2] - minimum[2]).max(0.001),
                    ],
                    minimum_wall_mm: None,
                    smallest_hole_mm: None,
                    material: part.material.as_ref().map(|material| material.name.clone()),
                }
            })
            .collect::<Vec<_>>();
        self.domain_report = Some(DomainReport::MechanicalDfm(mechanical_dfm::inspect(&parts)));
    }

    fn run_mechanical_drawing_check(&mut self) {
        self.domain_report = Some(DomainReport::MechanicalDrawing(
            mechanical_standards::DrawingSheet::default().inspect(),
        ));
    }

    fn run_mechanical_bom(&mut self) {
        let sources = self
            .session
            .document()
            .features
            .iter()
            .filter(|feature| !feature.primitive.is_reference_geometry())
            .map(|feature| mechanical_bom::BomSource {
                part_number: format!("CADX-{:04}", feature.id),
                description: feature.name.clone(),
                material: feature
                    .material
                    .as_ref()
                    .map(|material| material.name.clone()),
                revision: Some("A".into()),
            })
            .collect::<Vec<_>>();
        self.domain_report = Some(DomainReport::MechanicalBom(mechanical_bom::generate(
            sources,
        )));
    }

    fn model_panel(&mut self, root: &mut egui::Ui) {
        if !self.model_panel_open {
            return;
        }
        let translator = self.translator.clone();
        egui::Panel::left("model_panel")
            .default_size(238.0)
            .size_range(210.0..=360.0)
            .resizable(true)
            .frame(appearance::panel_frame(appearance::SURFACE))
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.label(appearance::icon("boxes", 14.0).color(appearance::ACCENT));
                    ui.label(
                        egui::RichText::new(translator.text("panel.model"))
                            .size(12.0)
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(self.session.document().features.len().to_string())
                                .monospace()
                                .size(10.0)
                                .color(appearance::TEXT_FAINT),
                        );
                    });
                });
                self.domain_inspector_summary(ui, &translator);
                ui.add_space(8.0);
                appearance::section_label(ui, translator.text("panel.features"));
                ui.add_space(4.0);

                let tree_height = (ui.available_height() * 0.36).clamp(110.0, 280.0);
                egui::ScrollArea::vertical()
                    .id_salt("feature_tree")
                    .max_height(tree_height)
                    .show(ui, |ui| {
                        let assemblies = self.session.document().assemblies.clone();
                        let features = self.session.document().features.clone();
                        let owned = assemblies
                            .iter()
                            .flat_map(|assembly| &assembly.occurrences)
                            .flat_map(|occurrence| &occurrence.feature_ids)
                            .copied()
                            .collect::<std::collections::BTreeSet<_>>();
                        for assembly in &assemblies {
                            egui::CollapsingHeader::new(&assembly.name)
                                .id_salt(("assembly", assembly.id))
                                .default_open(true)
                                .show(ui, |ui| {
                                    let roots = assembly
                                        .roots()
                                        .map(|occurrence| occurrence.id)
                                        .collect::<Vec<_>>();
                                    for root in roots {
                                        self.assembly_occurrence_row(
                                            ui,
                                            assembly,
                                            root,
                                            false,
                                            &translator,
                                        );
                                    }
                                });
                        }
                        for feature in features
                            .into_iter()
                            .filter(|feature| !owned.contains(&feature.id))
                        {
                            self.feature_row(ui, &feature, &translator, false);
                        }
                    });

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    appearance::section_label(ui, translator.text("panel.properties"));
                    if let Some(id) = self.selected {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let id = id.to_string();
                            ui.label(
                                egui::RichText::new(
                                    translator.format("panel.feature_id", &[("id", &id)]),
                                )
                                .monospace()
                                .size(9.0)
                                .color(appearance::TEXT_FAINT),
                            );
                        });
                    }
                });
                ui.add_space(5.0);

                egui::ScrollArea::vertical()
                    .id_salt("property_inspector")
                    .show(ui, |ui| {
                        if let Some(feature) = self
                            .selected
                            .and_then(|id| self.session.document().feature(id))
                            .cloned()
                        {
                            self.feature_properties(ui, &feature, &translator);
                        } else {
                            ui.label(
                                egui::RichText::new(translator.text("panel.no_selection"))
                                    .size(11.0)
                                    .color(appearance::TEXT_MUTED),
                            );
                        }
                    });
            });
    }

    fn domain_inspector_summary(&self, ui: &mut egui::Ui, translator: &Translator) {
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(appearance::icon("layers", 12.0).color(appearance::ACCENT));
            ui.label(
                egui::RichText::new(match self.active_domain {
                    DomainId::Ecad => translator.text("domain.pcb_inspector"),
                    DomainId::Aec => translator.text("domain.aec_inspector"),
                    DomainId::Mcad => translator.text("domain.mechanical_inspector"),
                })
                .size(10.0)
                .strong(),
            );
        });
        if self.active_domain == DomainId::Ecad {
            ui.label(format!(
                "{}  {} x {} x {} mm",
                self.pcb_board.name,
                self.pcb_board.width_mm,
                self.pcb_board.height_mm,
                self.pcb_board.thickness_mm
            ));
            ui.label(format!(
                "{}: {}  ·  {}: {}",
                translator.text("pcb.report.components"),
                self.pcb_board.components.len(),
                translator.text("domain.layers"),
                self.pcb_board.layers.len()
            ));
            ui.label(
                egui::RichText::new(format!(
                    "{} {:.2} mm  ·  {} {:.2} mm",
                    translator.text("domain.min_trace"),
                    self.pcb_board.rules.min_trace_width_mm,
                    translator.text("domain.clearance"),
                    self.pcb_board.rules.min_clearance_mm
                ))
                .monospace()
                .size(9.0)
                .color(appearance::TEXT_MUTED),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{}: {}  ·  {}: {}  ·  {}: {}",
                    translator.text("domain.nets"),
                    self.pcb_board.nets.len(),
                    translator.text("domain.routes"),
                    self.pcb_board.traces.len(),
                    translator.text("domain.vias"),
                    self.pcb_board.vias.len()
                ))
                .monospace()
                .size(9.0)
                .color(appearance::TEXT_MUTED),
            );
        } else if self.active_domain == DomainId::Aec {
            let bim_records = self
                .session
                .document()
                .domain_data
                .get("aec.bim")
                .map_or(0, BTreeMap::len);
            ui.label(
                egui::RichText::new(format!(
                    "{}  ·  {}",
                    translator.text("domain.bim_level"),
                    translator.text("domain.ifc_reference")
                ))
                .color(appearance::TEXT_MUTED),
            );
            ui.label(format!(
                "{}: {}  ·  {}: {}",
                translator.text("domain.bim_records"),
                bim_records,
                translator.text("domain.visible_parts"),
                self.session.scene().parts.len()
            ));
        } else {
            ui.label(
                egui::RichText::new(format!(
                    "{}  ·  {}",
                    translator.text("domain.gb_standard"),
                    translator.text("domain.first_angle")
                ))
                .color(appearance::TEXT_MUTED),
            );
            ui.label(format!(
                "{}: {}  ·  {}: {}  ·  {}: {}",
                translator.text("domain.visible_parts"),
                self.session.scene().parts.len(),
                translator.text("domain.assemblies"),
                self.session.document().assemblies.len(),
                translator.text("domain.standard_tools"),
                self.ai_tools.tools_for(DomainId::Mcad).len()
            ));
        }
        ui.add_space(4.0);
        ui.separator();
    }

    fn assembly_occurrence_row(
        &mut self,
        ui: &mut egui::Ui,
        assembly: &Assembly,
        occurrence_id: ComponentOccurrenceId,
        ancestor_suppressed: bool,
        translator: &Translator,
    ) {
        let Some(occurrence) = assembly.occurrence(occurrence_id) else {
            return;
        };
        let definition_name = assembly
            .definition(occurrence.definition_id)
            .map_or("", |definition| definition.name.as_str());
        let label = if occurrence.name == definition_name || definition_name.is_empty() {
            occurrence.name.clone()
        } else {
            format!("{}  [{}]", occurrence.name, definition_name)
        };
        let directly_suppressed = occurrence.suppressed;
        let effectively_suppressed = ancestor_suppressed || directly_suppressed;
        let assembly_id = assembly.id;
        let occurrence_id = occurrence.id;
        let state_id = ui.make_persistent_id(("occurrence", assembly_id, occurrence_id));
        egui::collapsing_header::CollapsingState::load_with_default_open(
            ui.ctx(),
            state_id,
            occurrence.parent_id.is_none(),
        )
        .show_header(ui, |ui| {
            let mut suppressed = directly_suppressed;
            let response = ui
                .checkbox(&mut suppressed, "")
                .on_hover_text(translator.text("tool.suppress_occurrence"));
            if response.changed() {
                let _ = self.execute(
                    vec![ModelCommand::SetOccurrenceSuppressed {
                        assembly_id,
                        occurrence_id,
                        suppressed,
                    }],
                    StatusMessage::Key("status.occurrence_suppression"),
                );
            }
            let text = egui::RichText::new(label).size(11.0);
            ui.label(if effectively_suppressed {
                text.color(appearance::TEXT_FAINT).strikethrough()
            } else {
                text.color(appearance::TEXT)
            });
        })
        .body(|ui| {
            for feature_id in &occurrence.feature_ids {
                if let Some(feature) = self.session.document().feature(*feature_id).cloned() {
                    self.feature_row(ui, &feature, translator, true);
                }
            }
            let children = assembly
                .children(occurrence.id)
                .map(|child| child.id)
                .collect::<Vec<_>>();
            for child in children {
                self.assembly_occurrence_row(
                    ui,
                    assembly,
                    child,
                    effectively_suppressed,
                    translator,
                );
            }
        });
    }

    fn feature_row(
        &mut self,
        ui: &mut egui::Ui,
        feature: &Feature,
        translator: &Translator,
        assembly_owned: bool,
    ) {
        ui.horizontal(|ui| {
            let (icon_name, primitive_key) = match feature.primitive {
                Primitive::Box { .. } => ("box", "primitive.box"),
                Primitive::Cylinder { .. } => ("cylinder", "primitive.cylinder"),
                Primitive::Sphere { .. } => ("circle", "primitive.sphere"),
                Primitive::Cone { .. } => ("triangle", "primitive.cone"),
                Primitive::Torus { .. } => ("circle-dot", "primitive.torus"),
                Primitive::Extrusion { .. } | Primitive::ExtrusionFromSketch { .. } => {
                    ("layers", "primitive.extrusion")
                }
                Primitive::RevolveFromSketch { .. } => ("rotate-cw", "primitive.revolve"),
                Primitive::LoftFromSketches { .. } => ("layers", "primitive.loft"),
                Primitive::ImportedStep { .. } => ("file-input", "primitive.imported_step"),
                Primitive::Boolean { .. } => ("combine", "primitive.boolean"),
                Primitive::Chamfer { .. } => ("octagon", "primitive.chamfer"),
                Primitive::Fillet { .. } => ("radius", "primitive.fillet"),
                Primitive::DatumPlane { .. } => ("layers", "primitive.datum_plane"),
                Primitive::DatumPoint { .. } => ("crosshair", "primitive.datum_point"),
                Primitive::Sketch { .. } => ("pencil", "primitive.sketch"),
            };
            let selected = self.selected == Some(feature.id);
            let label = format!("{}  {}", translator.text(primitive_key), feature.name);
            let row_width = (ui.available_width() - 113.0).max(80.0);
            let feature_response = ui.add_sized(
                [row_width, 30.0],
                egui::Button::selectable(selected, (appearance::icon(icon_name, 13.0), label)),
            );
            if feature_response.clicked() {
                self.selected = Some(feature.id);
                self.clear_topology_selection();
                self.sync_viewport();
            }
            feature_response.context_menu(|ui| {
                let tool_ids: &[&str] = match self.active_domain {
                    DomainId::Ecad => &["drc", "bom", "gerber"],
                    DomainId::Aec => &["bim-attrs", "quantity-takeoff", "clash"],
                    DomainId::Mcad => &["drawing", "dfm", "bom"],
                };
                for &tool_id in tool_ids {
                    let label = domain_tool_label(tool_id, self.active_domain, translator);
                    if ui.button(label).clicked() {
                        self.run_domain_tool(tool_id);
                        ui.close();
                    }
                }
            });

            let visibility_icon = if feature.visible { "eye" } else { "eye-off" };
            if icon_button(
                ui,
                visibility_icon,
                translator.text("tool.visibility"),
                true,
                false,
            )
            .clicked()
            {
                let _ = self.execute(
                    vec![ModelCommand::SetVisibility {
                        id: feature.id,
                        visible: !feature.visible,
                    }],
                    StatusMessage::Key("status.visibility"),
                );
            }
            if icon_button(ui, "copy", translator.text("tool.duplicate"), true, false).clicked() {
                self.duplicate_feature(feature.id);
            }
            if icon_button(
                ui,
                "trash-2",
                translator.text("tool.delete"),
                !assembly_owned,
                false,
            )
            .clicked()
            {
                let _ = self.execute(
                    vec![ModelCommand::Delete { id: feature.id }],
                    StatusMessage::Key("status.deleted"),
                );
            }
        });
        ui.add_space(2.0);
    }

    fn feature_properties(
        &mut self,
        ui: &mut egui::Ui,
        feature: &Feature,
        translator: &Translator,
    ) {
        let assembly_context = self
            .session
            .document()
            .assembly_occurrence_for_feature(feature.id)
            .map(|(assembly, occurrence)| {
                let definition = assembly.definition(occurrence.definition_id);
                AssemblyInspectorContext {
                    assembly_id: assembly.id,
                    assembly_name: assembly.name.clone(),
                    occurrence_id: occurrence.id,
                    occurrence_name: occurrence.name.clone(),
                    parent_occurrence_id: occurrence.parent_id,
                    occurrence_transform: occurrence.transform,
                    occurrence_suppressed: occurrence.suppressed,
                    occurrence_effectively_suppressed: assembly
                        .effective_suppression()
                        .ok()
                        .and_then(|effective| effective.get(&occurrence.id).copied())
                        .unwrap_or(occurrence.suppressed),
                    occurrence_source: occurrence.source,
                    definition_name: definition.map(|definition| definition.name.clone()),
                    definition_source: definition.and_then(|definition| definition.source),
                    mate: assembly.mate_for_child(occurrence.id).cloned(),
                    next_mate_id: assembly
                        .mates
                        .iter()
                        .map(|mate| mate.id)
                        .max()
                        .unwrap_or(0)
                        .checked_add(1),
                }
            });
        if let Some(reference) = self
            .selected_edges
            .last()
            .filter(|reference| reference.feature_id == feature.id)
        {
            property_label(ui, translator.text("property.edge_selection"), None);
            let fragment = reference.fragment.to_string();
            ui.label(
                egui::RichText::new(
                    translator.format("property.topology_fragment", &[("fragment", &fragment)]),
                )
                .monospace()
                .size(10.0)
                .color(appearance::ACCENT),
            );
            if let Some(edge) = self.session.scene().edge(reference) {
                let length = format!("{:.3}", edge.geometry.length);
                let error = edge
                    .geometry
                    .length_error_estimate
                    .map(|error| format!("{error:.2e}"));
                let (key, values) = match (edge.geometry.length_error_estimate, error.as_deref()) {
                    (Some(error), _) if error <= f64::EPSILON => (
                        "property.edge_length_exact",
                        vec![("length", length.as_str())],
                    ),
                    (Some(_), Some(error)) => (
                        "property.edge_length_numerical",
                        vec![("length", length.as_str()), ("error", error)],
                    ),
                    (None, _) => (
                        "property.edge_length_approximate",
                        vec![("length", length.as_str())],
                    ),
                    (Some(_), None) => unreachable!("formatted integration error must exist"),
                };
                ui.label(
                    egui::RichText::new(translator.format(key, &values))
                        .monospace()
                        .size(10.0)
                        .color(appearance::TEXT_MUTED),
                );
            }
            ui.add_space(8.0);
        }
        if let Some(reference) = self
            .selected_vertex
            .as_ref()
            .filter(|reference| reference.feature_id == feature.id)
        {
            property_label(ui, translator.text("property.vertex_selection"), None);
            let fragment = reference.fragment.to_string();
            ui.label(
                egui::RichText::new(
                    translator.format("property.topology_fragment", &[("fragment", &fragment)]),
                )
                .monospace()
                .size(10.0)
                .color(appearance::ACCENT),
            );
            if let Some(vertex) = self.session.scene().vertex(reference) {
                let [x, y, z] = vertex.geometry.position.map(|value| format!("{value:.3}"));
                ui.label(
                    egui::RichText::new(translator.format(
                        "property.vertex_position",
                        &[("x", &x), ("y", &y), ("z", &z)],
                    ))
                    .monospace()
                    .size(10.0)
                    .color(appearance::TEXT_MUTED),
                );
            }
            if tool_button(
                ui,
                "crosshair",
                translator.text("tool.create_datum_point"),
                translator.text("tool.create_datum_point"),
                true,
            )
            .clicked()
            {
                let _ = self.execute(
                    vec![ModelCommand::CreateDatumPoint {
                        name: String::new(),
                        vertex: reference.clone(),
                        offset: [0.0; 3],
                    }],
                    StatusMessage::Key("status.created_datum_point"),
                );
            }
            ui.add_space(8.0);
        }
        if let Some(reference) = self.selected_face.as_ref() {
            property_label(ui, translator.text("property.face_selection"), None);
            ui.label(
                egui::RichText::new(reference.to_string())
                    .monospace()
                    .size(9.0)
                    .color(appearance::ACCENT),
            );
            if reference.feature_id == feature.id
                && tool_button(
                    ui,
                    "layers",
                    translator.text("tool.create_datum"),
                    translator.text("tool.create_datum"),
                    planar_face_selection(self.session.scene(), Some(reference)).is_some(),
                )
                .clicked()
            {
                let _ = self.execute(
                    vec![ModelCommand::CreateDatumPlane {
                        name: String::new(),
                        face: reference.clone(),
                        offset: 0.0,
                    }],
                    StatusMessage::Key("status.created_datum"),
                );
            }
            ui.add_space(8.0);
        }
        property_label(ui, translator.text("property.name"), None);
        let mut name = feature.name.clone();
        if ui
            .add_sized(
                [ui.available_width(), 28.0],
                egui::TextEdit::singleline(&mut name),
            )
            .changed()
        {
            let _ = self.execute(
                vec![ModelCommand::Rename {
                    id: feature.id,
                    name,
                }],
                StatusMessage::Key("status.renamed"),
            );
        }

        if let Some(context) = &assembly_context {
            ui.add_space(10.0);
            property_label(ui, translator.text("property.assembly"), None);
            ui.label(
                egui::RichText::new(format!(
                    "{}  #{}",
                    context.assembly_name, context.assembly_id
                ))
                .size(10.0)
                .color(appearance::TEXT_MUTED),
            );
            property_label(ui, translator.text("property.occurrence"), None);
            let source = context
                .occurrence_source
                .map(|source| format!("  STEP #{}", source.entity_id))
                .unwrap_or_default();
            ui.label(
                egui::RichText::new(format!(
                    "{}  #{}{}",
                    context.occurrence_name, context.occurrence_id, source
                ))
                .size(10.0)
                .color(appearance::ACCENT),
            );
            property_label(ui, translator.text("property.suppression"), None);
            let mut suppressed = context.occurrence_suppressed;
            if ui
                .checkbox(&mut suppressed, translator.text("property.suppressed"))
                .changed()
            {
                let _ = self.execute(
                    vec![ModelCommand::SetOccurrenceSuppressed {
                        assembly_id: context.assembly_id,
                        occurrence_id: context.occurrence_id,
                        suppressed,
                    }],
                    StatusMessage::Key("status.occurrence_suppression"),
                );
            }
            if context.occurrence_effectively_suppressed && !context.occurrence_suppressed {
                ui.label(
                    egui::RichText::new(translator.text("property.suppressed_by_ancestor"))
                        .size(10.0)
                        .color(appearance::TEXT_MUTED),
                );
            }
            property_label(ui, translator.text("property.component_definition"), None);
            let source = context
                .definition_source
                .map(|source| format!("  STEP #{}", source.entity_id))
                .unwrap_or_default();
            ui.label(
                egui::RichText::new(format!(
                    "{}{}",
                    context.definition_name.as_deref().unwrap_or(""),
                    source
                ))
                .size(10.0)
                .color(appearance::TEXT_MUTED),
            );

            if let Some(mate) = &context.mate {
                property_label(ui, translator.text("property.assembly_mate"), None);
                let kind = translator.text(assembly_mate_kind_key(&mate.kind));
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{}  #{}, {kind}", mate.name, mate.id))
                            .size(10.0)
                            .color(appearance::ACCENT),
                    );
                    if icon_button(
                        ui,
                        "trash-2",
                        translator.text("tool.delete_assembly_mate"),
                        true,
                        false,
                    )
                    .clicked()
                    {
                        let _ = self.execute(
                            vec![ModelCommand::DeleteAssemblyMate {
                                assembly_id: context.assembly_id,
                                mate_id: mate.id,
                            }],
                            StatusMessage::Key("status.deleted_assembly_mate"),
                        );
                    }
                });
                if let Some(axis) = assembly_mate_axis(&mate.kind) {
                    property_label(ui, translator.text("property.mate_axis"), None);
                    let [x, y, z] = axis.map(|value| format!("{value:.4}"));
                    ui.label(
                        egui::RichText::new(format!("X {x}   Y {y}   Z {z}"))
                            .monospace()
                            .size(9.0)
                            .color(appearance::TEXT_MUTED),
                    );
                }
                if !matches!(mate.kind, AssemblyMateKind::Fixed) {
                    let (minimum, maximum, suffix, units_key) = match &mate.kind {
                        AssemblyMateKind::Revolute { limits_deg, .. } => {
                            let limits = limits_deg.unwrap_or(AssemblyMateLimits {
                                min: -360_000.0,
                                max: 360_000.0,
                            });
                            (limits.min, limits.max, "°", "property.units_deg")
                        }
                        AssemblyMateKind::Slider { limits_mm, .. } => {
                            let limits = limits_mm.unwrap_or(AssemblyMateLimits {
                                min: -100_000.0,
                                max: 100_000.0,
                            });
                            (limits.min, limits.max, " mm", "property.units_mm")
                        }
                        AssemblyMateKind::Fixed => unreachable!(),
                    };
                    property_label(
                        ui,
                        translator.text("property.mate_state"),
                        Some(translator.text(units_key)),
                    );
                    let mut state = mate.state;
                    if ui
                        .add(
                            egui::DragValue::new(&mut state)
                                .speed(0.25)
                                .range(minimum..=maximum)
                                .suffix(suffix),
                        )
                        .changed()
                    {
                        let _ = self.execute(
                            vec![ModelCommand::SetAssemblyMateState {
                                assembly_id: context.assembly_id,
                                mate_id: mate.id,
                                state,
                            }],
                            StatusMessage::Key("status.moved_assembly_mate"),
                        );
                    }
                }
                assembly_mate_frame_ui(
                    ui,
                    translator.text("property.parent_anchor_frame"),
                    mate.parent_frame,
                );
                assembly_mate_frame_ui(
                    ui,
                    translator.text("property.child_anchor_frame"),
                    mate.child_frame,
                );
            } else if let (Some(parent_occurrence_id), Some(mate_id)) =
                (context.parent_occurrence_id, context.next_mate_id)
            {
                property_label(ui, translator.text("property.assembly_mate"), None);
                let mut create = None;
                ui.menu_button(translator.text("tool.add_assembly_mate"), |ui| {
                    if ui.button(translator.text("mate.fixed")).clicked() {
                        create = Some((
                            translator.text("mate.fixed").to_owned(),
                            AssemblyMateKind::Fixed,
                        ));
                        ui.close();
                    }
                    ui.separator();
                    for (key, axis) in mate_axis_options() {
                        if ui
                            .button(translator.format("mate.revolute_axis", &[("axis", key)]))
                            .clicked()
                        {
                            create = Some((
                                translator.text("mate.revolute").to_owned(),
                                AssemblyMateKind::Revolute {
                                    axis,
                                    limits_deg: None,
                                },
                            ));
                            ui.close();
                        }
                    }
                    ui.separator();
                    for (key, axis) in mate_axis_options() {
                        if ui
                            .button(translator.format("mate.slider_axis", &[("axis", key)]))
                            .clicked()
                        {
                            create = Some((
                                translator.text("mate.slider").to_owned(),
                                AssemblyMateKind::Slider {
                                    axis,
                                    limits_mm: None,
                                },
                            ));
                            ui.close();
                        }
                    }
                });
                if let Some((name, kind)) = create {
                    let _ = self.execute(
                        vec![ModelCommand::CreateAssemblyMate {
                            assembly_id: context.assembly_id,
                            mate: AssemblyMate {
                                id: mate_id,
                                name,
                                parent_occurrence_id,
                                child_occurrence_id: context.occurrence_id,
                                parent_frame: context.occurrence_transform,
                                child_frame: AssemblyTransform::IDENTITY,
                                kind,
                                state: 0.0,
                            },
                        }],
                        StatusMessage::Key("status.created_assembly_mate"),
                    );
                }
            }

            property_label(ui, translator.text("property.local_placement"), None);
            property_label(
                ui,
                translator.text("property.local_position"),
                Some(translator.text("property.units_mm")),
            );
            let mut local_position = context.occurrence_transform.translation;
            let position_changed = ui
                .add_enabled_ui(context.mate.is_none(), |ui| {
                    vec3_editor(ui, &mut local_position, -100_000.0..=100_000.0)
                })
                .inner;
            if position_changed {
                let _ = self.execute(
                    vec![ModelCommand::SetOccurrenceTransform {
                        assembly_id: context.assembly_id,
                        occurrence_id: context.occurrence_id,
                        position: local_position,
                        rotation: context.occurrence_transform.euler_xyz_degrees(),
                    }],
                    StatusMessage::Key("status.moved_occurrence"),
                );
            }
            property_label(
                ui,
                translator.text("property.local_rotation"),
                Some(translator.text("property.units_deg")),
            );
            let mut local_rotation = context.occurrence_transform.euler_xyz_degrees();
            let rotation_changed = ui
                .add_enabled_ui(context.mate.is_none(), |ui| {
                    vec3_editor_with_suffix(ui, &mut local_rotation, -360.0..=360.0, "°")
                })
                .inner;
            if rotation_changed {
                let _ = self.execute(
                    vec![ModelCommand::SetOccurrenceTransform {
                        assembly_id: context.assembly_id,
                        occurrence_id: context.occurrence_id,
                        position: context.occurrence_transform.translation,
                        rotation: local_rotation,
                    }],
                    StatusMessage::Key("status.moved_occurrence"),
                );
            }
        }

        if !matches!(
            feature.primitive,
            Primitive::DatumPlane { .. } | Primitive::DatumPoint { .. }
        ) {
            let is_sketch = matches!(feature.primitive, Primitive::Sketch { .. });
            ui.add_space(10.0);
            property_label(
                ui,
                translator.text(if is_sketch {
                    "property.local_offset"
                } else {
                    "property.position"
                }),
                Some(translator.text("property.units_mm")),
            );
            let mut position = feature.translation.as_array();
            let position_changed = ui
                .add_enabled_ui(assembly_context.is_none(), |ui| {
                    vec3_editor(ui, &mut position, -100_000.0..=100_000.0)
                })
                .inner;
            if position_changed {
                let _ = self.execute(
                    vec![ModelCommand::Move {
                        id: feature.id,
                        position,
                    }],
                    StatusMessage::Key("status.moved"),
                );
            }

            ui.add_space(10.0);
            if is_sketch {
                property_label(
                    ui,
                    translator.text("property.plane_angle"),
                    Some(translator.text("property.units_deg")),
                );
                let mut angle = feature.rotation.z;
                if ui
                    .add_enabled(
                        assembly_context.is_none(),
                        egui::DragValue::new(&mut angle)
                            .speed(1.0)
                            .range(-360.0..=360.0)
                            .suffix("°"),
                    )
                    .changed()
                {
                    let _ = self.execute(
                        vec![ModelCommand::Rotate {
                            id: feature.id,
                            rotation: [0.0, 0.0, angle],
                        }],
                        StatusMessage::Key("status.rotated"),
                    );
                }
            } else {
                property_label(
                    ui,
                    translator.text("property.rotation"),
                    Some(translator.text("property.units_deg")),
                );
                let mut rotation = feature.rotation.as_array();
                let rotation_changed = ui
                    .add_enabled_ui(assembly_context.is_none(), |ui| {
                        vec3_editor_with_suffix(ui, &mut rotation, -360.0..=360.0, "°")
                    })
                    .inner;
                if rotation_changed {
                    let _ = self.execute(
                        vec![ModelCommand::Rotate {
                            id: feature.id,
                            rotation,
                        }],
                        StatusMessage::Key("status.rotated"),
                    );
                }
            }
        }

        ui.add_space(10.0);
        property_label(ui, translator.text("property.appearance"), None);
        let mut color = feature.color;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(translator.text("property.color"))
                    .size(11.0)
                    .color(appearance::TEXT_MUTED),
            );
            if ui.color_edit_button_rgba_unmultiplied(&mut color).changed() {
                let _ = self.execute(
                    vec![ModelCommand::SetColor {
                        id: feature.id,
                        color,
                    }],
                    StatusMessage::Key("status.color"),
                );
            }
        });

        if !feature.primitive.is_reference_geometry() {
            ui.add_space(10.0);
            property_label(ui, translator.text("property.material"), None);
            let selected_text = feature.material.as_ref().map_or_else(
                || translator.text("property.material_unassigned").to_owned(),
                |material| {
                    MATERIAL_PRESETS
                        .iter()
                        .find(|preset| preset.name == material.name)
                        .map_or_else(
                            || material.name.clone(),
                            |preset| translator.text(preset.translation_key).to_owned(),
                        )
                },
            );
            let mut selected_preset = None;
            let mut clear_material = false;
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt(("material_preset", feature.id))
                    .selected_text(selected_text)
                    .width((ui.available_width() - 38.0).max(80.0))
                    .show_ui(ui, |ui| {
                        for preset in MATERIAL_PRESETS {
                            if ui
                                .selectable_label(false, translator.text(preset.translation_key))
                                .clicked()
                            {
                                selected_preset = Some(preset);
                            }
                        }
                    });
                clear_material = icon_button(
                    ui,
                    "x",
                    translator.text("tool.clear_material"),
                    feature.material.is_some(),
                    false,
                )
                .clicked();
            });
            if clear_material {
                let _ = self.execute(
                    vec![ModelCommand::ClearMaterial { id: feature.id }],
                    StatusMessage::Key("status.material_cleared"),
                );
            } else if let Some(preset) = selected_preset {
                let _ = self.execute(
                    vec![ModelCommand::SetMaterial {
                        id: feature.id,
                        name: preset.name.into(),
                        density_kg_m3: preset.density_kg_m3,
                    }],
                    StatusMessage::Key("status.material"),
                );
            }

            if let Some(material) = &feature.material {
                let mut name = material.name.clone();
                let mut density_kg_m3 = material.density_kg_m3;
                let mut changed = false;
                egui::Grid::new(("material_properties", feature.id))
                    .num_columns(2)
                    .spacing([8.0, 7.0])
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(translator.text("property.material_name"))
                                .size(11.0)
                                .color(appearance::TEXT_MUTED),
                        );
                        changed |= ui
                            .add_sized(
                                [ui.available_width(), 24.0],
                                egui::TextEdit::singleline(&mut name),
                            )
                            .changed();
                        ui.end_row();
                        ui.label(
                            egui::RichText::new(translator.text("property.density"))
                                .size(11.0)
                                .color(appearance::TEXT_MUTED),
                        );
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut density_kg_m3)
                                    .speed(10.0)
                                    .range(0.1..=100_000.0)
                                    .suffix(" kg/m³"),
                            )
                            .changed();
                        ui.end_row();
                    });
                if changed {
                    let _ = self.execute(
                        vec![ModelCommand::SetMaterial {
                            id: feature.id,
                            name,
                            density_kg_m3,
                        }],
                        StatusMessage::Key("status.material"),
                    );
                }
                if let Some(mass) = analyze_scene(self.session.scene(), None)
                    .ok()
                    .and_then(|analysis| analysis.part(feature.id).and_then(|part| part.mass_kg))
                {
                    let mass = format!("{mass:.6}");
                    ui.label(
                        egui::RichText::new(
                            translator.format("property.mass_value", &[("mass", &mass)]),
                        )
                        .monospace()
                        .size(10.0)
                        .color(appearance::TEXT_MUTED),
                    );
                }
            }
        }

        if let Some(sketch_id) = feature.primitive.source_sketch() {
            ui.add_space(10.0);
            let id = sketch_id.to_string();
            property_label(ui, translator.text("property.dependencies"), None);
            ui.label(
                egui::RichText::new(translator.format("property.source_sketch", &[("id", &id)]))
                    .monospace()
                    .size(10.0)
                    .color(appearance::ACCENT),
            );
        }
        if let Primitive::LoftFromSketches { sketch_ids, .. } = &feature.primitive {
            ui.add_space(10.0);
            let count = sketch_ids.len().to_string();
            property_label(
                ui,
                translator.text("property.source_sketches"),
                Some(
                    translator
                        .format("loft.section_count", &[("count", &count)])
                        .as_str(),
                ),
            );
            for (index, sketch_id) in sketch_ids.iter().enumerate() {
                let order = (index + 1).to_string();
                let id = sketch_id.to_string();
                ui.label(
                    egui::RichText::new(
                        translator
                            .format("loft.section_reference", &[("order", &order), ("id", &id)]),
                    )
                    .monospace()
                    .size(10.0)
                    .color(appearance::ACCENT),
                );
            }
        }
        if let Primitive::Boolean {
            operation,
            left,
            right,
        } = &feature.primitive
        {
            ui.add_space(10.0);
            property_label(ui, translator.text("property.dependencies"), None);
            ui.label(
                egui::RichText::new(format!("{}  #{left} + #{right}", operation.label()))
                    .monospace()
                    .size(10.0)
                    .color(appearance::ACCENT),
            );
        }
        if let Primitive::Chamfer { edges, .. } | Primitive::Fillet { edges, .. } =
            &feature.primitive
        {
            ui.add_space(10.0);
            let count = edges.len().to_string();
            property_label(
                ui,
                translator.text("property.dependencies"),
                Some(
                    translator
                        .format("property.edge_count", &[("count", &count)])
                        .as_str(),
                ),
            );
            for edge in edges {
                ui.label(
                    egui::RichText::new(edge.to_string())
                        .monospace()
                        .size(9.0)
                        .color(appearance::ACCENT),
                );
            }
        }

        ui.add_space(10.0);
        match &feature.primitive {
            Primitive::Box { size } => {
                property_label(
                    ui,
                    translator.text("property.size"),
                    Some(translator.text("property.units_mm")),
                );
                let mut value = size.as_array();
                if vec3_editor(ui, &mut value, 0.1..=100_000.0) {
                    let _ = self.execute(
                        vec![ModelCommand::ResizeBox {
                            id: feature.id,
                            size: value,
                        }],
                        StatusMessage::Key("status.resized_box"),
                    );
                }
            }
            Primitive::Cylinder { radius, height } => {
                let mut radius = *radius;
                let mut height = *height;
                let mut changed = false;
                egui::Grid::new("cylinder_dimensions")
                    .num_columns(2)
                    .spacing([8.0, 7.0])
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(translator.text("property.radius"))
                                .size(11.0)
                                .color(appearance::TEXT_MUTED),
                        );
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut radius)
                                    .speed(0.25)
                                    .range(0.1..=100_000.0)
                                    .suffix(" mm"),
                            )
                            .changed();
                        ui.end_row();
                        ui.label(
                            egui::RichText::new(translator.text("property.height"))
                                .size(11.0)
                                .color(appearance::TEXT_MUTED),
                        );
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut height)
                                    .speed(0.25)
                                    .range(0.1..=100_000.0)
                                    .suffix(" mm"),
                            )
                            .changed();
                        ui.end_row();
                    });
                if changed {
                    let _ = self.execute(
                        vec![ModelCommand::ResizeCylinder {
                            id: feature.id,
                            radius,
                            height,
                        }],
                        StatusMessage::Key("status.resized_cylinder"),
                    );
                }
            }
            Primitive::Sphere { radius } => {
                property_label(
                    ui,
                    translator.text("property.radius"),
                    Some(translator.text("property.units_mm")),
                );
                let mut radius = *radius;
                if ui
                    .add(
                        egui::DragValue::new(&mut radius)
                            .speed(0.25)
                            .range(0.1..=100_000.0)
                            .suffix(" mm"),
                    )
                    .changed()
                {
                    let _ = self.execute(
                        vec![ModelCommand::ResizeSphere {
                            id: feature.id,
                            radius,
                        }],
                        StatusMessage::Key("status.resized_sphere"),
                    );
                }
            }
            Primitive::Cone {
                bottom_radius,
                top_radius,
                height,
            } => {
                let mut bottom_radius = *bottom_radius;
                let mut top_radius = *top_radius;
                let mut height = *height;
                let mut changed = false;
                egui::Grid::new("cone_dimensions")
                    .num_columns(2)
                    .spacing([8.0, 7.0])
                    .show(ui, |ui| {
                        for (label, value, range) in [
                            (
                                translator.text("property.bottom_radius"),
                                &mut bottom_radius,
                                0.1..=100_000.0,
                            ),
                            (
                                translator.text("property.top_radius"),
                                &mut top_radius,
                                0.0..=100_000.0,
                            ),
                            (
                                translator.text("property.height"),
                                &mut height,
                                0.1..=100_000.0,
                            ),
                        ] {
                            ui.label(
                                egui::RichText::new(label)
                                    .size(11.0)
                                    .color(appearance::TEXT_MUTED),
                            );
                            changed |= ui
                                .add(
                                    egui::DragValue::new(value)
                                        .speed(0.25)
                                        .range(range)
                                        .suffix(" mm"),
                                )
                                .changed();
                            ui.end_row();
                        }
                    });
                if changed {
                    let _ = self.execute(
                        vec![ModelCommand::ResizeCone {
                            id: feature.id,
                            bottom_radius,
                            top_radius,
                            height,
                        }],
                        StatusMessage::Key("status.resized_cone"),
                    );
                }
            }
            Primitive::Torus {
                major_radius,
                minor_radius,
            } => {
                let mut major_radius = *major_radius;
                let mut minor_radius = *minor_radius;
                let mut changed = false;
                egui::Grid::new("torus_dimensions")
                    .num_columns(2)
                    .spacing([8.0, 7.0])
                    .show(ui, |ui| {
                        for (label, value, range) in [
                            (
                                translator.text("property.major_radius"),
                                &mut major_radius,
                                0.2..=100_000.0,
                            ),
                            (
                                translator.text("property.minor_radius"),
                                &mut minor_radius,
                                0.1..=100_000.0,
                            ),
                        ] {
                            ui.label(
                                egui::RichText::new(label)
                                    .size(11.0)
                                    .color(appearance::TEXT_MUTED),
                            );
                            changed |= ui
                                .add(
                                    egui::DragValue::new(value)
                                        .speed(0.25)
                                        .range(range)
                                        .suffix(" mm"),
                                )
                                .changed();
                            ui.end_row();
                        }
                    });
                if changed {
                    let _ = self.execute(
                        vec![ModelCommand::ResizeTorus {
                            id: feature.id,
                            major_radius,
                            minor_radius,
                        }],
                        StatusMessage::Key("status.resized_torus"),
                    );
                }
            }
            Primitive::Extrusion { profile, height } => {
                let mut profile = profile.clone();
                let mut height = *height;
                let mut changed = false;
                property_label(
                    ui,
                    translator.text("property.profile"),
                    Some(translator.text("property.units_mm")),
                );
                egui::Grid::new("extrusion_profile")
                    .num_columns(3)
                    .spacing([6.0, 6.0])
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new("#")
                                .size(10.0)
                                .color(appearance::TEXT_FAINT),
                        );
                        ui.label(
                            egui::RichText::new("X")
                                .size(10.0)
                                .color(appearance::TEXT_MUTED),
                        );
                        ui.label(
                            egui::RichText::new("Y")
                                .size(10.0)
                                .color(appearance::TEXT_MUTED),
                        );
                        ui.end_row();
                        for (index, point) in profile.iter_mut().enumerate() {
                            ui.label(
                                egui::RichText::new((index + 1).to_string())
                                    .monospace()
                                    .size(9.0)
                                    .color(appearance::TEXT_FAINT),
                            );
                            changed |= ui
                                .add(
                                    egui::DragValue::new(&mut point[0])
                                        .speed(0.25)
                                        .suffix(" mm"),
                                )
                                .changed();
                            changed |= ui
                                .add(
                                    egui::DragValue::new(&mut point[1])
                                        .speed(0.25)
                                        .suffix(" mm"),
                                )
                                .changed();
                            ui.end_row();
                        }
                    });
                property_label(
                    ui,
                    translator.text("property.height"),
                    Some(translator.text("property.units_mm")),
                );
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut height)
                            .speed(0.25)
                            .range(0.1..=100_000.0)
                            .suffix(" mm"),
                    )
                    .changed();
                if changed {
                    let _ = self.execute(
                        vec![ModelCommand::ResizeExtrusion {
                            id: feature.id,
                            profile,
                            height,
                        }],
                        StatusMessage::Key("status.resized_extrusion"),
                    );
                }
            }
            Primitive::ExtrusionFromSketch { height, .. } => {
                property_label(
                    ui,
                    translator.text("property.height"),
                    Some(translator.text("property.units_mm")),
                );
                let mut height = *height;
                if ui
                    .add(
                        egui::DragValue::new(&mut height)
                            .speed(0.25)
                            .range(0.1..=100_000.0)
                            .suffix(" mm"),
                    )
                    .changed()
                {
                    let _ = self.execute(
                        vec![ModelCommand::ResizeExtrusion {
                            id: feature.id,
                            profile: Vec::new(),
                            height,
                        }],
                        StatusMessage::Key("status.resized_extrusion"),
                    );
                }
            }
            Primitive::RevolveFromSketch {
                axis_origin,
                axis_direction,
                angle,
                ..
            } => {
                let mut axis_origin = *axis_origin;
                let mut axis_direction = *axis_direction;
                let mut angle = *angle;
                let mut changed = false;
                property_label(
                    ui,
                    translator.text("property.axis_origin"),
                    Some(translator.text("property.units_mm")),
                );
                for value in &mut axis_origin {
                    changed |= ui
                        .add(egui::DragValue::new(value).speed(0.25).suffix(" mm"))
                        .changed();
                }
                property_label(ui, translator.text("property.axis_direction"), None);
                for value in &mut axis_direction {
                    changed |= ui.add(egui::DragValue::new(value).speed(0.05)).changed();
                }
                property_label(
                    ui,
                    translator.text("property.angle"),
                    Some(translator.text("property.units_deg")),
                );
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut angle)
                            .speed(1.0)
                            .range(0.1..=360.0)
                            .suffix("°"),
                    )
                    .changed();
                if changed {
                    let _ = self.execute(
                        vec![ModelCommand::ResizeRevolve {
                            id: feature.id,
                            axis_origin,
                            axis_direction,
                            angle,
                        }],
                        StatusMessage::Key("status.resized_revolve"),
                    );
                }
            }
            Primitive::Sketch {
                plane,
                region,
                construction,
                constraints,
            } => {
                let datum_options = self
                    .session
                    .document()
                    .features
                    .iter()
                    .filter_map(|candidate| {
                        matches!(candidate.primitive, Primitive::DatumPlane { .. })
                            .then_some((candidate.id, candidate.name.clone()))
                    })
                    .collect::<Vec<_>>();
                let mut selected_plane = plane.clone();
                property_label(ui, translator.text("property.sketch_plane"), None);
                egui::ComboBox::from_id_salt(("sketch_plane", feature.id))
                    .selected_text(sketch_plane_label(
                        &selected_plane,
                        &datum_options,
                        translator,
                    ))
                    .width(ui.available_width())
                    .show_ui(ui, |ui| {
                        for (candidate, key) in [
                            (SketchPlane::WorldXy, "sketch_plane.world_xy"),
                            (SketchPlane::WorldXz, "sketch_plane.world_xz"),
                            (SketchPlane::WorldYz, "sketch_plane.world_yz"),
                        ] {
                            ui.selectable_value(
                                &mut selected_plane,
                                candidate,
                                translator.text(key),
                            );
                        }
                        for (datum_id, name) in &datum_options {
                            ui.selectable_value(
                                &mut selected_plane,
                                SketchPlane::DatumPlane {
                                    datum_id: *datum_id,
                                },
                                translator.format("sketch_plane.datum", &[("name", name.as_str())]),
                            );
                        }
                        if let SketchPlane::PlanarFace { face } = plane {
                            ui.selectable_value(
                                &mut selected_plane,
                                plane.clone(),
                                translator.format(
                                    "sketch_plane.face",
                                    &[("face", face.to_string().as_str())],
                                ),
                            );
                        }
                    });
                if selected_plane != *plane {
                    let _ = self.execute(
                        vec![ModelCommand::SetSketchPlane {
                            id: feature.id,
                            plane: selected_plane,
                        }],
                        StatusMessage::Key("status.updated_sketch_plane"),
                    );
                }
                let mut region = region.clone();
                let mut construction = construction.clone();
                let mut constraints = constraints.clone();
                let curved = !region.profile.is_linear()
                    || region.holes.iter().any(|hole| !hole.is_linear());
                let mut region_changed = false;
                let mut construction_changed = false;
                let mut constraints_changed = false;
                property_label(
                    ui,
                    translator.text("property.profile"),
                    Some(translator.text("property.units_mm")),
                );
                if curved {
                    region_changed |= sketch_segment_editor(
                        ui,
                        (feature.id, "profile"),
                        &mut region.profile,
                        translator,
                    );
                    property_label(ui, translator.text("property.holes"), None);
                    let mut remove_hole = None;
                    for (hole_index, hole) in region.holes.iter_mut().enumerate() {
                        egui::CollapsingHeader::new(
                            translator.format(
                                "property.hole",
                                &[("index", &(hole_index + 1).to_string())],
                            ),
                        )
                        .id_salt(("sketch_hole", feature.id, hole_index))
                        .default_open(true)
                        .show(ui, |ui| {
                            region_changed |= sketch_segment_editor(
                                ui,
                                (feature.id, hole_index),
                                hole,
                                translator,
                            );
                            if icon_button(
                                ui,
                                "trash-2",
                                translator.text("tool.remove_hole"),
                                true,
                                false,
                            )
                            .clicked()
                            {
                                remove_hole = Some(hole_index);
                            }
                        });
                    }
                    if let Some(index) = remove_hole {
                        region.holes.remove(index);
                        region_changed = true;
                    }
                } else {
                    let mut profile = region.profile.vertices();
                    let mut holes = region
                        .holes
                        .iter()
                        .map(SketchLoop2D::vertices)
                        .collect::<Vec<_>>();
                    let mut linear_changed =
                        linear_loop_editor(ui, (feature.id, "profile"), &mut profile);
                    property_label(ui, translator.text("property.holes"), None);
                    let mut remove_hole = None;
                    for (hole_index, hole) in holes.iter_mut().enumerate() {
                        egui::CollapsingHeader::new(
                            translator.format(
                                "property.hole",
                                &[("index", &(hole_index + 1).to_string())],
                            ),
                        )
                        .id_salt(("sketch_hole", feature.id, hole_index))
                        .default_open(true)
                        .show(ui, |ui| {
                            linear_changed |=
                                linear_loop_editor(ui, (feature.id, hole_index), hole);
                            if icon_button(
                                ui,
                                "trash-2",
                                translator.text("tool.remove_hole"),
                                true,
                                false,
                            )
                            .clicked()
                            {
                                remove_hole = Some(hole_index);
                            }
                        });
                    }
                    if let Some(index) = remove_hole {
                        holes.remove(index);
                        linear_changed = true;
                    }
                    if linear_changed {
                        region = SketchRegion2D::from_polygons(profile, holes);
                        region_changed = true;
                    }
                }
                let suggested_hole = suggest_sketch_hole(&region);
                if tool_button(
                    ui,
                    "plus",
                    translator.text("tool.add_hole"),
                    translator.text("tool.add_hole"),
                    suggested_hole.is_some(),
                )
                .clicked()
                    && let Some(hole) = suggested_hole
                {
                    region.holes.push(hole);
                    region_changed = true;
                }
                property_label(ui, translator.text("property.construction"), None);
                construction_changed |= sketch_construction_editor(
                    ui,
                    feature.id,
                    &region.profile,
                    &mut construction,
                    translator,
                );
                property_label(ui, translator.text("property.constraints"), None);
                let solve_diagnostic = self.session.scene().sketch_diagnostic(feature.id).cloned();
                let sketch_failure = (self.last_sketch_failure_feature == Some(feature.id))
                    .then(|| self.last_sketch_failure.clone())
                    .flatten();
                if let Some(diagnostic) = &solve_diagnostic {
                    sketch_solve_diagnostic_ui(ui, diagnostic, translator);
                }
                if let Some(diagnostic) = &sketch_failure {
                    sketch_failure_diagnostic_ui(ui, diagnostic, translator);
                }
                let original_constraints = constraints.clone();
                let constraint_options =
                    SketchConstraintOptions::new(&region.profile, &construction);
                let point_max = constraint_options.all_points.last().copied().unwrap_or(0);
                let redundant_constraints = solve_diagnostic
                    .as_ref()
                    .map(|diagnostic| diagnostic.redundant_constraints.clone())
                    .unwrap_or_default();
                let conflicting_constraints = sketch_failure
                    .as_ref()
                    .map(|diagnostic| diagnostic.constraint_indices.clone())
                    .unwrap_or_default();
                let mut remove_constraint = None;
                for (index, constraint) in constraints.iter_mut().enumerate() {
                    ui.vertical(|ui| {
                        let index_id = u32::try_from(index).unwrap_or(u32::MAX);
                        let color = if conflicting_constraints.contains(&index_id) {
                            appearance::DANGER
                        } else if redundant_constraints.contains(&index_id) {
                            appearance::WARNING
                        } else {
                            appearance::TEXT
                        };
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} {}",
                                    index + 1,
                                    translator.text(constraint_label_key(constraint))
                                ))
                                .size(10.0)
                                .color(color),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if icon_button(
                                        ui,
                                        "trash-2",
                                        translator.text("tool.remove_constraint"),
                                        true,
                                        false,
                                    )
                                    .clicked()
                                    {
                                        remove_constraint = Some(index);
                                    }
                                },
                            );
                        });
                        ui.horizontal_wrapped(|ui| match constraint {
                            Constraint::Coincident { first, second } => {
                                constraints_changed |= point_id_editor(ui, first, point_max);
                                constraints_changed |= point_id_editor(ui, second, point_max);
                            }
                            Constraint::Horizontal { segment }
                            | Constraint::Vertical { segment } => {
                                constraints_changed |= sketch_segment_selector(
                                    ui,
                                    (feature.id, index, "line"),
                                    segment,
                                    &constraint_options.line_segments,
                                );
                            }
                            Constraint::Fixed { point, x, y } => {
                                constraints_changed |= point_id_editor(ui, point, point_max);
                                constraints_changed |=
                                    ui.add(egui::DragValue::new(x).suffix(" mm")).changed();
                                constraints_changed |=
                                    ui.add(egui::DragValue::new(y).suffix(" mm")).changed();
                            }
                            Constraint::Distance {
                                first,
                                second,
                                distance,
                            } => {
                                constraints_changed |= point_id_editor(ui, first, point_max);
                                constraints_changed |= point_id_editor(ui, second, point_max);
                                constraints_changed |= ui
                                    .add(egui::DragValue::new(distance).suffix(" mm"))
                                    .changed();
                            }
                            Constraint::HorizontalDistance {
                                first,
                                second,
                                distance,
                            }
                            | Constraint::VerticalDistance {
                                first,
                                second,
                                distance,
                            } => {
                                constraints_changed |= sketch_point_pair_selector(
                                    ui,
                                    (feature.id, index, "point_dimension"),
                                    first,
                                    second,
                                    &constraint_options.dimension_point_pairs,
                                );
                                constraints_changed |= ui
                                    .add(egui::DragValue::new(distance).suffix(" mm"))
                                    .changed();
                            }
                            Constraint::PointLineDistance {
                                point,
                                line,
                                distance,
                            } => {
                                constraints_changed |= sketch_point_segment_pair_selector(
                                    ui,
                                    (feature.id, index, "point_line_distance"),
                                    point,
                                    line,
                                    &constraint_options.point_line_pairs,
                                );
                                constraints_changed |= ui
                                    .add(
                                        egui::DragValue::new(distance)
                                            .range(0.0..=100_000.0)
                                            .suffix(" mm"),
                                    )
                                    .changed();
                            }
                            Constraint::LineThroughCenter { line, arc } => {
                                constraints_changed |= sketch_segment_pair_selector(
                                    ui,
                                    (feature.id, index, "line_through_center"),
                                    line,
                                    arc,
                                    &constraint_options.line_arc_pairs,
                                );
                            }
                            Constraint::PointOnCurve { point, segment } => {
                                constraints_changed |= sketch_point_segment_pair_selector(
                                    ui,
                                    (feature.id, index, "point_on_curve"),
                                    point,
                                    segment,
                                    &constraint_options.point_curve_pairs,
                                );
                            }
                            Constraint::Midpoint { point, segment } => {
                                constraints_changed |= sketch_point_segment_pair_selector(
                                    ui,
                                    (feature.id, index, "midpoint"),
                                    point,
                                    segment,
                                    &constraint_options.midpoint_pairs,
                                );
                            }
                            Constraint::Symmetric {
                                first,
                                second,
                                axis,
                            } => {
                                constraints_changed |= sketch_point_pair_selector(
                                    ui,
                                    (feature.id, index, "symmetric_points"),
                                    first,
                                    second,
                                    &constraint_options.point_pairs,
                                );
                                constraints_changed |= sketch_segment_selector(
                                    ui,
                                    (feature.id, index, "symmetric_axis"),
                                    axis,
                                    &constraint_options.line_segments,
                                );
                            }
                            Constraint::Length { segment, length } => {
                                constraints_changed |= sketch_segment_selector(
                                    ui,
                                    (feature.id, index, "length"),
                                    segment,
                                    &constraint_options.line_segments,
                                );
                                constraints_changed |= ui
                                    .add(
                                        egui::DragValue::new(length)
                                            .range(0.001..=100_000.0)
                                            .suffix(" mm"),
                                    )
                                    .changed();
                            }
                            Constraint::EqualLength { first, second }
                            | Constraint::Parallel { first, second }
                            | Constraint::Perpendicular { first, second } => {
                                constraints_changed |= sketch_segment_pair_selector(
                                    ui,
                                    (feature.id, index, "line_pair"),
                                    first,
                                    second,
                                    &constraint_options.line_pairs,
                                );
                            }
                            Constraint::Angle {
                                first,
                                second,
                                angle_degrees,
                            } => {
                                constraints_changed |= sketch_segment_pair_selector(
                                    ui,
                                    (feature.id, index, "angle_pair"),
                                    first,
                                    second,
                                    &constraint_options.angle_pairs,
                                );
                                constraints_changed |= ui
                                    .add(
                                        egui::DragValue::new(angle_degrees)
                                            .range(-180.0..=180.0)
                                            .suffix("°"),
                                    )
                                    .changed();
                            }
                            Constraint::Radius { segment, radius } => {
                                constraints_changed |= sketch_segment_selector(
                                    ui,
                                    (feature.id, index, "radius"),
                                    segment,
                                    &constraint_options.arc_segments,
                                );
                                constraints_changed |= ui
                                    .add(
                                        egui::DragValue::new(radius)
                                            .range(0.001..=100_000.0)
                                            .suffix(" mm"),
                                    )
                                    .changed();
                            }
                            Constraint::FixedCenter { segment, x, y } => {
                                constraints_changed |= sketch_segment_selector(
                                    ui,
                                    (feature.id, index, "center"),
                                    segment,
                                    &constraint_options.arc_segments,
                                );
                                constraints_changed |=
                                    ui.add(egui::DragValue::new(x).suffix(" mm")).changed();
                                constraints_changed |=
                                    ui.add(egui::DragValue::new(y).suffix(" mm")).changed();
                            }
                            Constraint::EqualRadius { first, second }
                            | Constraint::Concentric { first, second } => {
                                constraints_changed |= sketch_segment_pair_selector(
                                    ui,
                                    (feature.id, index, "arc_pair"),
                                    first,
                                    second,
                                    &constraint_options.arc_pairs,
                                );
                            }
                            Constraint::Tangent { first, second } => {
                                constraints_changed |= sketch_segment_pair_selector(
                                    ui,
                                    (feature.id, index, "tangent"),
                                    first,
                                    second,
                                    &constraint_options.tangent_pairs,
                                );
                            }
                            Constraint::CurvatureContinuous { first, second } => {
                                constraints_changed |= sketch_segment_pair_selector(
                                    ui,
                                    (feature.id, index, "curvature_continuous"),
                                    first,
                                    second,
                                    &constraint_options.curvature_pairs,
                                );
                            }
                        });
                    });
                    ui.add_space(2.0);
                }
                if let Some(index) = remove_constraint {
                    constraints.remove(index);
                    constraints_changed = true;
                }
                constraints_changed |= constraints != original_constraints;
                constraints_changed |= sketch_constraint_menu(
                    ui,
                    &mut constraints,
                    &region.profile,
                    &constraint_options,
                    translator,
                );
                if region_changed || construction_changed || constraints_changed {
                    let _ = self.execute(
                        vec![ModelCommand::SetSketchDefinition {
                            id: feature.id,
                            region,
                            construction,
                            constraints,
                        }],
                        StatusMessage::Key("status.updated_sketch_definition"),
                    );
                }
            }
            Primitive::DatumPlane { face, offset } => {
                property_label(ui, translator.text("property.face_selection"), None);
                ui.label(
                    egui::RichText::new(face.to_string())
                        .monospace()
                        .size(9.0)
                        .color(appearance::ACCENT),
                );
                property_label(
                    ui,
                    translator.text("property.offset"),
                    Some(translator.text("property.units_mm")),
                );
                let mut offset = *offset;
                if ui
                    .add(
                        egui::DragValue::new(&mut offset)
                            .speed(0.25)
                            .range(-100_000.0..=100_000.0)
                            .suffix(" mm"),
                    )
                    .changed()
                {
                    let _ = self.execute(
                        vec![ModelCommand::SetDatumPlaneOffset {
                            id: feature.id,
                            offset,
                        }],
                        StatusMessage::Key("status.moved_datum"),
                    );
                }
            }
            Primitive::DatumPoint { vertex, offset } => {
                property_label(ui, translator.text("property.vertex_selection"), None);
                ui.label(
                    egui::RichText::new(vertex.to_string())
                        .monospace()
                        .size(9.0)
                        .color(appearance::ACCENT),
                );
                property_label(
                    ui,
                    translator.text("property.offset"),
                    Some(translator.text("property.units_mm")),
                );
                let mut offset = offset.as_array();
                if vec3_editor(ui, &mut offset, -100_000.0..=100_000.0) {
                    let _ = self.execute(
                        vec![ModelCommand::SetDatumPointOffset {
                            id: feature.id,
                            offset,
                        }],
                        StatusMessage::Key("status.moved_datum_point"),
                    );
                }
            }
            Primitive::Boolean { .. } => {
                property_label(ui, translator.text("property.operation"), None);
                ui.label(
                    egui::RichText::new(feature.primitive.label())
                        .size(11.0)
                        .color(appearance::TEXT),
                );
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(translator.text("boolean.immutable_operands"))
                        .size(10.0)
                        .color(appearance::TEXT_MUTED),
                );
            }
            Primitive::LoftFromSketches {
                sketch_ids,
                profiles,
            } => {
                property_label(ui, translator.text("loft.mode"), None);
                ui.label(
                    egui::RichText::new(translator.text("loft.ruled"))
                        .size(11.0)
                        .color(appearance::TEXT),
                );
                let section_count = sketch_ids.len().to_string();
                let segment_count = profiles
                    .first()
                    .map_or(0, |profile| profile.segments.len())
                    .to_string();
                ui.label(
                    egui::RichText::new(
                        translator.format("loft.section_count", &[("count", &section_count)]),
                    )
                    .size(10.0)
                    .color(appearance::TEXT_MUTED),
                );
                ui.label(
                    egui::RichText::new(
                        translator.format("loft.segment_count", &[("count", &segment_count)]),
                    )
                    .size(10.0)
                    .color(appearance::TEXT_MUTED),
                );
            }
            Primitive::Chamfer { distance, .. } => {
                property_label(
                    ui,
                    translator.text("property.chamfer_distance"),
                    Some(translator.text("property.units_mm")),
                );
                let mut distance = *distance;
                if ui
                    .add(
                        egui::DragValue::new(&mut distance)
                            .speed(0.1)
                            .range(0.1..=100_000.0)
                            .suffix(" mm"),
                    )
                    .changed()
                {
                    let _ = self.execute(
                        vec![ModelCommand::SetChamferDistance {
                            id: feature.id,
                            distance,
                        }],
                        StatusMessage::Key("status.resized_chamfer"),
                    );
                }
            }
            Primitive::Fillet { radius, .. } => {
                property_label(
                    ui,
                    translator.text("property.fillet_radius"),
                    Some(translator.text("property.units_mm")),
                );
                let mut radius = *radius;
                if ui
                    .add(
                        egui::DragValue::new(&mut radius)
                            .speed(0.1)
                            .range(0.1..=100_000.0)
                            .suffix(" mm"),
                    )
                    .changed()
                {
                    let _ = self.execute(
                        vec![ModelCommand::SetFilletRadius {
                            id: feature.id,
                            radius,
                        }],
                        StatusMessage::Key("status.resized_fillet"),
                    );
                }
            }
            Primitive::ImportedStep {
                source,
                data_section,
                shell_id,
                void_shells,
                length_unit,
            } => {
                property_label(ui, translator.text("property.data_section"), None);
                ui.label(
                    egui::RichText::new(format!("{}", data_section.saturating_add(1)))
                        .monospace()
                        .size(10.0)
                        .color(appearance::TEXT_MUTED),
                );
                property_label(ui, translator.text("property.shell_id"), None);
                ui.label(
                    egui::RichText::new(format!("#{shell_id}"))
                        .monospace()
                        .size(10.0)
                        .color(appearance::ACCENT),
                );
                property_label(ui, translator.text("property.void_shells"), None);
                let void_shells = if void_shells.is_empty() {
                    translator.text("property.none").to_owned()
                } else {
                    void_shells
                        .iter()
                        .map(|boundary| {
                            format!(
                                "#{} {}",
                                boundary.shell_id,
                                if boundary.orientation { ".T." } else { ".F." }
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                ui.label(
                    egui::RichText::new(void_shells)
                        .monospace()
                        .size(10.0)
                        .color(appearance::TEXT_MUTED),
                );
                property_label(ui, translator.text("property.source_unit"), None);
                let assumed = if length_unit.declared {
                    ""
                } else {
                    translator.text("property.unit_assumed")
                };
                ui.label(
                    egui::RichText::new(format!(
                        "{} ({} mm/unit){assumed}",
                        length_unit.name, length_unit.millimeters_per_unit
                    ))
                    .monospace()
                    .size(10.0)
                    .color(appearance::TEXT_MUTED),
                );
                property_label(ui, translator.text("property.source_size"), None);
                ui.label(
                    egui::RichText::new(format!("{} bytes", source.len()))
                        .monospace()
                        .size(10.0)
                        .color(appearance::TEXT_MUTED),
                );
            }
        }
    }

    fn ai_panel(&mut self, root: &mut egui::Ui) {
        if !self.ai_panel_open {
            return;
        }
        let context = root.ctx().clone();
        let translator = self.translator.clone();
        egui::Panel::right("ai_panel")
            .default_size(320.0)
            .size_range(280.0..=460.0)
            .resizable(true)
            .frame(appearance::panel_frame(appearance::SURFACE))
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.label(appearance::icon("sparkles", 14.0).color(appearance::WARNING));
                    ui.label(
                        egui::RichText::new(translator.text("panel.ai"))
                            .size(12.0)
                            .strong(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(translator.text("panel.ai_ready"))
                                .size(9.0)
                                .color(appearance::ACCENT),
                        );
                        ui.label(appearance::icon("circle", 7.0).color(appearance::ACCENT));
                    });
                });
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(self.assistant.model_name())
                        .monospace()
                        .size(10.0)
                        .color(appearance::TEXT_FAINT),
                );
                ui.add_space(5.0);
                ui.separator();
                self.domain_tools_panel(ui, &translator);
                ui.separator();

                let composer_height = 126.0;
                let transcript_height = (ui.available_height() - composer_height).max(90.0);
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), transcript_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("ai_conversation")
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                for entry in &self.conversation {
                                    conversation_entry(ui, entry, &translator);
                                    ui.add_space(7.0);
                                }
                                self.domain_intent_review(ui, &translator);
                                self.ai_plan_review(ui, &translator);
                                if self.ai_pending {
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.label(
                                            egui::RichText::new(translator.text("ai.planning"))
                                                .size(11.0)
                                                .color(appearance::TEXT_MUTED),
                                        );
                                    });
                                }
                            });
                    },
                );

                ui.separator();
                ui.add_space(5.0);
                let editor = egui::TextEdit::multiline(&mut self.ai_input)
                    .desired_rows(3)
                    .hint_text(translator.text("ai.placeholder"));
                let response = ui.add_sized([ui.available_width(), 68.0], editor);
                let shortcut = response.has_focus()
                    && ui.input(|input| {
                        input.modifiers.command && input.key_pressed(egui::Key::Enter)
                    });
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(translator.text("property.units_mm"))
                            .monospace()
                            .size(10.0)
                            .color(appearance::TEXT_FAINT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let can_send = !self.ai_pending
                            && self.pending_ai_plan.is_none()
                            && self
                                .last_intent_diff
                                .as_ref()
                                .is_none_or(|intent| intent.accepted)
                            && !self.ai_input.trim().is_empty();
                        let send = tool_button(
                            ui,
                            "send",
                            translator.text("ai.generate"),
                            translator.text("ai.generate"),
                            can_send,
                        )
                        .clicked();
                        if send || (shortcut && can_send) {
                            self.request_ai_plan(&context);
                        }
                    });
                });
            });
    }

    fn domain_intent_review(&mut self, ui: &mut egui::Ui, translator: &Translator) {
        let Some(intent) = self
            .last_intent_diff
            .as_ref()
            .filter(|intent| !intent.accepted)
        else {
            return;
        };
        appearance::section_label(ui, translator.text("ai.intent_review"));
        ui.label(
            egui::RichText::new(intent.summary.as_str())
                .size(11.0)
                .color(appearance::TEXT),
        );
        ui.label(
            egui::RichText::new(format!(
                "{} · {}",
                intent.domain.slug(),
                intent.actions.len()
            ))
            .monospace()
            .size(9.0)
            .color(appearance::TEXT_MUTED),
        );
        let mut apply = false;
        let mut discard = false;
        ui.horizontal(|ui| {
            apply = tool_button(
                ui,
                "check",
                translator.text("ai.approve"),
                translator.text("ai.approve"),
                true,
            )
            .clicked();
            discard = tool_button(
                ui,
                "x",
                translator.text("ai.reject"),
                translator.text("ai.reject"),
                true,
            )
            .clicked();
        });
        if apply {
            self.approve_domain_intent();
        } else if discard {
            self.last_intent_diff = None;
            self.conversation.push(ConversationEntry {
                speaker: Speaker::Assistant,
                content: LocalizedText::Key("ai.plan_discarded"),
            });
        }
        ui.add_space(8.0);
    }

    fn approve_domain_intent(&mut self) {
        let Some(mut intent) = self.last_intent_diff.take() else {
            return;
        };
        let actions = intent.actions.clone();
        for action in actions {
            self.dispatch_domain_route(DomainRoute {
                action,
                confidence: 1.0,
                rationale: intent.summary.clone(),
            });
        }
        intent.accept();
        self.last_intent_diff = Some(intent);
        self.conversation.push(ConversationEntry {
            speaker: Speaker::Assistant,
            content: LocalizedText::Key("ai.intent_applied"),
        });
    }

    fn domain_tools_panel(&mut self, ui: &mut egui::Ui, translator: &Translator) {
        let Some(pack) = self
            .domain_bus
            .enabled_packs()
            .into_iter()
            .find(|pack| pack.manifest().id == self.active_domain)
        else {
            return;
        };
        let tools = pack.tools().to_vec();
        ui.horizontal(|ui| {
            ui.label(appearance::icon("layers", 13.0).color(appearance::ACCENT));
            ui.label(
                egui::RichText::new(translator.text("domain.tools"))
                    .size(10.0)
                    .strong(),
            );
            ui.label(
                egui::RichText::new(domain_description(self.active_domain, translator))
                    .size(9.0)
                    .color(appearance::TEXT_MUTED),
            );
            ui.label(
                egui::RichText::new(format!(
                    "{} {}",
                    translator.text("domain.tools_count"),
                    tools.len()
                ))
                .size(9.0)
                .color(appearance::TEXT_FAINT),
            );
        });
        ui.add_space(4.0);
        let mut categories = Vec::new();
        for tool in &tools {
            if !categories.contains(&tool.category) {
                categories.push(tool.category);
            }
        }
        for category in categories {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(domain_category_label(category, translator))
                        .size(9.0)
                        .strong()
                        .color(appearance::TEXT_FAINT),
                );
                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    for tool in tools.iter().filter(|tool| tool.category == category) {
                        let label = domain_tool_label(tool.id, self.active_domain, translator);
                        if tool_button(ui, tool.icon, label, label, true).clicked() {
                            self.open_domain_tool(tool.id);
                        }
                    }
                });
            });
        }
        if let Some((domain, tool_id)) = self.domain_tool_form.clone()
            && domain == self.active_domain
            && let Some(panel) = pack.tool_panel(&tool_id)
        {
            self.domain_schema_form(ui, translator, &tool_id, panel);
        }
    }

    fn open_domain_tool(&mut self, tool_id: &str) {
        let has_panel = self
            .domain_bus
            .enabled_packs()
            .into_iter()
            .find(|pack| pack.manifest().id == self.active_domain)
            .and_then(|pack| pack.tool_panel(tool_id))
            .is_some();
        if has_panel {
            self.domain_tool_form = Some((self.active_domain, tool_id.to_string()));
        } else {
            self.run_domain_tool(tool_id);
        }
    }

    fn run_domain_tool(&mut self, tool_id: &str) {
        self.run_domain_tool_with_parameters(tool_id, DomainParameters::new());
    }

    fn run_domain_tool_with_parameters(&mut self, tool_id: &str, parameters: DomainParameters) {
        let request =
            DomainToolRequest::new(tool_id, self.domain_context()).with_parameters(parameters);
        match self.domain_bus.execute(self.active_domain, &request) {
            Ok(execution) => self.dispatch_domain_execution(execution),
            Err(error) => self.status = StatusMessage::Text(error.to_string()),
        }
    }

    fn domain_schema_form(
        &mut self,
        ui: &mut egui::Ui,
        translator: &Translator,
        tool_id: &str,
        panel: cadx_domain_api::DomainPanelSchema,
    ) {
        ui.add_space(4.0);
        ui.separator();
        let key = (self.active_domain, panel.id.to_string());
        let values = self
            .domain_form_values
            .entry(key.clone())
            .or_insert_with(|| {
                panel
                    .resolve_parameters(&DomainParameters::new())
                    .unwrap_or_default()
            });
        let mut close_form = false;
        ui.horizontal(|ui| {
            ui.label(appearance::icon("list-tree", 12.0).color(appearance::ACCENT));
            ui.label(egui::RichText::new(panel.label).size(10.0).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, "x", "Close", true, false).clicked() {
                    close_form = true;
                }
            });
        });
        let active_domain = self.active_domain;
        egui::Grid::new(("domain_schema", active_domain, panel.id))
            .num_columns(2)
            .spacing([8.0, 5.0])
            .show(ui, |ui| {
                for field in panel.fields {
                    ui.label(
                        egui::RichText::new(field.label)
                            .size(9.0)
                            .color(appearance::TEXT_MUTED),
                    );
                    domain_field_editor(ui, field, values);
                    ui.end_row();
                }
            });
        ui.add_space(3.0);
        if tool_button(
            ui,
            "play",
            translator.text("tool.apply"),
            translator.text("tool.apply"),
            true,
        )
        .clicked()
        {
            let parameters = self
                .domain_form_values
                .get(&key)
                .cloned()
                .unwrap_or_default();
            self.run_domain_tool_with_parameters(tool_id, parameters);
        }
        if close_form {
            self.domain_tool_form = None;
        }
    }

    fn open_domain_panel(&mut self, tool_id: &str) {
        match (self.active_domain, tool_id) {
            (DomainId::Mcad, "feature-tree") => {
                self.model_panel_open = true;
                self.status = StatusMessage::Text("MCAD feature tree".into());
            }
            (DomainId::Mcad, "sketch") => {
                self.toolbar_tab = ToolbarTab::Create;
                self.status = StatusMessage::Key("tool.sketch");
            }
            (DomainId::Mcad, "extrude") => {
                self.toolbar_tab = ToolbarTab::Create;
                self.status = StatusMessage::Key("tool.extrusion");
            }
            (DomainId::Mcad, "edge-modifiers") => {
                self.toolbar_tab = ToolbarTab::Design;
                self.status = StatusMessage::Text("MCAD chamfer / fillet".into());
            }
            (DomainId::Mcad, "drawing" | "standards-check") => {
                self.run_mechanical_drawing_check();
            }
            (DomainId::Mcad, "dfm") => self.run_mechanical_dfm(),
            (DomainId::Mcad, "bom") => self.run_mechanical_bom(),
            (DomainId::Mcad, "interference") | (DomainId::Aec, "clash") => {
                self.run_interference_analysis();
            }
            (DomainId::Mcad, "ai-part") => {
                self.ai_input = "Create a mechanical bracket".into();
            }
            (DomainId::Aec, "wall" | "slab" | "opening" | "levels" | "bim-attrs") => {
                self.status = StatusMessage::Text(format!("AEC {tool_id}"));
            }
            (DomainId::Aec, "space" | "quantity-takeoff") => {
                self.status = StatusMessage::Text(format!(
                    "AEC {}: {}",
                    tool_id,
                    self.session.document().features.len()
                ));
            }
            (DomainId::Aec, "ifc") => {
                self.status = StatusMessage::Text("IFC export preview".into());
            }
            (DomainId::Aec, "schedule") => {
                self.status = StatusMessage::Text(format!(
                    "AEC schedule: {}",
                    self.session.document().features.len()
                ));
            }
            (DomainId::Ecad, "board") => {
                self.status = StatusMessage::Text("ECAD board".into());
            }
            (
                DomainId::Ecad,
                "schematic" | "netlist" | "footprint-library" | "placement" | "3d-link",
            ) => {
                self.status = StatusMessage::Text(format!("ECAD {tool_id}"));
            }
            (DomainId::Ecad, "drc") => {
                self.domain_report = Some(DomainReport::PcbDrc(pcb_drc::run(&self.pcb_board)));
            }
            (DomainId::Ecad, "bom") => {
                self.domain_report = Some(DomainReport::PcbBom(self.pcb_board.bom()));
            }
            (DomainId::Ecad, "gerber") => {
                self.domain_report = Some(DomainReport::ExportPreview(pcb_export::gerber_bundle(
                    &self.pcb_board,
                )));
            }
            (DomainId::Ecad, "routing" | "stackup" | "diff-pair" | "via") => {
                self.status = StatusMessage::Key("pcb.status.routing_ready");
            }
            _ => {}
        }
    }

    fn domain_report_window(&mut self, context: &egui::Context) {
        let Some(report) = self.domain_report.take() else {
            return;
        };
        let translator = self.translator.clone();
        let mut open = true;
        egui::Window::new(translator.text("domain.report"))
            .open(&mut open)
            .default_width(540.0)
            .resizable(true)
            .show(context, |ui| match &report {
                DomainReport::MechanicalDfm(report) => {
                    ui.label(translator.text("mechanical.report.dfm"));
                    ui.label(format!(
                        "{}: {}",
                        translator.text("domain.checked"),
                        report.checked_parts
                    ));
                    for issue in &report.issues {
                        ui.label(format!(
                            "[{:?}] {} · {}",
                            issue.severity, issue.code, issue.message
                        ));
                    }
                }
                DomainReport::MechanicalDrawing(issues) => {
                    ui.label(translator.text("mechanical.report.drawing"));
                    if issues.is_empty() {
                        ui.colored_label(appearance::ACCENT, translator.text("domain.no_issues"));
                    }
                    for issue in issues {
                        ui.label(format!(
                            "[{:?}] {} · {}",
                            issue.severity, issue.code, issue.message
                        ));
                    }
                }
                DomainReport::MechanicalBom(items) => {
                    ui.label(translator.text("mechanical.report.bom"));
                    for item in items {
                        ui.label(format!(
                            "{}  x{}  {}{}",
                            item.part_number,
                            item.quantity,
                            item.description,
                            item.material
                                .as_deref()
                                .map_or(String::new(), |material| format!(" · {material}"))
                        ));
                    }
                }
                DomainReport::PcbDrc(report) => {
                    ui.label(translator.text("pcb.report.drc"));
                    ui.label(format!(
                        "{}: {} · {}: {}",
                        translator.text("pcb.report.components"),
                        report.checked_components,
                        translator.text("pcb.report.traces"),
                        report.checked_traces
                    ));
                    if report.issues.is_empty() {
                        ui.colored_label(appearance::ACCENT, translator.text("domain.no_issues"));
                    }
                    for issue in &report.issues {
                        ui.label(format!(
                            "[{:?}] {} · {}",
                            issue.severity, issue.code, issue.message
                        ));
                    }
                }
                DomainReport::PcbBom(items) => {
                    ui.label(translator.text("pcb.report.bom"));
                    for (reference, value, footprint, quantity) in items {
                        ui.label(format!("{reference}  x{quantity}  {value} · {footprint}"));
                    }
                }
                DomainReport::ExportPreview(files) => {
                    ui.label(translator.text("pcb.report.export"));
                    for file in files {
                        ui.horizontal(|ui| {
                            ui.label(
                                appearance::icon("file-input", 12.0).color(appearance::ACCENT),
                            );
                            ui.label(file.name.as_str());
                            ui.label(
                                egui::RichText::new(format!("{} bytes", file.contents.len()))
                                    .monospace()
                                    .color(appearance::TEXT_MUTED),
                            );
                        });
                    }
                }
                DomainReport::Artifacts(artifacts) => {
                    for artifact in artifacts {
                        ui.horizontal(|ui| {
                            ui.label(
                                appearance::icon("file-output", 12.0).color(appearance::ACCENT),
                            );
                            ui.label(artifact.name.as_str());
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} · {} bytes",
                                    artifact.media_type,
                                    artifact.contents.len()
                                ))
                                .monospace()
                                .color(appearance::TEXT_MUTED),
                            );
                        });
                        egui::ScrollArea::vertical()
                            .id_salt(("domain_artifact", artifact.name.as_str()))
                            .max_height(120.0)
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(&artifact.contents).monospace(),
                                    )
                                    .selectable(true)
                                    .wrap(),
                                );
                            });
                    }
                }
            });
        if open {
            self.domain_report = Some(report);
        }
    }

    fn boolean_dialog(&mut self, context: &egui::Context) {
        let Some(mut state) = self.boolean_dialog.take() else {
            return;
        };
        let initial_selection = (state.operation, state.left, state.right);
        let translator = self.translator.clone();
        let features: Vec<_> = self
            .session
            .document()
            .features
            .iter()
            .filter(|feature| !feature.primitive.is_reference_geometry())
            .map(|feature| (feature.id, feature.name.clone()))
            .collect();
        let mut open = true;
        let mut apply = false;
        egui::Window::new(translator.text("boolean.title"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(320.0)
            .show(context, |ui| {
                ui.label(
                    egui::RichText::new(translator.text("boolean.description"))
                        .size(11.0)
                        .color(appearance::TEXT_MUTED),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    for (operation, label) in [
                        (BooleanOperation::Union, translator.text("boolean.union")),
                        (
                            BooleanOperation::Subtract,
                            translator.text("boolean.subtract"),
                        ),
                        (
                            BooleanOperation::Intersect,
                            translator.text("boolean.intersect"),
                        ),
                    ] {
                        ui.selectable_value(&mut state.operation, operation, label);
                    }
                });
                ui.add_space(8.0);
                for (value, label) in [
                    (&mut state.left, translator.text("boolean.left")),
                    (&mut state.right, translator.text("boolean.right")),
                ] {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(label).size(11.0));
                        let selected_text = value
                            .and_then(|id| {
                                features
                                    .iter()
                                    .find(|(feature_id, _)| *feature_id == id)
                                    .map(|(_, name)| name.as_str())
                            })
                            .unwrap_or(translator.text("boolean.choose"));
                        egui::ComboBox::from_id_salt(label)
                            .selected_text(selected_text)
                            .show_ui(ui, |ui| {
                                for (id, name) in &features {
                                    ui.selectable_value(value, Some(*id), name);
                                }
                            });
                    });
                }
                if initial_selection != (state.operation, state.left, state.right) {
                    state.diagnostic = None;
                    state.error = None;
                }
                if let Some(diagnostic) = &state.diagnostic {
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);
                    boolean_diagnostic_ui(ui, diagnostic, &translator);
                } else if let Some(error) = &state.error {
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(error)
                            .size(10.0)
                            .color(appearance::DANGER),
                    );
                }
                ui.add_space(10.0);
                let valid =
                    state.left.is_some() && state.right.is_some() && state.left != state.right;
                if ui
                    .add_enabled(
                        valid,
                        egui::Button::new((
                            appearance::icon("check", 14.0),
                            egui::RichText::new(translator.text("boolean.apply")).size(11.0),
                        )),
                    )
                    .clicked()
                {
                    apply = true;
                }
            });
        if apply {
            let (Some(left), Some(right)) = (state.left, state.right) else {
                self.boolean_dialog = open.then_some(state);
                return;
            };
            let result = self.execute(
                vec![ModelCommand::CreateBoolean {
                    name: translator.text("primitive.boolean").into(),
                    operation: state.operation,
                    left,
                    right,
                }],
                StatusMessage::Key("status.created_boolean"),
            );
            if let Err(error) = result {
                state.diagnostic = error.boolean_diagnostic().cloned();
                state.error = state.diagnostic.is_none().then(|| error.to_string());
                self.boolean_dialog = Some(state);
            }
        } else {
            self.boolean_dialog = open.then_some(state);
        }
    }

    fn run_interference_analysis(&mut self) {
        let result = self
            .session
            .analyze_interference()
            .map_err(|error| error.to_string());
        self.status = match &result {
            Ok(report) if report.failed_pair_count > 0 => {
                let failed = report.failed_pair_count.to_string();
                StatusMessage::Text(
                    self.translator
                        .format("status.interference_incomplete", &[("failed", &failed)]),
                )
            }
            Ok(report) if report.has_interference() => {
                let count = report.interfering_pair_count.to_string();
                StatusMessage::Text(
                    self.translator
                        .format("status.interference_found", &[("count", &count)]),
                )
            }
            Ok(_) => StatusMessage::Key("status.interference_clear"),
            Err(error) => StatusMessage::Text(error.clone()),
        };
        self.interference_dialog = Some(InterferenceDialogState { result });
    }

    fn interference_dialog(&mut self, context: &egui::Context) {
        let Some(state) = self.interference_dialog.take() else {
            return;
        };
        let translator = self.translator.clone();
        let mut open = true;
        let mut selected = None;
        egui::Window::new(translator.text("interference.title"))
            .open(&mut open)
            .collapsible(false)
            .default_width(460.0)
            .show(context, |ui| match &state.result {
                Err(error) => {
                    ui.label(
                        egui::RichText::new(error)
                            .size(10.0)
                            .color(appearance::DANGER),
                    );
                }
                Ok(report) => {
                    let (summary_key, summary_color) = if report.failed_pair_count > 0 {
                        ("interference.summary_incomplete", appearance::WARNING)
                    } else if report.has_interference() {
                        ("interference.summary_found", appearance::DANGER)
                    } else {
                        ("interference.summary_clear", appearance::ACCENT)
                    };
                    ui.label(
                        egui::RichText::new(translator.text(summary_key))
                            .size(12.0)
                            .strong()
                            .color(summary_color),
                    );
                    ui.add_space(6.0);
                    let candidates = report.candidate_feature_ids.len().to_string();
                    let pairs = report.total_pair_count.to_string();
                    let broad = report.broad_phase_pair_count.to_string();
                    let clear = report.clear_pair_count.to_string();
                    let clashes = report.interfering_pair_count.to_string();
                    let failed = report.failed_pair_count.to_string();
                    ui.label(
                        egui::RichText::new(translator.format(
                            "interference.counts",
                            &[
                                ("candidates", &candidates),
                                ("pairs", &pairs),
                                ("broad", &broad),
                                ("clear", &clear),
                                ("clashes", &clashes),
                                ("failed", &failed),
                            ],
                        ))
                        .monospace()
                        .size(9.0)
                        .color(appearance::TEXT_MUTED),
                    );
                    let volume_tolerance = format!("{:.9}", report.volume_tolerance_mm3);
                    ui.label(
                        egui::RichText::new(translator.format(
                            "interference.volume_tolerance",
                            &[("volume", &volume_tolerance)],
                        ))
                        .monospace()
                        .size(9.0)
                        .color(appearance::TEXT_FAINT),
                    );
                    if report.pairs.is_empty() {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(translator.text("interference.no_broad_pairs"))
                                .size(10.0)
                                .color(appearance::TEXT_MUTED),
                        );
                        return;
                    }
                    ui.add_space(8.0);
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .id_salt("interference_pairs")
                        .max_height(280.0)
                        .show(ui, |ui| {
                            for pair in &report.pairs {
                                let [left_id, right_id] = pair.feature_ids;
                                let feature_label = |id| {
                                    self.session.document().feature(id).map_or_else(
                                        || format!("#{id}"),
                                        |feature| format!("{} #{id}", feature.name),
                                    )
                                };
                                let left = feature_label(left_id);
                                let right = feature_label(right_id);
                                let (icon, color, detail) = match &pair.outcome {
                                    InterferencePairOutcome::Clear { volume_mm3, .. } => (
                                        "circle-check",
                                        appearance::ACCENT,
                                        translator.format(
                                            "interference.pair_clear",
                                            &[("volume", &format!("{volume_mm3:.6}"))],
                                        ),
                                    ),
                                    InterferencePairOutcome::Interfering { volume_mm3, .. } => (
                                        "triangle-alert",
                                        appearance::DANGER,
                                        translator.format(
                                            "interference.pair_found",
                                            &[("volume", &format!("{volume_mm3:.6}"))],
                                        ),
                                    ),
                                    InterferencePairOutcome::Failed {
                                        stage,
                                        reason,
                                        detail,
                                    } => (
                                        "circle-x",
                                        appearance::WARNING,
                                        translator.format(
                                            "interference.pair_failed",
                                            &[
                                                (
                                                    "stage",
                                                    translator.text(interference_stage_key(*stage)),
                                                ),
                                                (
                                                    "reason",
                                                    translator
                                                        .text(interference_reason_key(*reason)),
                                                ),
                                                ("detail", detail),
                                            ],
                                        ),
                                    ),
                                };
                                ui.horizontal(|ui| {
                                    ui.label(appearance::icon(icon, 13.0).color(color));
                                    if ui
                                        .selectable_label(
                                            self.selected == Some(left_id),
                                            egui::RichText::new(format!("{left} / {right}"))
                                                .size(10.0),
                                        )
                                        .clicked()
                                    {
                                        selected = Some(left_id);
                                    }
                                });
                                ui.label(
                                    egui::RichText::new(detail)
                                        .monospace()
                                        .size(9.0)
                                        .color(appearance::TEXT_MUTED),
                                );
                                ui.add_space(5.0);
                            }
                        });
                }
            });
        if let Some(id) = selected {
            self.selected = Some(id);
            self.clear_topology_selection();
            self.sync_viewport();
        }
        self.interference_dialog = open.then_some(state);
    }

    fn loft_dialog(&mut self, context: &egui::Context) {
        let Some(mut state) = self.loft_dialog.take() else {
            return;
        };
        let translator = self.translator.clone();
        let candidates = self
            .session
            .document()
            .features
            .iter()
            .filter_map(|feature| match &feature.primitive {
                Primitive::Sketch { region, .. } if region.holes.is_empty() => Some((
                    feature.id,
                    feature.name.clone(),
                    region.profile.segments.len(),
                    region.profile.signed_area().is_sign_positive(),
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        state
            .sketch_ids
            .retain(|id| candidates.iter().any(|(candidate, ..)| candidate == id));

        let mut open = true;
        let mut apply = false;
        egui::Window::new(translator.text("loft.title"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(400.0)
            .show(context, |ui| {
                ui.label(
                    egui::RichText::new(translator.text("loft.description"))
                        .size(11.0)
                        .color(appearance::TEXT_MUTED),
                );
                ui.add_space(8.0);
                let count = state.sketch_ids.len().to_string();
                property_label(
                    ui,
                    translator.text("loft.sections"),
                    Some(
                        translator
                            .format("loft.section_count", &[("count", &count)])
                            .as_str(),
                    ),
                );
                egui::ScrollArea::vertical()
                    .id_salt("loft_section_candidates")
                    .max_height(220.0)
                    .show(ui, |ui| {
                        for (id, name, segment_count, _) in &candidates {
                            let selected_index =
                                state.sketch_ids.iter().position(|selected| selected == id);
                            let mut included = selected_index.is_some();
                            ui.horizontal(|ui| {
                                let can_include =
                                    included || state.sketch_ids.len() < MAX_LOFT_SECTIONS;
                                let changed = ui
                                    .add_enabled(
                                        can_include,
                                        egui::Checkbox::new(&mut included, ""),
                                    )
                                    .changed();
                                let order = selected_index
                                    .map_or_else(|| "-".into(), |index| (index + 1).to_string());
                                let id_text = id.to_string();
                                let segments = segment_count.to_string();
                                ui.label(
                                    egui::RichText::new(translator.format(
                                        "loft.section_candidate",
                                        &[
                                            ("order", &order),
                                            ("name", name),
                                            ("id", &id_text),
                                            ("count", &segments),
                                        ],
                                    ))
                                    .size(10.0)
                                    .color(appearance::TEXT),
                                );
                                if let Some(index) = selected_index {
                                    if icon_button(
                                        ui,
                                        "arrow-up",
                                        translator.text("loft.move_up"),
                                        index > 0,
                                        false,
                                    )
                                    .clicked()
                                    {
                                        state.sketch_ids.swap(index, index - 1);
                                        state.error = None;
                                    }
                                    if icon_button(
                                        ui,
                                        "arrow-down",
                                        translator.text("loft.move_down"),
                                        index + 1 < state.sketch_ids.len(),
                                        false,
                                    )
                                    .clicked()
                                    {
                                        state.sketch_ids.swap(index, index + 1);
                                        state.error = None;
                                    }
                                }
                                if changed {
                                    if included {
                                        state.sketch_ids.push(*id);
                                    } else {
                                        state.sketch_ids.retain(|selected| selected != id);
                                    }
                                    state.error = None;
                                }
                            });
                        }
                    });

                let compatible = loft_sections_compatible(&state.sketch_ids, &candidates);
                if state.sketch_ids.len() >= 2 && !compatible {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(translator.text("loft.incompatible"))
                            .size(10.0)
                            .color(appearance::DANGER),
                    );
                } else if let Some(error) = &state.error {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(error)
                            .size(10.0)
                            .color(appearance::DANGER),
                    );
                }
                ui.add_space(10.0);
                if ui
                    .add_enabled(
                        (2..=MAX_LOFT_SECTIONS).contains(&state.sketch_ids.len()) && compatible,
                        egui::Button::new((
                            appearance::icon("check", 14.0),
                            egui::RichText::new(translator.text("loft.apply")).size(11.0),
                        )),
                    )
                    .clicked()
                {
                    apply = true;
                }
            });
        if apply {
            let result = self.execute(
                vec![ModelCommand::CreateLoftFromSketches {
                    name: translator.text("primitive.loft").into(),
                    sketch_ids: state.sketch_ids.clone(),
                    position: [0.0; 3],
                }],
                StatusMessage::Key("status.created_loft"),
            );
            if let Err(error) = result {
                state.error = Some(error.to_string());
                self.loft_dialog = Some(state);
            }
        } else {
            self.loft_dialog = open.then_some(state);
        }
    }

    fn edge_modifier_dialog(&mut self, context: &egui::Context) {
        let Some(mut state) = self.edge_modifier_dialog.take() else {
            return;
        };
        let translator = self.translator.clone();
        let (title_key, parameter_key, apply_key) = match state.kind {
            EdgeModifierKind::Chamfer => (
                "chamfer.title",
                "property.chamfer_distance",
                "chamfer.apply",
            ),
            EdgeModifierKind::Fillet => ("fillet.title", "property.fillet_radius", "fillet.apply"),
        };
        let mut open = true;
        let mut apply = false;
        egui::Window::new(translator.text(title_key))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_width(340.0)
            .show(context, |ui| {
                let count = state.edges.len().to_string();
                property_label(
                    ui,
                    translator.text("property.edge_selection"),
                    Some(
                        translator
                            .format("property.edge_count", &[("count", &count)])
                            .as_str(),
                    ),
                );
                egui::ScrollArea::vertical()
                    .max_height(96.0)
                    .show(ui, |ui| {
                        for edge in &state.edges {
                            ui.label(
                                egui::RichText::new(edge.to_string())
                                    .monospace()
                                    .size(9.0)
                                    .color(appearance::ACCENT),
                            );
                        }
                    });
                ui.add_space(8.0);
                property_label(
                    ui,
                    translator.text(parameter_key),
                    Some(translator.text("property.units_mm")),
                );
                if ui
                    .add(
                        egui::DragValue::new(&mut state.size)
                            .speed(0.1)
                            .range(0.1..=100_000.0)
                            .suffix(" mm"),
                    )
                    .changed()
                {
                    state.diagnostic = None;
                    state.error = None;
                }
                if let Some(diagnostic) = &state.diagnostic {
                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(6.0);
                    edge_modifier_diagnostic_ui(ui, diagnostic, &translator);
                } else if let Some(error) = &state.error {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(error)
                            .size(10.0)
                            .color(appearance::DANGER),
                    );
                }
                ui.add_space(10.0);
                if ui
                    .add_enabled(
                        state.size.is_finite() && state.size > 0.0,
                        egui::Button::new((
                            appearance::icon("check", 14.0),
                            egui::RichText::new(translator.text(apply_key)).size(11.0),
                        )),
                    )
                    .clicked()
                {
                    apply = true;
                }
            });
        if apply {
            let (command, status) = match state.kind {
                EdgeModifierKind::Chamfer => (
                    ModelCommand::CreateChamfer {
                        name: translator.text("primitive.chamfer").into(),
                        edges: state.edges.clone(),
                        distance: state.size,
                    },
                    "status.created_chamfer",
                ),
                EdgeModifierKind::Fillet => (
                    ModelCommand::CreateFillet {
                        name: translator.text("primitive.fillet").into(),
                        edges: state.edges.clone(),
                        radius: state.size,
                    },
                    "status.created_fillet",
                ),
            };
            let result = self.execute(vec![command], StatusMessage::Key(status));
            if let Err(error) = result {
                state.diagnostic = error.edge_modifier_diagnostic().cloned();
                state.error = state.diagnostic.is_none().then(|| error.to_string());
                self.edge_modifier_dialog = Some(state);
            }
        } else {
            self.edge_modifier_dialog = open.then_some(state);
        }
    }

    fn ai_plan_review(&mut self, ui: &mut egui::Ui, translator: &Translator) {
        let Some(plan) = self.pending_ai_plan.clone() else {
            return;
        };
        let mut approve = false;
        let mut reject = false;
        egui::Frame::new()
            .fill(appearance::ACCENT_SOFT)
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::same(9))
            .stroke(egui::Stroke::new(1.0, appearance::ACCENT))
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(translator.text("ai.plan_review"))
                        .size(10.0)
                        .strong()
                        .color(appearance::ACCENT),
                );
                ui.add_space(3.0);
                ui.label(egui::RichText::new(plan.summary).size(11.0));
                egui::ScrollArea::vertical()
                    .id_salt("ai_plan_commands")
                    .max_height(82.0)
                    .show(ui, |ui| {
                        for (index, command) in plan.commands.iter().enumerate() {
                            ui.label(
                                egui::RichText::new(format!("{}  {}", index + 1, command.label()))
                                    .size(10.0)
                                    .color(appearance::TEXT_MUTED),
                            );
                        }
                    });
                ui.horizontal(|ui| {
                    approve = tool_button(
                        ui,
                        "check",
                        translator.text("ai.approve"),
                        translator.text("ai.approve"),
                        true,
                    )
                    .clicked();
                    reject = tool_button(
                        ui,
                        "x",
                        translator.text("ai.reject"),
                        translator.text("ai.reject"),
                        true,
                    )
                    .clicked();
                });
            });
        if approve {
            self.approve_ai_plan();
        } else if reject {
            self.reject_ai_plan();
        }
    }

    fn status_bar(&mut self, root: &mut egui::Ui) {
        let translator = self.translator.clone();
        let bodies = self.session.scene().parts.len().to_string();
        let triangles = self.session.scene().triangle_count().to_string();
        let geometry = translator.format(
            "status.geometry",
            &[("bodies", &bodies), ("triangles", &triangles)],
        );
        let analysis = analyze_scene(self.session.scene(), None).ok();
        let volume = analysis.as_ref().map_or_else(
            || "-".into(),
            |value| format!("{:.1}", value.total_volume_mm3),
        );
        let surface = analysis.as_ref().map_or_else(
            || "-".into(),
            |value| format!("{:.1}", value.total_surface_area_mm2),
        );
        let metrics = translator.format(
            "status.analysis",
            &[("volume", &volume), ("surface", &surface)],
        );
        let mass = analysis
            .as_ref()
            .and_then(|value| value.total_mass_kg)
            .map(|value| {
                let value = format!("{value:.6}");
                translator.format("status.mass", &[("mass", &value)])
            });
        let selection = if let Some(vertex) = &self.selected_vertex {
            let id = vertex.feature_id.to_string();
            let fragment = vertex.fragment.to_string();
            translator.format(
                "status.selected_vertex",
                &[("id", &id), ("fragment", &fragment)],
            )
        } else if let Some(edge) = self.selected_edges.last() {
            let id = edge.feature_id.to_string();
            let fragment = edge.fragment.to_string();
            if self.selected_edges.len() == 1 {
                translator.format(
                    "status.selected_edge",
                    &[("id", &id), ("fragment", &fragment)],
                )
            } else {
                let count = self.selected_edges.len().to_string();
                translator.format("status.selected_edges", &[("id", &id), ("count", &count)])
            }
        } else if let Some(face) = &self.selected_face {
            let reference = face.to_string();
            translator.format("status.selected_face", &[("face", &reference)])
        } else {
            self.selected.map_or_else(
                || translator.text("status.no_selection").to_owned(),
                |id| {
                    let id = id.to_string();
                    translator.format("status.selected", &[("id", &id)])
                },
            )
        };
        let kernel = translator.format("status.kernel", &[("name", self.session.kernel_name())]);

        egui::Panel::bottom("status_bar")
            .exact_size(25.0)
            .frame(
                egui::Frame::new()
                    .fill(appearance::SURFACE)
                    .inner_margin(egui::Margin::symmetric(8, 2))
                    .stroke(egui::Stroke::new(1.0, appearance::BORDER_SOFT)),
            )
            .show(root, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(appearance::icon("circle", 7.0).color(appearance::ACCENT));
                    ui.label(
                        egui::RichText::new(self.status.resolve(&translator))
                            .size(10.0)
                            .color(appearance::TEXT),
                    );
                    ui.separator();
                    ui.label(
                        egui::RichText::new(geometry)
                            .size(10.0)
                            .color(appearance::TEXT_MUTED),
                    );
                    if let Some(mass) = &mass {
                        ui.separator();
                        ui.label(
                            egui::RichText::new(mass)
                                .size(10.0)
                                .color(appearance::TEXT_MUTED),
                        );
                    }
                    ui.separator();
                    ui.label(
                        egui::RichText::new(metrics)
                            .size(10.0)
                            .color(appearance::TEXT_MUTED),
                    );
                    ui.separator();
                    ui.label(
                        egui::RichText::new(selection)
                            .size(10.0)
                            .color(appearance::TEXT_MUTED),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(translator.text("status.units"))
                                .size(10.0)
                                .color(appearance::TEXT_MUTED),
                        );
                        ui.separator();
                        ui.label(
                            egui::RichText::new(kernel)
                                .monospace()
                                .size(9.0)
                                .color(appearance::TEXT_FAINT),
                        );
                    });
                });
            });
    }

    fn viewport(&mut self, root: &mut egui::Ui) {
        let context = root.ctx().clone();
        let translator = self.translator.clone();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(appearance::BACKGROUND))
            .show(root, |ui| {
                let rect = ui.available_rect_before_wrap();
                let response = ui.allocate_rect(rect, egui::Sense::click_and_drag());
                let mut annotations = render::layout_sketch_annotations(
                    self.session.scene(),
                    self.selected,
                    rect,
                    self.camera,
                );
                let dimension_hit = response.interact_pointer_pos().and_then(|pointer| {
                    render::pick_sketch_dimension(&annotations, pointer).and_then(|annotation| {
                        Some((
                            annotation.feature_id,
                            annotation.constraint_index,
                            annotation.constraint.dimension()?,
                            annotation.label_rect.right_bottom(),
                        ))
                    })
                });
                if response.dragged_by(egui::PointerButton::Primary) {
                    let delta = ui.input(|input| input.pointer.delta());
                    self.camera.orbit(delta);
                    context.request_repaint();
                }
                if response.dragged_by(egui::PointerButton::Middle) {
                    let delta = ui.input(|input| input.pointer.delta());
                    self.camera.pan(delta);
                    context.request_repaint();
                }
                if response.hovered() {
                    let scroll = ui.input(|input| input.smooth_scroll_delta.y);
                    if scroll.abs() > f32::EPSILON {
                        self.camera.zoom(scroll);
                        context.request_repaint();
                    }
                }
                let selected_before_click = self.selected;
                if response.double_clicked() {
                    if let Some((feature_id, constraint_index, dimension, position)) = dimension_hit
                    {
                        self.sketch_dimension_editor = Some(SketchDimensionEditor {
                            feature_id,
                            constraint_index,
                            kind: dimension.kind,
                            value: dimension.value,
                            position: position + egui::vec2(8.0, 8.0),
                        });
                    } else {
                        self.camera.frame_scene(self.session.scene());
                    }
                } else if response.clicked() {
                    if let Some(pointer) = response.interact_pointer_pos() {
                        let additive = ui.input(|input| input.modifiers.shift);
                        let (topology_feature, measurement_entity) = match self.selection_mode {
                            SelectionMode::Face => {
                                self.clear_topology_selection();
                                self.selected_face = render::pick_face(
                                    self.session.scene(),
                                    rect,
                                    pointer,
                                    self.camera,
                                );
                                (
                                    self.selected_face
                                        .as_ref()
                                        .map(|reference| reference.feature_id),
                                    self.selected_face.clone().map(MeasurementEntity::Face),
                                )
                            }
                            SelectionMode::Edge => {
                                self.selected_face = None;
                                self.selected_vertex = None;
                                let picked = render::pick_edge(
                                    self.session.scene(),
                                    rect,
                                    pointer,
                                    self.camera,
                                );
                                update_edge_selection(
                                    &mut self.selected_edges,
                                    picked.as_ref(),
                                    additive,
                                );
                                (
                                    picked.as_ref().map(|reference| reference.feature_id),
                                    picked.map(MeasurementEntity::Edge),
                                )
                            }
                            SelectionMode::Vertex => {
                                self.clear_topology_selection();
                                self.selected_vertex = render::pick_vertex(
                                    self.session.scene(),
                                    rect,
                                    pointer,
                                    self.camera,
                                );
                                (
                                    self.selected_vertex
                                        .as_ref()
                                        .map(|reference| reference.feature_id),
                                    self.selected_vertex.clone().map(MeasurementEntity::Vertex),
                                )
                            }
                        };
                        if let Some(entity) = measurement_entity
                            && self.measurement.active
                        {
                            self.add_measurement_entity(entity);
                        }
                        self.selected = topology_feature.or_else(|| {
                            render::pick_feature(self.session.scene(), rect, pointer, self.camera)
                        });
                    } else {
                        self.selected = None;
                        self.clear_topology_selection();
                    }
                    self.sync_viewport();
                }

                render::paint_viewport(ui, rect, self.viewport_scene.clone(), self.camera);
                if self.selected != selected_before_click {
                    annotations = render::layout_sketch_annotations(
                        self.session.scene(),
                        self.selected,
                        rect,
                        self.camera,
                    );
                }
                let redundant_constraints = self
                    .selected
                    .and_then(|id| self.session.scene().sketch_diagnostic(id))
                    .map_or(&[][..], |diagnostic| {
                        diagnostic.redundant_constraints.as_slice()
                    });
                let conflict_constraints = (self.last_sketch_failure_feature == self.selected)
                    .then_some(self.last_sketch_failure.as_ref())
                    .flatten()
                    .map_or(&[][..], |diagnostic| {
                        diagnostic.constraint_indices.as_slice()
                    });
                let editing_constraint = self
                    .sketch_dimension_editor
                    .as_ref()
                    .filter(|editor| Some(editor.feature_id) == self.selected)
                    .map(|editor| editor.constraint_index);
                render::paint_sketch_annotations(
                    ui,
                    &annotations,
                    redundant_constraints,
                    conflict_constraints,
                    editing_constraint,
                );
                ui.painter().text(
                    rect.left_top() + egui::vec2(14.0, 12.0),
                    egui::Align2::LEFT_TOP,
                    translator.text("viewport.perspective"),
                    egui::FontId::proportional(10.0),
                    appearance::TEXT_MUTED,
                );
            });
    }

    fn sketch_dimension_dialog(&mut self, context: &egui::Context) {
        let Some(mut editor) = self.sketch_dimension_editor.take() else {
            return;
        };
        if self.selected != Some(editor.feature_id) {
            return;
        }
        let current_dimension =
            self.session
                .document()
                .feature(editor.feature_id)
                .and_then(|feature| match &feature.primitive {
                    Primitive::Sketch { constraints, .. } => constraints
                        .get(usize::try_from(editor.constraint_index).ok()?)
                        .and_then(Constraint::dimension),
                    _ => None,
                });
        if current_dimension.is_none_or(|dimension| dimension.kind != editor.kind) {
            return;
        }

        let translator = self.translator.clone();
        let mut apply = false;
        let mut cancel = false;
        egui::Window::new(translator.text("sketch.dimension_edit"))
            .id(egui::Id::new((
                "sketch_dimension_editor",
                editor.feature_id,
                editor.constraint_index,
            )))
            .default_pos(editor.position)
            .collapsible(false)
            .resizable(false)
            .show(context, |ui| {
                ui.label(
                    egui::RichText::new(translator.text(sketch_dimension_kind_key(editor.kind)))
                        .size(10.0)
                        .color(appearance::TEXT_MUTED),
                );
                let value_response = match editor.kind {
                    SketchDimensionKind::HorizontalDistance
                    | SketchDimensionKind::VerticalDistance => {
                        ui.add(egui::DragValue::new(&mut editor.value).suffix(" mm"))
                    }
                    SketchDimensionKind::Distance | SketchDimensionKind::PointLineDistance => ui
                        .add(
                            egui::DragValue::new(&mut editor.value)
                                .range(0.0..=100_000.0)
                                .suffix(" mm"),
                        ),
                    SketchDimensionKind::Length | SketchDimensionKind::Radius => ui.add(
                        egui::DragValue::new(&mut editor.value)
                            .range(0.001..=100_000.0)
                            .suffix(" mm"),
                    ),
                    SketchDimensionKind::Angle => ui.add(
                        egui::DragValue::new(&mut editor.value)
                            .range(-180.0..=180.0)
                            .suffix("°"),
                    ),
                };
                if value_response.lost_focus()
                    && ui.input(|input| input.key_pressed(egui::Key::Enter))
                {
                    apply = true;
                }
                ui.horizontal(|ui| {
                    cancel =
                        icon_button(ui, "x", translator.text("tool.cancel"), true, false).clicked();
                    apply |= icon_button(
                        ui,
                        "check",
                        translator.text("tool.apply"),
                        editor.kind.accepts(editor.value),
                        false,
                    )
                    .clicked();
                });
            });
        if cancel {
            return;
        }
        if apply && editor.kind.accepts(editor.value) {
            if !self.apply_sketch_dimension(&editor) {
                self.sketch_dimension_editor = Some(editor);
            }
        } else {
            self.sketch_dimension_editor = Some(editor);
        }
    }

    fn apply_sketch_dimension(&mut self, editor: &SketchDimensionEditor) -> bool {
        let Some(feature) = self.session.document().feature(editor.feature_id) else {
            return false;
        };
        let Primitive::Sketch {
            region,
            construction,
            constraints,
            ..
        } = &feature.primitive
        else {
            return false;
        };
        let mut constraints = constraints.clone();
        if !replace_sketch_dimension(
            &mut constraints,
            editor.constraint_index,
            editor.kind,
            editor.value,
        ) {
            return false;
        }
        self.execute(
            vec![ModelCommand::SetSketchDefinition {
                id: editor.feature_id,
                region: region.clone(),
                construction: construction.clone(),
                constraints,
            }],
            StatusMessage::Key("status.updated_sketch_dimension"),
        )
        .is_ok()
    }
}

fn boolean_diagnostic_ui(
    ui: &mut egui::Ui,
    diagnostic: &BooleanDiagnostic,
    translator: &Translator,
) {
    ui.horizontal(|ui| {
        ui.label(appearance::icon("triangle-alert", 14.0).color(appearance::DANGER));
        ui.label(
            egui::RichText::new(translator.text(boolean_reason_key(diagnostic.reason)))
                .size(11.0)
                .strong()
                .color(appearance::DANGER),
        );
    });
    let stage = translator.text(boolean_stage_key(diagnostic.stage));
    let tolerance = format!("{:.6}", diagnostic.tolerance_mm);
    ui.label(
        egui::RichText::new(translator.format(
            "boolean.diagnostic.stage_tolerance",
            &[("stage", stage), ("tolerance", &tolerance)],
        ))
        .monospace()
        .size(9.0)
        .color(appearance::TEXT_MUTED),
    );
    if !diagnostic.attempts.is_empty() {
        let count = diagnostic.attempts.len().to_string();
        ui.label(
            egui::RichText::new(
                translator.format("boolean.diagnostic.attempts", &[("count", &count)]),
            )
            .monospace()
            .size(9.0)
            .color(appearance::TEXT_MUTED),
        );
    }
    if let Some([x, y, z]) = diagnostic.operand_separation_mm() {
        let [x, y, z] = [x, y, z].map(|value| format!("{value:.6}"));
        ui.label(
            egui::RichText::new(translator.format(
                "boolean.diagnostic.separation",
                &[("x", &x), ("y", &y), ("z", &z)],
            ))
            .monospace()
            .size(9.0)
            .color(appearance::TEXT_MUTED),
        );
    }
    ui.collapsing(translator.text("boolean.diagnostic.technical"), |ui| {
        ui.label(
            egui::RichText::new(&diagnostic.detail)
                .monospace()
                .size(9.0)
                .color(appearance::TEXT_FAINT),
        );
    });
}

fn edge_modifier_diagnostic_ui(
    ui: &mut egui::Ui,
    diagnostic: &EdgeModifierDiagnostic,
    translator: &Translator,
) {
    ui.horizontal(|ui| {
        ui.label(appearance::icon("triangle-alert", 14.0).color(appearance::DANGER));
        ui.label(
            egui::RichText::new(translator.text(edge_modifier_reason_key(diagnostic.reason)))
                .size(11.0)
                .strong()
                .color(appearance::DANGER),
        );
    });
    let stage = translator.text(edge_modifier_stage_key(diagnostic.stage));
    let tolerance = format!("{:.6}", diagnostic.tolerance_mm);
    ui.label(
        egui::RichText::new(translator.format(
            "edge_modifier.diagnostic.stage_tolerance",
            &[("stage", stage), ("tolerance", &tolerance)],
        ))
        .monospace()
        .size(9.0)
        .color(appearance::TEXT_MUTED),
    );
    let parameter = translator.text(edge_modifier_parameter_key(diagnostic.parameter));
    let value = format!("{:.6}", diagnostic.parameter_value_mm);
    let source = diagnostic
        .source_feature_id
        .map_or_else(|| "-".into(), |id| id.to_string());
    ui.label(
        egui::RichText::new(translator.format(
            "edge_modifier.diagnostic.parameter_source",
            &[
                ("parameter", parameter),
                ("value", &value),
                ("source", &source),
            ],
        ))
        .monospace()
        .size(9.0)
        .color(appearance::TEXT_MUTED),
    );
    if let Some(indices) = &diagnostic.offending_edge_indices {
        let indices = indices
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        ui.label(
            egui::RichText::new(translator.format(
                "edge_modifier.diagnostic.offending_edges",
                &[("indices", &indices)],
            ))
            .monospace()
            .size(9.0)
            .color(appearance::TEXT_MUTED),
        );
    }
    ui.collapsing(
        translator.text("edge_modifier.diagnostic.technical"),
        |ui| {
            ui.label(
                egui::RichText::new(&diagnostic.detail)
                    .monospace()
                    .size(9.0)
                    .color(appearance::TEXT_FAINT),
            );
        },
    );
}

const fn edge_modifier_reason_key(reason: EdgeModifierFailureReason) -> &'static str {
    match reason {
        EdgeModifierFailureReason::EmptyEdgeSet => "edge_modifier.diagnostic.empty_edge_set",
        EdgeModifierFailureReason::MixedSourceFeatures => {
            "edge_modifier.diagnostic.mixed_source_features"
        }
        EdgeModifierFailureReason::LostReference => "edge_modifier.diagnostic.lost_reference",
        EdgeModifierFailureReason::AmbiguousReference => {
            "edge_modifier.diagnostic.ambiguous_reference"
        }
        EdgeModifierFailureReason::NonLinearEdge => "edge_modifier.diagnostic.non_linear_edge",
        EdgeModifierFailureReason::NonPlanarSupport => {
            "edge_modifier.diagnostic.non_planar_support"
        }
        EdgeModifierFailureReason::NonConvexEdge => "edge_modifier.diagnostic.non_convex_edge",
        EdgeModifierFailureReason::SharedVertexUnsupported => {
            "edge_modifier.diagnostic.shared_vertex_unsupported"
        }
        EdgeModifierFailureReason::NonConvexSource => "edge_modifier.diagnostic.non_convex_source",
        EdgeModifierFailureReason::ParameterBelowTolerance => {
            "edge_modifier.diagnostic.parameter_below_tolerance"
        }
        EdgeModifierFailureReason::ParameterExceedsTopology => {
            "edge_modifier.diagnostic.parameter_exceeds_topology"
        }
        EdgeModifierFailureReason::KernelRejected => "edge_modifier.diagnostic.kernel_rejected",
        EdgeModifierFailureReason::KernelPanic => "edge_modifier.diagnostic.kernel_panic",
        EdgeModifierFailureReason::InvalidResultTopology => {
            "edge_modifier.diagnostic.invalid_result_topology"
        }
        EdgeModifierFailureReason::TopologyNamingFailed => {
            "edge_modifier.diagnostic.topology_naming_failed"
        }
    }
}

const fn edge_modifier_stage_key(stage: EdgeModifierFailureStage) -> &'static str {
    match stage {
        EdgeModifierFailureStage::ReferenceResolution => "edge_modifier.stage.reference_resolution",
        EdgeModifierFailureStage::GeometryValidation => "edge_modifier.stage.geometry_validation",
        EdgeModifierFailureStage::Construction => "edge_modifier.stage.construction",
        EdgeModifierFailureStage::ResultValidation => "edge_modifier.stage.result_validation",
        EdgeModifierFailureStage::TopologyNaming => "edge_modifier.stage.topology_naming",
    }
}

const fn edge_modifier_parameter_key(parameter: EdgeModifierParameter) -> &'static str {
    match parameter {
        EdgeModifierParameter::Distance => "edge_modifier.parameter.distance",
        EdgeModifierParameter::Radius => "edge_modifier.parameter.radius",
    }
}

const fn boolean_reason_key(reason: BooleanFailureReason) -> &'static str {
    match reason {
        BooleanFailureReason::MissingOperand => "boolean.diagnostic.missing_operand",
        BooleanFailureReason::InvalidOperandTopology => {
            "boolean.diagnostic.invalid_operand_topology"
        }
        BooleanFailureReason::InvalidOperandGeometry => {
            "boolean.diagnostic.invalid_operand_geometry"
        }
        BooleanFailureReason::DisjointOperands => "boolean.diagnostic.disjoint_operands",
        BooleanFailureReason::KernelRejected => "boolean.diagnostic.kernel_rejected",
        BooleanFailureReason::KernelPanic => "boolean.diagnostic.kernel_panic",
        BooleanFailureReason::EmptyResult => "boolean.diagnostic.empty_result",
        BooleanFailureReason::InvalidResultTopology => "boolean.diagnostic.invalid_result_topology",
        BooleanFailureReason::HealingFailed => "boolean.diagnostic.healing_failed",
        BooleanFailureReason::ResultEvaluationFailed => {
            "boolean.diagnostic.result_evaluation_failed"
        }
        BooleanFailureReason::TopologyNamingFailed => "boolean.diagnostic.topology_naming_failed",
    }
}

const fn boolean_stage_key(stage: BooleanFailureStage) -> &'static str {
    match stage {
        BooleanFailureStage::OperandResolution => "boolean.stage.operand_resolution",
        BooleanFailureStage::OperandValidation => "boolean.stage.operand_validation",
        BooleanFailureStage::BroadPhase => "boolean.stage.broad_phase",
        BooleanFailureStage::KernelOperation => "boolean.stage.kernel_operation",
        BooleanFailureStage::ResultValidation => "boolean.stage.result_validation",
        BooleanFailureStage::TopologyHealing => "boolean.stage.topology_healing",
        BooleanFailureStage::TopologyNaming => "boolean.stage.topology_naming",
    }
}

const fn interference_stage_key(stage: InterferenceFailureStage) -> &'static str {
    match stage {
        InterferenceFailureStage::KernelOperation => "interference.stage.kernel_operation",
        InterferenceFailureStage::ResultValidation => "interference.stage.result_validation",
        InterferenceFailureStage::VolumeIntegration => "interference.stage.volume_integration",
    }
}

const fn interference_reason_key(reason: InterferenceFailureReason) -> &'static str {
    match reason {
        InterferenceFailureReason::KernelRejected => "interference.reason.kernel_rejected",
        InterferenceFailureReason::KernelPanic => "interference.reason.kernel_panic",
        InterferenceFailureReason::InvalidResultTopology => {
            "interference.reason.invalid_result_topology"
        }
        InterferenceFailureReason::EmptyMesh => "interference.reason.empty_mesh",
        InterferenceFailureReason::NonFiniteVolume => "interference.reason.non_finite_volume",
    }
}

impl eframe::App for CadxApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.keyboard_shortcuts(ui.ctx());
        self.receive_ai_plans();
        self.header(ui);
        self.status_bar(ui);
        self.model_panel(ui);
        self.ai_panel(ui);
        self.viewport(ui);
        self.loft_dialog(ui.ctx());
        self.boolean_dialog(ui.ctx());
        self.edge_modifier_dialog(ui.ctx());
        self.interference_dialog(ui.ctx());
        self.measurement_panel(ui.ctx());
        self.sketch_dimension_dialog(ui.ctx());
        self.domain_report_window(ui.ctx());
        if self.ai_pending {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }
}

fn icon_button(
    ui: &mut egui::Ui,
    icon: &str,
    tooltip: &str,
    enabled: bool,
    selected: bool,
) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(appearance::icon(icon, 14.0))
            .selected(selected)
            .min_size(egui::vec2(30.0, 28.0)),
    )
    .on_hover_text(tooltip)
}

fn tool_button(
    ui: &mut egui::Ui,
    icon: &str,
    label: &str,
    tooltip: &str,
    enabled: bool,
) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new((
            appearance::icon(icon, 14.0),
            egui::RichText::new(label).size(11.0),
        ))
        .min_size(egui::vec2(0.0, 30.0)),
    )
    .on_hover_text(tooltip)
}

fn domain_tool_label<'a>(id: &'a str, domain: DomainId, translator: &'a Translator) -> &'a str {
    let key = match id {
        "feature-tree" => "mechanical.tool.feature_tree",
        "sketch" => "mechanical.tool.sketch",
        "extrude" => "mechanical.tool.extrude",
        "edge-modifiers" => "mechanical.tool.edge_modifiers",
        "drawing" => "mechanical.tool.drawing",
        "standards-check" => "mechanical.tool.standards",
        "standard-parts" => "mechanical.tool.standard_parts",
        "assembly" => "mechanical.tool.assembly",
        "interference" => "mechanical.tool.interference",
        "dfm" => "mechanical.tool.dfm",
        "ai-part" => "mechanical.tool.ai_part",
        "bom" => {
            if domain == DomainId::Ecad {
                "pcb.tool.bom"
            } else {
                "mechanical.tool.bom"
            }
        }
        "board" => "pcb.tool.board",
        "placement" => "pcb.tool.component",
        "routing" => "pcb.tool.routing",
        "drc" => "pcb.tool.drc",
        "stackup" => "pcb.tool.stackup",
        "3d-link" => "pcb.tool.link_3d",
        "gerber" => "pcb.tool.gerber",
        "wall" => "aec.tool.wall",
        "slab" => "aec.tool.slab",
        "opening" => "aec.tool.opening",
        "levels" => "aec.tool.levels",
        "space" => "aec.tool.space",
        "bim-attrs" => "aec.tool.bim_attrs",
        "ifc" => "aec.tool.ifc",
        "schedule" => "aec.tool.schedule",
        "quantity-takeoff" => "aec.tool.quantity_takeoff",
        "clash" => "aec.tool.clash",
        "schematic" => "pcb.tool.schematic",
        "netlist" => "pcb.tool.netlist",
        "footprint-library" => "pcb.tool.footprint_library",
        "diff-pair" => "pcb.tool.diff_pair",
        "via" => "pcb.tool.via",
        _ => id,
    };
    translator.text(key)
}

fn domain_description(domain: DomainId, translator: &Translator) -> &str {
    translator.text(match domain {
        DomainId::Mcad => "domain.mcad_description",
        DomainId::Aec => "domain.aec_description",
        DomainId::Ecad => "domain.ecad_description",
    })
}

fn domain_category_label<'a>(category: &str, translator: &'a Translator) -> &'a str {
    let key = match category {
        "2D" => "domain.category.2d",
        "3D" => "domain.category.3d",
        "AEC" => "domain.category.aec",
        "AI" => "domain.category.ai",
        "Analysis" => "domain.category.analysis",
        "BIM" => "domain.category.bim",
        "ECAD" => "domain.category.ecad",
        "Export" => "domain.category.export",
        "MCAD" => "domain.category.mcad",
        _ => "domain.category.other",
    };
    translator.text(key)
}

fn aec_tool_icon(id: &str) -> &'static str {
    match id {
        "wall" => "panel-top",
        "slab" => "square",
        "opening" => "door-open",
        "levels" => "layers",
        "space" => "cuboid",
        "bim-attrs" => "list-tree",
        "ifc" => "file-output",
        "schedule" => "table",
        "quantity-takeoff" => "calculator",
        "clash" => "scan-search",
        _ => "circle",
    }
}

fn domain_field_editor(
    ui: &mut egui::Ui,
    field: &DomainFieldSchema,
    values: &mut DomainParameters,
) {
    let value = values
        .entry(field.id.to_string())
        .or_insert_with(|| default_domain_field_value(field));
    ui.horizontal(|ui| {
        match field.kind {
            DomainFieldKind::Text | DomainFieldKind::Select => {
                if !matches!(value, DomainFieldValue::Text(_)) {
                    *value = default_domain_field_value(field);
                }
                let DomainFieldValue::Text(text) = value else {
                    unreachable!("text field value is normalized before editing")
                };
                if field.kind == DomainFieldKind::Select {
                    egui::ComboBox::from_id_salt(("domain_select", field.id))
                        .selected_text(text.as_str())
                        .width(128.0)
                        .show_ui(ui, |ui| {
                            for option in field.options {
                                ui.selectable_value(text, option.value.to_string(), option.label);
                            }
                        });
                } else {
                    ui.add(egui::TextEdit::singleline(text).desired_width(148.0));
                }
            }
            DomainFieldKind::Integer => {
                if !matches!(value, DomainFieldValue::Integer(_)) {
                    *value = default_domain_field_value(field);
                }
                let DomainFieldValue::Integer(number) = value else {
                    unreachable!("integer field value is normalized before editing")
                };
                ui.add(egui::DragValue::new(number).speed(1));
            }
            DomainFieldKind::Decimal | DomainFieldKind::LengthMm | DomainFieldKind::AngleDeg => {
                if !matches!(value, DomainFieldValue::Decimal(_)) {
                    *value = default_domain_field_value(field);
                }
                let DomainFieldValue::Decimal(number) = value else {
                    unreachable!("decimal field value is normalized before editing")
                };
                ui.add(egui::DragValue::new(number).speed(0.1).max_decimals(4));
            }
            DomainFieldKind::Boolean => {
                if !matches!(value, DomainFieldValue::Boolean(_)) {
                    *value = default_domain_field_value(field);
                }
                let DomainFieldValue::Boolean(enabled) = value else {
                    unreachable!("boolean field value is normalized before editing")
                };
                ui.checkbox(enabled, "");
            }
            DomainFieldKind::EntityReference => {
                if !matches!(value, DomainFieldValue::EntityReference(_)) {
                    *value = default_domain_field_value(field);
                }
                let DomainFieldValue::EntityReference(reference) = value else {
                    unreachable!("entity field value is normalized before editing")
                };
                ui.add(egui::DragValue::new(reference).speed(1));
            }
            DomainFieldKind::Color => {
                if !matches!(value, DomainFieldValue::Color(_)) {
                    *value = default_domain_field_value(field);
                }
                let DomainFieldValue::Color(color) = value else {
                    unreachable!("color field value is normalized before editing")
                };
                ui.add(egui::TextEdit::singleline(color).desired_width(108.0));
            }
        }
        if let Some(unit) = field.unit {
            ui.label(
                egui::RichText::new(unit)
                    .monospace()
                    .size(9.0)
                    .color(appearance::TEXT_FAINT),
            );
        }
    });
}

fn default_domain_field_value(field: &DomainFieldSchema) -> DomainFieldValue {
    let default = field.default_value.unwrap_or_default();
    match field.kind {
        DomainFieldKind::Text | DomainFieldKind::Select => DomainFieldValue::Text(default.into()),
        DomainFieldKind::Integer => {
            DomainFieldValue::Integer(default.parse::<i64>().unwrap_or_default())
        }
        DomainFieldKind::Decimal | DomainFieldKind::LengthMm | DomainFieldKind::AngleDeg => {
            DomainFieldValue::Decimal(default.parse::<f64>().unwrap_or_default())
        }
        DomainFieldKind::Boolean => {
            DomainFieldValue::Boolean(default.parse::<bool>().unwrap_or_default())
        }
        DomainFieldKind::EntityReference => {
            DomainFieldValue::EntityReference(default.parse::<u64>().unwrap_or_default())
        }
        DomainFieldKind::Color => DomainFieldValue::Color(default.into()),
    }
}

const fn domain_icon(domain: DomainId) -> &'static str {
    match domain {
        DomainId::Mcad => "boxes",
        DomainId::Aec => "building-2",
        DomainId::Ecad => "layers",
    }
}

fn assembly_mate_kind_key(kind: &AssemblyMateKind) -> &'static str {
    match kind {
        AssemblyMateKind::Fixed => "mate.fixed",
        AssemblyMateKind::Revolute { .. } => "mate.revolute",
        AssemblyMateKind::Slider { .. } => "mate.slider",
    }
}

fn assembly_mate_axis(kind: &AssemblyMateKind) -> Option<[f64; 3]> {
    match kind {
        AssemblyMateKind::Fixed => None,
        AssemblyMateKind::Revolute { axis, .. } | AssemblyMateKind::Slider { axis, .. } => {
            Some(*axis)
        }
    }
}

fn mate_axis_options() -> [(&'static str, [f64; 3]); 3] {
    [
        ("X", [1.0, 0.0, 0.0]),
        ("Y", [0.0, 1.0, 0.0]),
        ("Z", [0.0, 0.0, 1.0]),
    ]
}

fn assembly_mate_frame_ui(ui: &mut egui::Ui, label: &str, frame: AssemblyTransform) {
    property_label(ui, label, None);
    let [x, y, z] = frame.translation.map(|value| format!("{value:.3}"));
    let [rx, ry, rz] = frame.euler_xyz_degrees().map(|value| format!("{value:.2}"));
    ui.label(
        egui::RichText::new(format!("X {x}  Y {y}  Z {z} mm"))
            .monospace()
            .size(9.0)
            .color(appearance::TEXT_MUTED),
    );
    ui.label(
        egui::RichText::new(format!("X {rx}°  Y {ry}°  Z {rz}°"))
            .monospace()
            .size(9.0)
            .color(appearance::TEXT_MUTED),
    );
}

fn loft_sections_compatible(
    sketch_ids: &[FeatureId],
    candidates: &[(FeatureId, String, usize, bool)],
) -> bool {
    let Some((segment_count, winding)) = sketch_ids.first().and_then(|first| {
        candidates
            .iter()
            .find(|(candidate, ..)| candidate == first)
            .map(|(_, _, segments, winding)| (*segments, *winding))
    }) else {
        return sketch_ids.is_empty();
    };
    sketch_ids.iter().all(|id| {
        candidates
            .iter()
            .any(|(candidate, _, segments, candidate_winding)| {
                candidate == id && *segments == segment_count && *candidate_winding == winding
            })
    })
}

fn planar_face_selection(scene: &EvaluatedScene, selection: Option<&FaceRef>) -> Option<FaceRef> {
    selection
        .filter(|reference| {
            scene
                .face(reference)
                .is_some_and(|face| face.geometry.plane.is_some())
        })
        .cloned()
}

fn exact_circle_loop(center: [f64; 2], radius: f64) -> SketchLoop2D {
    let right = [center[0] + radius, center[1]];
    let left = [center[0] - radius, center[1]];
    SketchLoop2D {
        segments: vec![
            SketchSegment2D::Arc {
                start: right,
                end: left,
                center,
                ccw: true,
            },
            SketchSegment2D::Arc {
                start: left,
                end: right,
                center,
                ccw: true,
            },
        ],
    }
}

fn linear_loop_editor(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    points: &mut [[f64; 2]],
) -> bool {
    let mut changed = false;
    egui::Grid::new(("linear_sketch_loop", id))
        .num_columns(3)
        .spacing([6.0, 6.0])
        .show(ui, |ui| {
            ui.label("#");
            ui.label("X");
            ui.label("Y");
            ui.end_row();
            for (index, point) in points.iter_mut().enumerate() {
                ui.label((index + 1).to_string());
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut point[0])
                            .speed(0.25)
                            .suffix(" mm"),
                    )
                    .changed();
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut point[1])
                            .speed(0.25)
                            .suffix(" mm"),
                    )
                    .changed();
                ui.end_row();
            }
        });
    changed
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SketchSegmentKind {
    Line,
    Arc,
    RationalQuadratic,
    CubicBezier,
}

impl SketchSegmentKind {
    const fn from_segment(segment: &SketchSegment2D) -> Self {
        match segment {
            SketchSegment2D::Line { .. } => Self::Line,
            SketchSegment2D::Arc { .. } => Self::Arc,
            SketchSegment2D::RationalQuadratic { .. } => Self::RationalQuadratic,
            SketchSegment2D::CubicBezier { .. } => Self::CubicBezier,
        }
    }

    const fn translation_key(self) -> &'static str {
        match self {
            Self::Line => "sketch.segment_line",
            Self::Arc => "sketch.segment_arc",
            Self::RationalQuadratic => "sketch.segment_rational_quadratic",
            Self::CubicBezier => "sketch.segment_cubic_bezier",
        }
    }

    fn segment(self, start: [f64; 2], end: [f64; 2]) -> SketchSegment2D {
        let delta = [end[0] - start[0], end[1] - start[1]];
        match self {
            Self::Line => SketchSegment2D::Line { start, end },
            Self::Arc => SketchSegment2D::Arc {
                start,
                end,
                center: [
                    (start[0] + end[0] - delta[1]) / 2.0,
                    (start[1] + end[1] + delta[0]) / 2.0,
                ],
                ccw: true,
            },
            Self::RationalQuadratic => SketchSegment2D::RationalQuadratic {
                start,
                control: [
                    (start[0] + end[0] - delta[1]) / 2.0,
                    (start[1] + end[1] + delta[0]) / 2.0,
                ],
                end,
                weight: 1.0,
            },
            Self::CubicBezier => SketchSegment2D::CubicBezier {
                start,
                control1: [start[0] + delta[0] / 3.0, start[1] + delta[1] / 3.0],
                control2: [
                    start[0] + delta[0] * 2.0 / 3.0,
                    start[1] + delta[1] * 2.0 / 3.0,
                ],
                end,
            },
        }
    }
}

fn point2_editor(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    point: &mut [f64; 2],
) -> bool {
    let mut changed = false;
    egui::Grid::new(("sketch_point2", id))
        .num_columns(3)
        .spacing([6.0, 6.0])
        .show(ui, |ui| {
            ui.label(label);
            changed |= ui
                .add(
                    egui::DragValue::new(&mut point[0])
                        .speed(0.25)
                        .prefix("X ")
                        .suffix(" mm"),
                )
                .changed();
            changed |= ui
                .add(
                    egui::DragValue::new(&mut point[1])
                        .speed(0.25)
                        .prefix("Y ")
                        .suffix(" mm"),
                )
                .changed();
            ui.end_row();
        });
    changed
}

fn set_segment_start(segment: &mut SketchSegment2D, value: [f64; 2]) {
    match segment {
        SketchSegment2D::Line { start, .. }
        | SketchSegment2D::Arc { start, .. }
        | SketchSegment2D::RationalQuadratic { start, .. }
        | SketchSegment2D::CubicBezier { start, .. } => *start = value,
    }
}

fn set_segment_end(segment: &mut SketchSegment2D, value: [f64; 2]) {
    match segment {
        SketchSegment2D::Line { end, .. }
        | SketchSegment2D::Arc { end, .. }
        | SketchSegment2D::RationalQuadratic { end, .. }
        | SketchSegment2D::CubicBezier { end, .. } => *end = value,
    }
}

fn project_to_radius(center: [f64; 2], point: [f64; 2], radius: f64) -> [f64; 2] {
    let offset = [point[0] - center[0], point[1] - center[1]];
    let length = offset[0].hypot(offset[1]);
    if length <= f64::EPSILON {
        [center[0] + radius, center[1]]
    } else {
        [
            radius.mul_add(offset[0] / length, center[0]),
            radius.mul_add(offset[1] / length, center[1]),
        ]
    }
}

fn sketch_segment_editor(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug + Clone,
    sketch_loop: &mut SketchLoop2D,
    translator: &Translator,
) -> bool {
    let mut changed = false;
    for index in 0..sketch_loop.segments.len() {
        let original = sketch_loop.segments[index].clone();
        let mut edited = original.clone();
        let mut kind = SketchSegmentKind::from_segment(&edited);
        let original_kind = kind;
        let segment_label = translator.format(
            "sketch.segment",
            &[
                ("index", &(index + 1).to_string()),
                ("type", translator.text(kind.translation_key())),
            ],
        );
        let mut start_changed = false;
        let mut end_changed = false;
        let mut center_changed = false;
        egui::CollapsingHeader::new(segment_label)
            .id_salt(("exact_sketch_segment", id.clone(), index))
            .default_open(true)
            .show(ui, |ui| {
                egui::ComboBox::from_id_salt(("segment_kind", id.clone(), index))
                    .selected_text(translator.text(kind.translation_key()))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut kind,
                            SketchSegmentKind::Line,
                            translator.text("sketch.segment_line"),
                        );
                        ui.selectable_value(
                            &mut kind,
                            SketchSegmentKind::Arc,
                            translator.text("sketch.segment_arc"),
                        );
                        ui.selectable_value(
                            &mut kind,
                            SketchSegmentKind::RationalQuadratic,
                            translator.text("sketch.segment_rational_quadratic"),
                        );
                        ui.selectable_value(
                            &mut kind,
                            SketchSegmentKind::CubicBezier,
                            translator.text("sketch.segment_cubic_bezier"),
                        );
                    });
                if kind != original_kind {
                    let start = original.start();
                    let end = original.end();
                    edited = kind.segment(start, end);
                }
                match &mut edited {
                    SketchSegment2D::Line { start, end } => {
                        start_changed |= point2_editor(
                            ui,
                            (id.clone(), index, "start"),
                            translator.text("sketch.segment_start"),
                            start,
                        );
                        end_changed |= point2_editor(
                            ui,
                            (id.clone(), index, "end"),
                            translator.text("sketch.segment_end"),
                            end,
                        );
                    }
                    SketchSegment2D::Arc {
                        start,
                        end,
                        center,
                        ccw,
                    } => {
                        let old_center = *center;
                        start_changed |= point2_editor(
                            ui,
                            (id.clone(), index, "start"),
                            translator.text("sketch.segment_start"),
                            start,
                        );
                        end_changed |= point2_editor(
                            ui,
                            (id.clone(), index, "end"),
                            translator.text("sketch.segment_end"),
                            end,
                        );
                        center_changed |= point2_editor(
                            ui,
                            (id.clone(), index, "center"),
                            translator.text("sketch.segment_center"),
                            center,
                        );
                        if center_changed {
                            let delta = [center[0] - old_center[0], center[1] - old_center[1]];
                            for point in [start, end] {
                                point[0] += delta[0];
                                point[1] += delta[1];
                            }
                            start_changed = true;
                            end_changed = true;
                        } else if start_changed && !end_changed {
                            *end = project_to_radius(
                                *center,
                                *end,
                                (start[0] - center[0]).hypot(start[1] - center[1]),
                            );
                            end_changed = true;
                        } else if end_changed && !start_changed {
                            *start = project_to_radius(
                                *center,
                                *start,
                                (end[0] - center[0]).hypot(end[1] - center[1]),
                            );
                            start_changed = true;
                        }
                        ui.checkbox(ccw, translator.text("sketch.segment_ccw"));
                    }
                    SketchSegment2D::RationalQuadratic {
                        start,
                        control,
                        end,
                        weight,
                    } => {
                        start_changed |= point2_editor(
                            ui,
                            (id.clone(), index, "start"),
                            translator.text("sketch.segment_start"),
                            start,
                        );
                        point2_editor(
                            ui,
                            (id.clone(), index, "control"),
                            translator.text("sketch.segment_control"),
                            control,
                        );
                        end_changed |= point2_editor(
                            ui,
                            (id.clone(), index, "end"),
                            translator.text("sketch.segment_end"),
                            end,
                        );
                        ui.add(
                            egui::DragValue::new(weight)
                                .speed(0.01)
                                .range(0.001..=1_000.0)
                                .prefix(format!("{} ", translator.text("sketch.segment_weight"))),
                        );
                    }
                    SketchSegment2D::CubicBezier {
                        start,
                        control1,
                        control2,
                        end,
                    } => {
                        start_changed |= point2_editor(
                            ui,
                            (id.clone(), index, "start"),
                            translator.text("sketch.segment_start"),
                            start,
                        );
                        point2_editor(
                            ui,
                            (id.clone(), index, "control1"),
                            translator.text("sketch.segment_control1"),
                            control1,
                        );
                        point2_editor(
                            ui,
                            (id.clone(), index, "control2"),
                            translator.text("sketch.segment_control2"),
                            control2,
                        );
                        end_changed |= point2_editor(
                            ui,
                            (id.clone(), index, "end"),
                            translator.text("sketch.segment_end"),
                            end,
                        );
                    }
                }
            });

        if edited != original {
            let start = edited.start();
            let end = edited.end();
            sketch_loop.segments[index] = edited;
            let previous = if index == 0 {
                sketch_loop.segments.len() - 1
            } else {
                index - 1
            };
            let next = (index + 1) % sketch_loop.segments.len();
            if start_changed || kind != original_kind {
                set_segment_end(&mut sketch_loop.segments[previous], start);
            }
            if end_changed || kind != original_kind {
                set_segment_start(&mut sketch_loop.segments[next], end);
            }
            changed = true;
        }
    }
    changed
}

fn sketch_construction_editor(
    ui: &mut egui::Ui,
    feature_id: FeatureId,
    profile: &SketchLoop2D,
    construction: &mut Vec<SketchSegment2D>,
    translator: &Translator,
) -> bool {
    let mut changed = false;
    let mut remove = None;
    for (index, segment) in construction.iter_mut().enumerate() {
        let original = segment.clone();
        let mut edited = original.clone();
        let mut kind = SketchSegmentKind::from_segment(&edited);
        let original_kind = kind;
        let segment_id = construction_segment_id(profile.segments.len(), index).unwrap_or(u32::MAX);
        let [start_id, end_id] =
            construction_point_ids(profile.segments.len(), index).unwrap_or([u32::MAX; 2]);
        let label = translator.format(
            "sketch.construction_segment",
            &[
                ("segment", &segment_id.to_string()),
                ("start", &start_id.to_string()),
                ("end", &end_id.to_string()),
                ("type", translator.text(kind.translation_key())),
            ],
        );
        egui::CollapsingHeader::new(label)
            .id_salt(("construction_segment", feature_id, index))
            .default_open(true)
            .show(ui, |ui| {
                egui::ComboBox::from_id_salt(("construction_kind", feature_id, index))
                    .selected_text(translator.text(kind.translation_key()))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut kind,
                            SketchSegmentKind::Line,
                            translator.text("sketch.segment_line"),
                        );
                        ui.selectable_value(
                            &mut kind,
                            SketchSegmentKind::Arc,
                            translator.text("sketch.segment_arc"),
                        );
                        ui.selectable_value(
                            &mut kind,
                            SketchSegmentKind::RationalQuadratic,
                            translator.text("sketch.segment_rational_quadratic"),
                        );
                        ui.selectable_value(
                            &mut kind,
                            SketchSegmentKind::CubicBezier,
                            translator.text("sketch.segment_cubic_bezier"),
                        );
                    });
                if kind != original_kind {
                    let start = original.start();
                    let end = original.end();
                    edited = kind.segment(start, end);
                }
                match &mut edited {
                    SketchSegment2D::Line { start, end } => {
                        point2_editor(
                            ui,
                            (feature_id, index, "construction_start"),
                            &format!("P{start_id}"),
                            start,
                        );
                        point2_editor(
                            ui,
                            (feature_id, index, "construction_end"),
                            &format!("P{end_id}"),
                            end,
                        );
                    }
                    SketchSegment2D::Arc {
                        start,
                        end,
                        center,
                        ccw,
                    } => {
                        let old_start = *start;
                        let old_end = *end;
                        let old_center = *center;
                        let start_changed = point2_editor(
                            ui,
                            (feature_id, index, "construction_start"),
                            &format!("P{start_id}"),
                            start,
                        );
                        let end_changed = point2_editor(
                            ui,
                            (feature_id, index, "construction_end"),
                            &format!("P{end_id}"),
                            end,
                        );
                        let center_changed = point2_editor(
                            ui,
                            (feature_id, index, "construction_center"),
                            translator.text("sketch.segment_center"),
                            center,
                        );
                        if center_changed {
                            let delta = [center[0] - old_center[0], center[1] - old_center[1]];
                            *start = [old_start[0] + delta[0], old_start[1] + delta[1]];
                            *end = [old_end[0] + delta[0], old_end[1] + delta[1]];
                        } else if start_changed && !end_changed {
                            *end = project_to_radius(
                                *center,
                                *end,
                                (start[0] - center[0]).hypot(start[1] - center[1]),
                            );
                        } else if end_changed && !start_changed {
                            *start = project_to_radius(
                                *center,
                                *start,
                                (end[0] - center[0]).hypot(end[1] - center[1]),
                            );
                        }
                        ui.checkbox(ccw, translator.text("sketch.segment_ccw"));
                    }
                    SketchSegment2D::RationalQuadratic {
                        start,
                        control,
                        end,
                        weight,
                    } => {
                        point2_editor(
                            ui,
                            (feature_id, index, "construction_start"),
                            &format!("P{start_id}"),
                            start,
                        );
                        point2_editor(
                            ui,
                            (feature_id, index, "construction_control"),
                            translator.text("sketch.segment_control"),
                            control,
                        );
                        point2_editor(
                            ui,
                            (feature_id, index, "construction_end"),
                            &format!("P{end_id}"),
                            end,
                        );
                        ui.add(
                            egui::DragValue::new(weight)
                                .speed(0.01)
                                .range(0.001..=1_000.0)
                                .prefix(format!("{} ", translator.text("sketch.segment_weight"))),
                        );
                    }
                    SketchSegment2D::CubicBezier {
                        start,
                        control1,
                        control2,
                        end,
                    } => {
                        point2_editor(
                            ui,
                            (feature_id, index, "construction_start"),
                            &format!("P{start_id}"),
                            start,
                        );
                        point2_editor(
                            ui,
                            (feature_id, index, "construction_control1"),
                            translator.text("sketch.segment_control1"),
                            control1,
                        );
                        point2_editor(
                            ui,
                            (feature_id, index, "construction_control2"),
                            translator.text("sketch.segment_control2"),
                            control2,
                        );
                        point2_editor(
                            ui,
                            (feature_id, index, "construction_end"),
                            &format!("P{end_id}"),
                            end,
                        );
                    }
                }
                if icon_button(
                    ui,
                    "trash-2",
                    translator.text("tool.remove_construction"),
                    true,
                    false,
                )
                .clicked()
                {
                    remove = Some(index);
                }
            });
        if edited != original {
            *segment = edited;
            changed = true;
        }
    }
    if let Some(index) = remove {
        construction.remove(index);
        changed = true;
    }
    if tool_button(
        ui,
        "plus",
        translator.text("tool.add_construction"),
        translator.text("tool.add_construction"),
        construction.len() < MAX_CONSTRUCTION_SEGMENTS,
    )
    .clicked()
        && let Some(segment) = suggest_construction_segment(profile, construction.len())
    {
        construction.push(segment);
        changed = true;
    }
    changed
}

fn suggest_construction_segment(
    profile: &SketchLoop2D,
    construction_count: usize,
) -> Option<SketchSegment2D> {
    let points = profile.sampled_points(std::f64::consts::PI / 36.0);
    let mut minimum = [f64::INFINITY; 2];
    let mut maximum = [f64::NEG_INFINITY; 2];
    for point in points {
        for axis in 0..2 {
            minimum[axis] = minimum[axis].min(point[axis]);
            maximum[axis] = maximum[axis].max(point[axis]);
        }
    }
    let width = maximum[0] - minimum[0];
    let height = maximum[1] - minimum[1];
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return None;
    }
    let construction_count = u32::try_from(construction_count).unwrap_or(u32::MAX);
    let offset = (f64::from(construction_count) * height * 0.08).min(height * 0.4);
    let y = minimum[1].midpoint(maximum[1]) + offset;
    Some(SketchSegment2D::Line {
        start: [minimum[0], y],
        end: [maximum[0], y],
    })
}

fn suggest_sketch_hole(region: &SketchRegion2D) -> Option<SketchLoop2D> {
    let profile_points = region.profile.sampled_points(std::f64::consts::PI / 36.0);
    let mut minimum = [f64::INFINITY; 2];
    let mut maximum = [f64::NEG_INFINITY; 2];
    for point in &profile_points {
        for axis in 0..2 {
            minimum[axis] = minimum[axis].min(point[axis]);
            maximum[axis] = maximum[axis].max(point[axis]);
        }
    }
    let width = maximum[0] - minimum[0];
    let height = maximum[1] - minimum[1];
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return None;
    }

    let mut best = None::<([f64; 2], f64)>;
    for x_index in 1..32 {
        for y_index in 1..32 {
            let point = [
                (f64::from(x_index) / 32.0).mul_add(width, minimum[0]),
                (f64::from(y_index) / 32.0).mul_add(height, minimum[1]),
            ];
            if !region.profile.contains_point_strict(point)
                || region
                    .holes
                    .iter()
                    .any(|hole| hole.contains_point_strict(point))
            {
                continue;
            }
            let clearance = std::iter::once(&region.profile)
                .chain(region.holes.iter())
                .flat_map(|sketch_loop| sketch_loop.segments.iter())
                .map(|segment| segment.distance_squared_to(point).sqrt())
                .fold(f64::INFINITY, f64::min);
            if best
                .as_ref()
                .is_none_or(|(_, best_clearance)| clearance > *best_clearance)
            {
                best = Some((point, clearance));
            }
        }
    }
    let (center, clearance) = best?;
    if clearance <= width.hypot(height).mul_add(1.0e-3, 0.05) {
        return None;
    }
    let hole = exact_circle_loop(center, clearance * 0.25);
    let mut candidate = region.clone();
    candidate.holes.push(hole.clone());
    candidate.validate().is_ok().then_some(hole)
}

fn tab_button(ui: &mut egui::Ui, icon: &str, label: &str, selected: bool) -> egui::Response {
    ui.add(
        egui::Button::selectable(
            selected,
            (
                appearance::icon(icon, 13.0).color(if selected {
                    appearance::ACCENT
                } else {
                    appearance::TEXT_MUTED
                }),
                egui::RichText::new(label).size(11.0),
            ),
        )
        .min_size(egui::vec2(70.0, 30.0)),
    )
}

fn sketch_plane_label(
    plane: &SketchPlane,
    datum_options: &[(FeatureId, String)],
    translator: &Translator,
) -> String {
    match plane {
        SketchPlane::WorldXy => translator.text("sketch_plane.world_xy").into(),
        SketchPlane::WorldXz => translator.text("sketch_plane.world_xz").into(),
        SketchPlane::WorldYz => translator.text("sketch_plane.world_yz").into(),
        SketchPlane::DatumPlane { datum_id } => datum_options
            .iter()
            .find(|(candidate, _)| candidate == datum_id)
            .map_or_else(
                || format!("Datum #{datum_id}"),
                |(_, name)| translator.format("sketch_plane.datum", &[("name", name)]),
            ),
        SketchPlane::PlanarFace { face } => {
            translator.format("sketch_plane.face", &[("face", face.to_string().as_str())])
        }
    }
}

fn property_label(ui: &mut egui::Ui, label: &str, units: Option<&str>) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label)
                .size(10.0)
                .strong()
                .color(appearance::TEXT_MUTED),
        );
        if let Some(units) = units {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(units)
                        .monospace()
                        .size(9.0)
                        .color(appearance::TEXT_FAINT),
                );
            });
        }
    });
    ui.add_space(3.0);
}

fn sketch_solve_diagnostic_ui(
    ui: &mut egui::Ui,
    diagnostic: &SketchSolveDiagnostic,
    translator: &Translator,
) {
    let (status, color) = if diagnostic.is_fully_constrained() {
        ("sketch.diagnostic.fully_constrained", appearance::ACCENT)
    } else {
        ("sketch.diagnostic.under_constrained", appearance::WARNING)
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(translator.text(status))
                .size(10.0)
                .strong()
                .color(color),
        );
        let dof = diagnostic.degrees_of_freedom.to_string();
        let rank = diagnostic.rank.to_string();
        let parameters = diagnostic.parameter_count.to_string();
        let equations = diagnostic.equation_count.to_string();
        ui.label(
            egui::RichText::new(translator.format(
                "sketch.diagnostic.rank",
                &[
                    ("dof", &dof),
                    ("rank", &rank),
                    ("parameters", &parameters),
                    ("equations", &equations),
                ],
            ))
            .monospace()
            .size(9.0)
            .color(appearance::TEXT_MUTED),
        );
    });
    if !diagnostic.redundant_constraints.is_empty() {
        let indices = display_constraint_indices(&diagnostic.redundant_constraints);
        ui.label(
            egui::RichText::new(
                translator.format("sketch.diagnostic.redundant", &[("indices", &indices)]),
            )
            .size(9.0)
            .color(appearance::WARNING),
        );
    }
    ui.add_space(3.0);
}

fn sketch_failure_diagnostic_ui(
    ui: &mut egui::Ui,
    diagnostic: &SketchConstraintDiagnostic,
    translator: &Translator,
) {
    let reason = translator.text(match diagnostic.reason {
        SketchConstraintFailureReason::Conflict => "sketch.diagnostic.conflict",
        SketchConstraintFailureReason::NonConvergence => "sketch.diagnostic.non_convergence",
    });
    let indices = display_constraint_indices(&diagnostic.constraint_indices);
    let residual = format!("{:.3e}", diagnostic.residual);
    let iterations = diagnostic.iterations.to_string();
    ui.label(
        egui::RichText::new(translator.format(
            "sketch.diagnostic.failure",
            &[
                ("reason", reason),
                ("indices", &indices),
                ("iterations", &iterations),
                ("residual", &residual),
            ],
        ))
        .size(9.0)
        .color(appearance::DANGER),
    );
    ui.add_space(3.0);
}

fn display_constraint_indices(indices: &[u32]) -> String {
    if indices.is_empty() {
        return "-".into();
    }
    indices
        .iter()
        .map(|index| index.saturating_add(1).to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn constraint_label_key(constraint: &Constraint) -> &'static str {
    match constraint {
        Constraint::Coincident { .. } => "constraint.coincident",
        Constraint::Horizontal { .. } => "constraint.horizontal",
        Constraint::Vertical { .. } => "constraint.vertical",
        Constraint::Fixed { .. } => "constraint.fixed",
        Constraint::Distance { .. } => "constraint.distance",
        Constraint::HorizontalDistance { .. } => "constraint.horizontal_distance",
        Constraint::VerticalDistance { .. } => "constraint.vertical_distance",
        Constraint::PointLineDistance { .. } => "constraint.point_line_distance",
        Constraint::LineThroughCenter { .. } => "constraint.line_through_center",
        Constraint::PointOnCurve { .. } => "constraint.point_on_curve",
        Constraint::Midpoint { .. } => "constraint.midpoint",
        Constraint::Symmetric { .. } => "constraint.symmetric",
        Constraint::Length { .. } => "constraint.length",
        Constraint::EqualLength { .. } => "constraint.equal_length",
        Constraint::Parallel { .. } => "constraint.parallel",
        Constraint::Perpendicular { .. } => "constraint.perpendicular",
        Constraint::Angle { .. } => "constraint.angle",
        Constraint::Radius { .. } => "constraint.radius",
        Constraint::FixedCenter { .. } => "constraint.fixed_center",
        Constraint::EqualRadius { .. } => "constraint.equal_radius",
        Constraint::Concentric { .. } => "constraint.concentric",
        Constraint::Tangent { .. } => "constraint.tangent",
        Constraint::CurvatureContinuous { .. } => "constraint.curvature_continuous",
    }
}

const fn sketch_dimension_kind_key(kind: SketchDimensionKind) -> &'static str {
    match kind {
        SketchDimensionKind::Distance => "constraint.distance",
        SketchDimensionKind::HorizontalDistance => "constraint.horizontal_distance",
        SketchDimensionKind::VerticalDistance => "constraint.vertical_distance",
        SketchDimensionKind::PointLineDistance => "constraint.point_line_distance",
        SketchDimensionKind::Length => "constraint.length",
        SketchDimensionKind::Angle => "constraint.angle",
        SketchDimensionKind::Radius => "constraint.radius",
    }
}

fn replace_sketch_dimension(
    constraints: &mut [Constraint],
    constraint_index: u32,
    expected_kind: SketchDimensionKind,
    value: f64,
) -> bool {
    let Some(constraint) = usize::try_from(constraint_index)
        .ok()
        .and_then(|index| constraints.get_mut(index))
    else {
        return false;
    };
    if constraint
        .dimension()
        .is_none_or(|dimension| dimension.kind != expected_kind)
    {
        return false;
    }
    let Some(replacement) = constraint.with_dimension_value(value) else {
        return false;
    };
    *constraint = replacement;
    true
}

fn sketch_segments_matching(
    profile: &SketchLoop2D,
    construction: &[SketchSegment2D],
    arcs: bool,
) -> Vec<u32> {
    profile
        .segments
        .iter()
        .chain(construction)
        .enumerate()
        .filter(|(_, segment)| {
            if arcs {
                segment.is_arc()
            } else {
                segment.is_line()
            }
        })
        .filter_map(|(index, _)| u32::try_from(index).ok())
        .collect()
}

fn sketch_tangent_pairs(sketch_loop: &SketchLoop2D) -> Vec<(u32, u32)> {
    let count = sketch_loop.segments.len();
    let mut pairs = Vec::new();
    for first in 0..count {
        let second = (first + 1) % count;
        if !sketch_loop.segments[first].is_curved() && !sketch_loop.segments[second].is_curved() {
            continue;
        }
        let pair = if first < second {
            (first, second)
        } else {
            (second, first)
        };
        let pair = (u32::try_from(pair.0), u32::try_from(pair.1));
        if let (Ok(first), Ok(second)) = pair
            && !pairs.contains(&(first, second))
        {
            pairs.push((first, second));
        }
    }
    pairs
}

fn sketch_curvature_pairs(sketch_loop: &SketchLoop2D) -> Vec<(u32, u32)> {
    sketch_tangent_pairs(sketch_loop)
        .into_iter()
        .filter(|(first, second)| {
            let first = usize::try_from(*first).expect("profile segment id fits usize");
            let second = usize::try_from(*second).expect("profile segment id fits usize");
            sketch_loop.segments[first].is_curved() && sketch_loop.segments[second].is_curved()
        })
        .collect()
}

fn sketch_distinct_pairs(segments: &[u32]) -> Vec<(u32, u32)> {
    segments
        .iter()
        .enumerate()
        .flat_map(|(index, first)| {
            segments[index + 1..]
                .iter()
                .map(move |second| (*first, *second))
        })
        .collect()
}

fn sketch_ordered_pairs(segments: &[u32]) -> Vec<(u32, u32)> {
    segments
        .iter()
        .flat_map(|first| {
            segments
                .iter()
                .filter(move |second| *second != first)
                .map(move |second| (*first, *second))
        })
        .collect()
}

fn sketch_point_pairs(points: &[u32]) -> Vec<(u32, u32)> {
    sketch_distinct_pairs(points)
}

fn sketch_ordered_point_pairs(points: &[u32]) -> Vec<(u32, u32)> {
    sketch_ordered_pairs(points)
}

fn sketch_segment_point_pair(profile_count: usize, segment: u32) -> Option<(u32, u32)> {
    let index = usize::try_from(segment).ok()?;
    if index < profile_count {
        Some((segment, u32::try_from((index + 1) % profile_count).ok()?))
    } else {
        construction_point_ids(profile_count, index - profile_count)
            .map(|[start, end]| (start, end))
    }
}

fn sketch_point_segment_pairs(
    points: &[u32],
    segments: &[u32],
    profile_count: usize,
) -> Vec<(u32, u32)> {
    points
        .iter()
        .flat_map(|point| {
            segments.iter().filter_map(move |segment| {
                let (start, end) = sketch_segment_point_pair(profile_count, *segment)?;
                (*point != start && *point != end).then_some((*point, *segment))
            })
        })
        .collect()
}

fn sketch_all_point_segment_pairs(points: &[u32], segments: &[u32]) -> Vec<(u32, u32)> {
    points
        .iter()
        .flat_map(|point| segments.iter().map(move |segment| (*point, *segment)))
        .collect()
}

struct SketchConstraintOptions {
    segments: Vec<SketchSegment2D>,
    points: Vec<[f64; 2]>,
    all_points: Vec<u32>,
    line_segments: Vec<u32>,
    line_pairs: Vec<(u32, u32)>,
    angle_pairs: Vec<(u32, u32)>,
    arc_segments: Vec<u32>,
    arc_pairs: Vec<(u32, u32)>,
    tangent_pairs: Vec<(u32, u32)>,
    curvature_pairs: Vec<(u32, u32)>,
    point_pairs: Vec<(u32, u32)>,
    dimension_point_pairs: Vec<(u32, u32)>,
    point_curve_pairs: Vec<(u32, u32)>,
    point_line_pairs: Vec<(u32, u32)>,
    midpoint_pairs: Vec<(u32, u32)>,
    line_arc_pairs: Vec<(u32, u32)>,
}

impl SketchConstraintOptions {
    fn new(profile: &SketchLoop2D, construction: &[SketchSegment2D]) -> Self {
        let profile_count = profile.segments.len();
        let point_count = profile_count.saturating_add(construction.len().saturating_mul(2));
        let all_points = (0..point_count)
            .filter_map(|point| u32::try_from(point).ok())
            .collect::<Vec<_>>();
        let line_segments = sketch_segments_matching(profile, construction, false);
        let arc_segments = sketch_segments_matching(profile, construction, true);
        let all_segments = profile
            .segments
            .iter()
            .chain(construction)
            .enumerate()
            .filter_map(|(index, _)| u32::try_from(index).ok())
            .collect::<Vec<_>>();
        let segments = profile
            .segments
            .iter()
            .chain(construction)
            .cloned()
            .collect::<Vec<_>>();
        let points = profile
            .segments
            .iter()
            .map(SketchSegment2D::start)
            .chain(
                construction
                    .iter()
                    .flat_map(|segment| [segment.start(), segment.end()]),
            )
            .collect();
        Self {
            segments,
            points,
            line_pairs: sketch_distinct_pairs(&line_segments),
            angle_pairs: sketch_ordered_pairs(&line_segments),
            arc_pairs: sketch_distinct_pairs(&arc_segments),
            tangent_pairs: sketch_tangent_pairs(profile),
            curvature_pairs: sketch_curvature_pairs(profile),
            point_pairs: sketch_point_pairs(&all_points),
            dimension_point_pairs: sketch_ordered_point_pairs(&all_points),
            point_curve_pairs: sketch_point_segment_pairs(
                &all_points,
                &all_segments,
                profile_count,
            ),
            point_line_pairs: sketch_all_point_segment_pairs(&all_points, &line_segments),
            midpoint_pairs: sketch_point_segment_pairs(&all_points, &line_segments, profile_count),
            line_arc_pairs: line_segments
                .iter()
                .flat_map(|line| arc_segments.iter().map(move |arc| (*line, *arc)))
                .collect(),
            all_points,
            line_segments,
            arc_segments,
        }
    }

    fn segment(&self, id: u32) -> Option<&SketchSegment2D> {
        self.segments.get(usize::try_from(id).ok()?)
    }

    fn point(&self, id: u32) -> Option<[f64; 2]> {
        self.points.get(usize::try_from(id).ok()?).copied()
    }
}

fn point_line_distance(point: [f64; 2], line: &SketchSegment2D) -> f64 {
    let start = line.start();
    let end = line.end();
    let direction = [end[0] - start[0], end[1] - start[1]];
    let length = direction[0].hypot(direction[1]);
    if length <= f64::EPSILON {
        return 0.0;
    }
    direction[0]
        .mul_add(point[1] - start[1], -direction[1] * (point[0] - start[0]))
        .abs()
        / length
}

fn sketch_directed_angle_degrees(
    segments: &[SketchSegment2D],
    first: u32,
    second: u32,
) -> Option<f64> {
    let direction = |segment: u32| {
        let segment = segments.get(usize::try_from(segment).ok()?)?;
        let start = segment.start();
        let end = segment.end();
        let vector = [end[0] - start[0], end[1] - start[1]];
        let length = vector[0].hypot(vector[1]);
        (length > f64::EPSILON).then_some([vector[0] / length, vector[1] / length])
    };
    let first = direction(first)?;
    let second = direction(second)?;
    Some(
        first[0]
            .mul_add(second[1], -first[1] * second[0])
            .atan2(first[0].mul_add(second[0], first[1] * second[1]))
            .to_degrees(),
    )
}

fn point_id_editor(ui: &mut egui::Ui, point: &mut u32, maximum: u32) -> bool {
    ui.add(egui::DragValue::new(point).range(0..=maximum).prefix("P"))
        .changed()
}

fn sketch_point_pair_selector(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    first: &mut u32,
    second: &mut u32,
    candidates: &[(u32, u32)],
) -> bool {
    if candidates.is_empty() {
        ui.add_enabled(false, egui::Button::new(format!("P{first} · P{second}")));
        return false;
    }
    let mut selected = (*first, *second);
    let mut changed = false;
    egui::ComboBox::from_id_salt(salt)
        .selected_text(format!("P{first} · P{second}"))
        .width(92.0)
        .show_ui(ui, |ui| {
            for candidate in candidates {
                changed |= ui
                    .selectable_value(
                        &mut selected,
                        *candidate,
                        format!("P{} · P{}", candidate.0, candidate.1),
                    )
                    .changed();
            }
        });
    if changed {
        (*first, *second) = selected;
    }
    changed
}

fn sketch_point_segment_pair_selector(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    point: &mut u32,
    segment: &mut u32,
    candidates: &[(u32, u32)],
) -> bool {
    if candidates.is_empty() {
        ui.add_enabled(false, egui::Button::new(format!("P{point} · S{segment}")));
        return false;
    }
    let mut selected = (*point, *segment);
    let mut changed = false;
    egui::ComboBox::from_id_salt(salt)
        .selected_text(format!("P{point} · S{segment}"))
        .width(92.0)
        .show_ui(ui, |ui| {
            for candidate in candidates {
                changed |= ui
                    .selectable_value(
                        &mut selected,
                        *candidate,
                        format!("P{} · S{}", candidate.0, candidate.1),
                    )
                    .changed();
            }
        });
    if changed {
        (*point, *segment) = selected;
    }
    changed
}

fn sketch_segment_selector(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    segment: &mut u32,
    candidates: &[u32],
) -> bool {
    if candidates.is_empty() {
        ui.add_enabled(false, egui::Button::new(format!("S{segment}")));
        return false;
    }
    let mut changed = false;
    egui::ComboBox::from_id_salt(salt)
        .selected_text(format!("S{segment}"))
        .width(48.0)
        .show_ui(ui, |ui| {
            for candidate in candidates {
                changed |= ui
                    .selectable_value(segment, *candidate, format!("S{candidate}"))
                    .changed();
            }
        });
    changed
}

fn sketch_segment_pair_selector(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    first: &mut u32,
    second: &mut u32,
    candidates: &[(u32, u32)],
) -> bool {
    if candidates.is_empty() {
        ui.add_enabled(false, egui::Button::new(format!("S{first} · S{second}")));
        return false;
    }
    let mut selected = (*first, *second);
    let mut changed = false;
    egui::ComboBox::from_id_salt(salt)
        .selected_text(format!("S{first} · S{second}"))
        .width(92.0)
        .show_ui(ui, |ui| {
            for candidate in candidates {
                changed |= ui
                    .selectable_value(
                        &mut selected,
                        *candidate,
                        format!("S{} · S{}", candidate.0, candidate.1),
                    )
                    .changed();
            }
        });
    if changed {
        (*first, *second) = selected;
    }
    changed
}

fn constraint_menu_item(ui: &mut egui::Ui, icon: &str, label: &str, enabled: bool) -> bool {
    ui.add_enabled(
        enabled,
        egui::Button::new((
            appearance::icon(icon, 14.0),
            egui::RichText::new(label).size(11.0),
        ))
        .min_size(egui::vec2(ui.available_width(), 28.0)),
    )
    .clicked()
}

fn sketch_constraint_menu(
    ui: &mut egui::Ui,
    constraints: &mut Vec<Constraint>,
    profile: &SketchLoop2D,
    options: &SketchConstraintOptions,
    translator: &Translator,
) -> bool {
    let mut changed = false;
    ui.menu_button(
        (
            appearance::icon("plus", 14.0),
            egui::RichText::new(translator.text("constraint.add")).size(11.0),
        ),
        |ui| {
            egui::ScrollArea::vertical()
                .max_height(520.0)
                .show(ui, |ui| {
                    let first_line = options.line_segments.first().copied();
                    if constraint_menu_item(
                        ui,
                        "minus",
                        translator.text("constraint.horizontal"),
                        first_line.is_some(),
                    ) && let Some(segment) = first_line
                    {
                        constraints.push(Constraint::Horizontal { segment });
                        changed = true;
                        ui.close();
                    }
                    if constraint_menu_item(
                        ui,
                        "panel-top",
                        translator.text("constraint.vertical"),
                        first_line.is_some(),
                    ) && let Some(segment) = first_line
                    {
                        constraints.push(Constraint::Vertical { segment });
                        changed = true;
                        ui.close();
                    }
                    let line_geometry = first_line.and_then(|segment| {
                        options
                            .segment(segment)
                            .map(|geometry| (segment, geometry.length()))
                    });
                    if constraint_menu_item(
                        ui,
                        "radius",
                        translator.text("constraint.length"),
                        line_geometry.is_some(),
                    ) && let Some((segment, length)) = line_geometry
                    {
                        constraints.push(Constraint::Length { segment, length });
                        changed = true;
                        ui.close();
                    }
                    let point_dimension = options.dimension_point_pairs.first().copied().and_then(
                        |(first, second)| {
                            Some((first, second, options.point(first)?, options.point(second)?))
                        },
                    );
                    if constraint_menu_item(
                        ui,
                        "move-horizontal",
                        translator.text("constraint.horizontal_distance"),
                        point_dimension.is_some(),
                    ) && let Some((first, second, first_point, second_point)) = point_dimension
                    {
                        constraints.push(Constraint::HorizontalDistance {
                            first,
                            second,
                            distance: second_point[0] - first_point[0],
                        });
                        changed = true;
                        ui.close();
                    }
                    if constraint_menu_item(
                        ui,
                        "move-vertical",
                        translator.text("constraint.vertical_distance"),
                        point_dimension.is_some(),
                    ) && let Some((first, second, first_point, second_point)) = point_dimension
                    {
                        constraints.push(Constraint::VerticalDistance {
                            first,
                            second,
                            distance: second_point[1] - first_point[1],
                        });
                        changed = true;
                        ui.close();
                    }
                    let point_line_distance =
                        options
                            .point_line_pairs
                            .first()
                            .copied()
                            .and_then(|(point, line)| {
                                Some((
                                    point,
                                    line,
                                    point_line_distance(
                                        options.point(point)?,
                                        options.segment(line)?,
                                    ),
                                ))
                            });
                    if constraint_menu_item(
                        ui,
                        "between-horizontal-start",
                        translator.text("constraint.point_line_distance"),
                        point_line_distance.is_some(),
                    ) && let Some((point, line, distance)) = point_line_distance
                    {
                        constraints.push(Constraint::PointLineDistance {
                            point,
                            line,
                            distance,
                        });
                        changed = true;
                        ui.close();
                    }
                    let line_pair = options.line_pairs.first().copied();
                    if constraint_menu_item(
                        ui,
                        "combine",
                        translator.text("constraint.equal_length"),
                        line_pair.is_some(),
                    ) && let Some((first, second)) = line_pair
                    {
                        constraints.push(Constraint::EqualLength { first, second });
                        changed = true;
                        ui.close();
                    }
                    if constraint_menu_item(
                        ui,
                        "layers",
                        translator.text("constraint.parallel"),
                        line_pair.is_some(),
                    ) && let Some((first, second)) = line_pair
                    {
                        constraints.push(Constraint::Parallel { first, second });
                        changed = true;
                        ui.close();
                    }
                    if constraint_menu_item(
                        ui,
                        "panel-top",
                        translator.text("constraint.perpendicular"),
                        line_pair.is_some(),
                    ) && let Some((first, second)) = line_pair
                    {
                        constraints.push(Constraint::Perpendicular { first, second });
                        changed = true;
                        ui.close();
                    }
                    let angle_geometry =
                        options
                            .angle_pairs
                            .first()
                            .copied()
                            .and_then(|(first, second)| {
                                sketch_directed_angle_degrees(&options.segments, first, second)
                                    .map(|angle_degrees| (first, second, angle_degrees))
                            });
                    if constraint_menu_item(
                        ui,
                        "rotate-cw",
                        translator.text("constraint.angle"),
                        angle_geometry.is_some(),
                    ) && let Some((first, second, angle_degrees)) = angle_geometry
                    {
                        constraints.push(Constraint::Angle {
                            first,
                            second,
                            angle_degrees,
                        });
                        changed = true;
                        ui.close();
                    }
                    let first_point = profile.segments.first().map(SketchSegment2D::start);
                    if constraint_menu_item(
                        ui,
                        "focus",
                        translator.text("constraint.fixed"),
                        first_point.is_some(),
                    ) && let Some(point) = first_point
                    {
                        constraints.push(Constraint::Fixed {
                            point: 0,
                            x: point[0],
                            y: point[1],
                        });
                        changed = true;
                        ui.close();
                    }
                    let second_point = profile.segments.get(1).map(SketchSegment2D::start);
                    if constraint_menu_item(
                        ui,
                        "radius",
                        translator.text("constraint.distance"),
                        first_point.is_some() && second_point.is_some(),
                    ) && let (Some(first), Some(second)) = (first_point, second_point)
                    {
                        constraints.push(Constraint::Distance {
                            first: 0,
                            second: 1,
                            distance: (second[0] - first[0]).hypot(second[1] - first[1]),
                        });
                        changed = true;
                        ui.close();
                    }
                    if constraint_menu_item(
                        ui,
                        "locate-fixed",
                        translator.text("constraint.point_on_curve"),
                        !options.point_curve_pairs.is_empty(),
                    ) && let Some(&(point, segment)) = options.point_curve_pairs.first()
                    {
                        constraints.push(Constraint::PointOnCurve { point, segment });
                        changed = true;
                        ui.close();
                    }
                    if constraint_menu_item(
                        ui,
                        "align-center",
                        translator.text("constraint.midpoint"),
                        !options.midpoint_pairs.is_empty(),
                    ) && let Some(&(point, segment)) = options.midpoint_pairs.first()
                    {
                        constraints.push(Constraint::Midpoint { point, segment });
                        changed = true;
                        ui.close();
                    }
                    let symmetry = options.line_segments.iter().find_map(|axis| {
                        let endpoints = sketch_segment_point_pair(profile.segments.len(), *axis)?;
                        options
                            .point_pairs
                            .iter()
                            .find(|(first, second)| {
                                *first != endpoints.0
                                    && *first != endpoints.1
                                    && *second != endpoints.0
                                    && *second != endpoints.1
                            })
                            .map(|(first, second)| (*first, *second, *axis))
                    });
                    if constraint_menu_item(
                        ui,
                        "flip-horizontal-2",
                        translator.text("constraint.symmetric"),
                        symmetry.is_some(),
                    ) && let Some((first, second, axis)) = symmetry
                    {
                        constraints.push(Constraint::Symmetric {
                            first,
                            second,
                            axis,
                        });
                        changed = true;
                        ui.close();
                    }
                    if constraint_menu_item(
                        ui,
                        "combine",
                        translator.text("constraint.coincident"),
                        first_point.is_some(),
                    ) && first_point.is_some()
                    {
                        constraints.push(Constraint::Coincident {
                            first: 0,
                            second: 0,
                        });
                        changed = true;
                        ui.close();
                    }
                    ui.separator();
                    let first_arc = options.arc_segments.first().copied();
                    let arc_geometry = first_arc.and_then(|segment_id| {
                        options
                            .segment(segment_id)
                            .and_then(|segment| match segment {
                                SketchSegment2D::Arc { start, center, .. } => Some((
                                    segment_id,
                                    *center,
                                    (start[0] - center[0]).hypot(start[1] - center[1]),
                                )),
                                SketchSegment2D::Line { .. }
                                | SketchSegment2D::RationalQuadratic { .. }
                                | SketchSegment2D::CubicBezier { .. } => None,
                            })
                    });
                    if constraint_menu_item(
                        ui,
                        "circle",
                        translator.text("constraint.radius"),
                        arc_geometry.is_some(),
                    ) && let Some((segment, _, radius)) = arc_geometry
                    {
                        constraints.push(Constraint::Radius { segment, radius });
                        changed = true;
                        ui.close();
                    }
                    if constraint_menu_item(
                        ui,
                        "circle-dot",
                        translator.text("constraint.fixed_center"),
                        arc_geometry.is_some(),
                    ) && let Some((segment, center, _)) = arc_geometry
                    {
                        constraints.push(Constraint::FixedCenter {
                            segment,
                            x: center[0],
                            y: center[1],
                        });
                        changed = true;
                        ui.close();
                    }
                    let line_arc = options.line_arc_pairs.first().copied();
                    if constraint_menu_item(
                        ui,
                        "circle-dot-dashed",
                        translator.text("constraint.line_through_center"),
                        line_arc.is_some(),
                    ) && let Some((line, arc)) = line_arc
                    {
                        constraints.push(Constraint::LineThroughCenter { line, arc });
                        changed = true;
                        ui.close();
                    }
                    let arc_pair = options.arc_pairs.first().copied();
                    if constraint_menu_item(
                        ui,
                        "combine",
                        translator.text("constraint.equal_radius"),
                        arc_pair.is_some(),
                    ) && let Some((first, second)) = arc_pair
                    {
                        constraints.push(Constraint::EqualRadius { first, second });
                        changed = true;
                        ui.close();
                    }
                    if constraint_menu_item(
                        ui,
                        "focus",
                        translator.text("constraint.concentric"),
                        arc_pair.is_some(),
                    ) && let Some((first, second)) = arc_pair
                    {
                        constraints.push(Constraint::Concentric { first, second });
                        changed = true;
                        ui.close();
                    }
                    if constraint_menu_item(
                        ui,
                        "radius",
                        translator.text("constraint.tangent"),
                        !options.tangent_pairs.is_empty(),
                    ) && let Some(&(first, second)) = options.tangent_pairs.first()
                    {
                        constraints.push(Constraint::Tangent { first, second });
                        changed = true;
                        ui.close();
                    }
                    if constraint_menu_item(
                        ui,
                        "activity",
                        translator.text("constraint.curvature_continuous"),
                        !options.curvature_pairs.is_empty(),
                    ) && let Some(&(first, second)) = options.curvature_pairs.first()
                    {
                        constraints.push(Constraint::CurvatureContinuous { first, second });
                        changed = true;
                        ui.close();
                    }
                });
        },
    );
    changed
}

fn vec3_editor(
    ui: &mut egui::Ui,
    value: &mut [f64; 3],
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    vec3_editor_with_suffix(ui, value, range, " mm")
}

fn vec3_editor_with_suffix(
    ui: &mut egui::Ui,
    value: &mut [f64; 3],
    range: std::ops::RangeInclusive<f64>,
    suffix: &str,
) -> bool {
    let mut changed = false;
    let axes = [
        ("X", egui::Color32::from_rgb(224, 105, 105)),
        ("Y", egui::Color32::from_rgb(102, 190, 126)),
        ("Z", egui::Color32::from_rgb(100, 150, 222)),
    ];
    egui::Grid::new(ui.next_auto_id())
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            for ((axis, color), coordinate) in axes.into_iter().zip(value.iter_mut()) {
                ui.label(egui::RichText::new(axis).monospace().strong().color(color));
                changed |= ui
                    .add(
                        egui::DragValue::new(coordinate)
                            .speed(0.25)
                            .range(range.clone())
                            .suffix(suffix),
                    )
                    .changed();
                ui.end_row();
            }
        });
    changed
}

fn conversation_entry(ui: &mut egui::Ui, entry: &ConversationEntry, translator: &Translator) {
    let (role, role_color, fill, align) = match entry.speaker {
        Speaker::User => (
            translator.text("ai.you"),
            appearance::WARNING,
            egui::Color32::from_rgb(45, 39, 33),
            egui::Align::Max,
        ),
        Speaker::Assistant => (
            translator.text("ai.assistant"),
            appearance::ACCENT,
            appearance::SURFACE_RAISED,
            egui::Align::Min,
        ),
        Speaker::Error => (
            translator.text("ai.error"),
            appearance::DANGER,
            egui::Color32::from_rgb(47, 31, 32),
            egui::Align::Min,
        ),
    };

    ui.with_layout(egui::Layout::top_down(align), |ui| {
        egui::Frame::new()
            .fill(fill)
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(9, 7))
            .stroke(egui::Stroke::new(1.0, appearance::BORDER_SOFT))
            .show(ui, |ui| {
                ui.set_max_width(260.0);
                ui.label(
                    egui::RichText::new(role)
                        .size(9.0)
                        .strong()
                        .color(role_color),
                );
                ui.label(
                    egui::RichText::new(entry.content.resolve(translator))
                        .size(11.0)
                        .color(appearance::TEXT),
                );
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loft_section_precheck_requires_matching_segments_and_winding() {
        let candidates = vec![
            (1, "first".into(), 4, true),
            (2, "second".into(), 4, true),
            (3, "triangle".into(), 3, true),
            (4, "reversed".into(), 4, false),
        ];

        assert!(loft_sections_compatible(&[1, 2], &candidates));
        assert!(loft_sections_compatible(&[2, 1], &candidates));
        assert!(!loft_sections_compatible(&[1, 3], &candidates));
        assert!(!loft_sections_compatible(&[1, 4], &candidates));
        assert!(!loft_sections_compatible(&[1, 99], &candidates));
    }
    use cadx_core::topology::PrimitiveFace;

    fn edge(feature_id: FeatureId, first: PrimitiveFace, second: PrimitiveFace) -> EdgeRef {
        EdgeRef::new(
            feature_id,
            FaceRef::primitive(feature_id, first),
            FaceRef::primitive(feature_id, second),
            0,
        )
    }

    #[test]
    fn edge_multi_selection_replaces_toggles_and_stays_on_one_feature() {
        let first = edge(1, PrimitiveFace::BoxXMax, PrimitiveFace::BoxZMax);
        let second = edge(1, PrimitiveFace::BoxXMin, PrimitiveFace::BoxZMin);
        let other_body = edge(2, PrimitiveFace::BoxXMax, PrimitiveFace::BoxZMax);
        let mut selected = Vec::new();

        update_edge_selection(&mut selected, Some(&first), false);
        assert_eq!(selected, std::slice::from_ref(&first));
        update_edge_selection(&mut selected, Some(&second), true);
        assert_eq!(selected.len(), 2);
        update_edge_selection(&mut selected, Some(&first), true);
        assert_eq!(selected, [second]);
        update_edge_selection(&mut selected, Some(&other_body), true);
        assert_eq!(selected, [other_body]);
        update_edge_selection(&mut selected, None, false);
        assert!(selected.is_empty());
    }

    #[test]
    fn edge_modifier_tools_follow_kernel_capabilities() {
        let single_only = EdgeModifierCapability {
            edge_count: EdgeCountSupport::Single,
            ..EdgeModifierCapability::default()
        };
        assert!(!edge_modifier_tool_enabled(single_only, 0));
        assert!(edge_modifier_tool_enabled(single_only, 1));
        assert!(!edge_modifier_tool_enabled(single_only, 2));

        let multiple = EdgeModifierCapability {
            edge_count: EdgeCountSupport::Multiple,
            ..single_only
        };
        assert!(edge_modifier_tool_enabled(multiple, 2));
        assert!(!edge_modifier_tool_enabled(
            EdgeModifierCapability::default(),
            1
        ));
    }

    #[test]
    fn advanced_constraint_pair_candidates_are_distinct_and_angle_ordered() {
        let profile =
            SketchLoop2D::from_polygon(vec![[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]]);
        let lines = sketch_segments_matching(&profile, &[], false);
        assert_eq!(lines, [0, 1, 2, 3]);
        let distinct = sketch_distinct_pairs(&lines);
        assert_eq!(distinct.len(), 6);
        assert!(distinct.iter().all(|(first, second)| first < second));
        let ordered = sketch_ordered_pairs(&lines);
        assert_eq!(ordered.len(), 12);
        assert!(ordered.contains(&(0, 1)));
        assert!(ordered.contains(&(1, 0)));
        assert_eq!(
            sketch_directed_angle_degrees(&profile.segments, 0, 1),
            Some(90.0)
        );
        assert_eq!(
            sketch_directed_angle_degrees(&profile.segments, 1, 0),
            Some(-90.0)
        );
    }

    #[test]
    fn curvature_candidates_are_only_adjacent_arc_pairs() {
        let profile = SketchLoop2D {
            segments: vec![
                SketchSegment2D::Arc {
                    start: [0.0, 0.0],
                    end: [2.0, 2.0],
                    center: [0.0, 2.0],
                    ccw: true,
                },
                SketchSegment2D::Arc {
                    start: [2.0, 2.0],
                    end: [4.0, 4.0],
                    center: [4.0, 2.0],
                    ccw: false,
                },
                SketchSegment2D::Line {
                    start: [4.0, 4.0],
                    end: [0.0, 4.0],
                },
                SketchSegment2D::Line {
                    start: [0.0, 4.0],
                    end: [0.0, 0.0],
                },
            ],
        };
        let options = SketchConstraintOptions::new(&profile, &[]);

        assert_eq!(options.curvature_pairs, [(0, 1)]);
        assert!(options.tangent_pairs.contains(&(0, 1)));
        assert!(options.tangent_pairs.contains(&(1, 2)));
        assert!(options.tangent_pairs.contains(&(0, 3)));
    }

    #[test]
    fn freeform_segments_use_curve_specific_constraint_candidates() {
        let profile = SketchLoop2D {
            segments: vec![
                SketchSegment2D::CubicBezier {
                    start: [0.0, 0.0],
                    control1: [1.0, 1.0],
                    control2: [2.0, 2.0],
                    end: [3.0, 3.0],
                },
                SketchSegment2D::RationalQuadratic {
                    start: [3.0, 3.0],
                    control: [4.0, 4.0],
                    end: [5.0, 3.0],
                    weight: 0.8,
                },
                SketchSegment2D::Line {
                    start: [5.0, 3.0],
                    end: [5.0, 0.0],
                },
                SketchSegment2D::Line {
                    start: [5.0, 0.0],
                    end: [0.0, 0.0],
                },
            ],
        };
        let options = SketchConstraintOptions::new(&profile, &[]);

        assert_eq!(options.line_segments, [2, 3]);
        assert!(options.arc_segments.is_empty());
        assert_eq!(options.curvature_pairs, [(0, 1)]);
        assert!(options.tangent_pairs.contains(&(0, 1)));
        assert!(options.tangent_pairs.contains(&(1, 2)));
        assert!(
            options
                .point_curve_pairs
                .iter()
                .any(|(_, segment)| *segment == 0)
        );
        assert!(
            !options
                .line_pairs
                .iter()
                .any(|(first, second)| *first < 2 || *second < 2)
        );
    }

    #[test]
    fn construction_constraint_candidates_use_appended_entity_ids() {
        let profile =
            SketchLoop2D::from_polygon(vec![[0.0, 0.0], [10.0, 0.0], [10.0, 5.0], [0.0, 5.0]]);
        let construction = vec![
            SketchSegment2D::Line {
                start: [-2.0, 2.5],
                end: [12.0, 2.5],
            },
            SketchSegment2D::Arc {
                start: [8.0, 2.5],
                end: [2.0, 2.5],
                center: [5.0, 2.5],
                ccw: true,
            },
        ];
        let options = SketchConstraintOptions::new(&profile, &construction);
        assert_eq!(options.all_points, (0..8).collect::<Vec<_>>());
        assert_eq!(options.line_segments, [0, 1, 2, 3, 4]);
        assert_eq!(options.arc_segments, [5]);
        assert!(options.line_pairs.contains(&(0, 4)));
        assert!(!options.point_curve_pairs.contains(&(4, 4)));
        assert!(!options.point_curve_pairs.contains(&(5, 4)));
        assert!(options.point_curve_pairs.contains(&(4, 5)));
        assert!(!options.midpoint_pairs.contains(&(4, 4)));
        assert!(options.midpoint_pairs.contains(&(6, 4)));
        assert_eq!(options.point_pairs.len(), 28);
        assert_eq!(options.dimension_point_pairs.len(), 56);
        assert!(options.dimension_point_pairs.contains(&(0, 7)));
        assert!(options.dimension_point_pairs.contains(&(7, 0)));
        assert!(options.point_line_pairs.contains(&(4, 4)));
        assert!(!options.point_line_pairs.iter().any(|(_, line)| *line == 5));
        assert_eq!(
            options.line_arc_pairs,
            vec![(0, 5), (1, 5), (2, 5), (3, 5), (4, 5)]
        );
        assert!(
            (point_line_distance(options.point(0).unwrap(), options.segment(4).unwrap()) - 2.5)
                .abs()
                < 1.0e-12
        );
        assert_eq!(display_constraint_indices(&[0, 2, 5]), "1, 3, 6");
    }

    #[test]
    fn point_dimension_tools_reference_installed_icons() {
        for icon in [
            "move-horizontal",
            "move-vertical",
            "between-horizontal-start",
            "circle-dot-dashed",
        ] {
            let _ = appearance::icon(icon, 14.0);
        }
    }

    #[test]
    fn viewport_dimension_edits_preserve_variant_and_value_domain() {
        let mut constraints = vec![
            Constraint::HorizontalDistance {
                first: 0,
                second: 1,
                distance: 10.0,
            },
            Constraint::Radius {
                segment: 4,
                radius: 3.0,
            },
            Constraint::Horizontal { segment: 0 },
        ];

        assert!(replace_sketch_dimension(
            &mut constraints,
            0,
            SketchDimensionKind::HorizontalDistance,
            -4.5,
        ));
        assert_eq!(
            constraints[0],
            Constraint::HorizontalDistance {
                first: 0,
                second: 1,
                distance: -4.5,
            }
        );
        assert!(!replace_sketch_dimension(
            &mut constraints,
            1,
            SketchDimensionKind::Radius,
            0.0,
        ));
        assert!((constraints[1].dimension().unwrap().value - 3.0).abs() < f64::EPSILON);
        assert!(!replace_sketch_dimension(
            &mut constraints,
            1,
            SketchDimensionKind::Length,
            4.0,
        ));
        assert!(!replace_sketch_dimension(
            &mut constraints,
            2,
            SketchDimensionKind::Length,
            4.0,
        ));
        assert!(!replace_sketch_dimension(
            &mut constraints,
            99,
            SketchDimensionKind::Length,
            4.0,
        ));
    }

    #[test]
    fn sketch_on_face_accepts_only_resolved_planar_faces() {
        let mut document = CadDocument::default();
        let cylinder = document
            .apply(ModelCommand::CreateCylinder {
                name: "support".into(),
                radius: 5.0,
                height: 10.0,
                position: [0.0; 3],
            })
            .unwrap()
            .unwrap();
        let scene = TruckKernel::default().evaluate(&document).unwrap();
        let planar = FaceRef::primitive(cylinder, PrimitiveFace::StartCap);
        let curved = FaceRef::primitive(cylinder, PrimitiveFace::Lateral);
        let lost = FaceRef::primitive(cylinder, PrimitiveFace::Patch { index: 99 });

        assert_eq!(planar_face_selection(&scene, Some(&planar)), Some(planar));
        assert!(planar_face_selection(&scene, Some(&curved)).is_none());
        assert!(planar_face_selection(&scene, Some(&lost)).is_none());
        assert!(planar_face_selection(&scene, None).is_none());
    }

    #[test]
    fn suggested_sketch_holes_stay_inside_concave_profiles_and_avoid_existing_holes() {
        let mut region = SketchRegion2D::from_polygons(
            vec![
                [0.0, 0.0],
                [20.0, 0.0],
                [20.0, 5.0],
                [5.0, 5.0],
                [5.0, 20.0],
                [0.0, 20.0],
            ],
            Vec::new(),
        );
        let first = suggest_sketch_hole(&region).unwrap();
        assert!(first.segments.iter().all(SketchSegment2D::is_arc));
        region.holes.push(first);
        region.validate().unwrap();

        let second = suggest_sketch_hole(&region).unwrap();
        assert!(second.segments.iter().all(SketchSegment2D::is_arc));
        region.holes.push(second);
        region.validate().unwrap();
    }
}
