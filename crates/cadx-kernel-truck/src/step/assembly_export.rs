use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt::Write,
};

use cadx_core::{
    assembly::{
        Assembly, AssemblyDefinitionBody, AssemblyId, AssemblyTransform, ComponentDefinitionId,
        ComponentOccurrence, ComponentOccurrenceId,
    },
    domain::{CadDocument, FeatureId},
    kernel::KernelError,
};
use truck_stepio::r#in::ruststep::ast::EntityInstance;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepExportBodyOwner {
    AssemblyDefinition(AssemblyDefinitionBody),
    Standalone(FeatureId),
}

pub(crate) struct AssemblyStepExportPlan {
    active_occurrences: BTreeMap<AssemblyId, BTreeSet<ComponentOccurrenceId>>,
    representative_bodies: HashMap<FeatureId, AssemblyDefinitionBody>,
    assembly_features: HashMap<FeatureId, AssemblyDefinitionBody>,
}

impl AssemblyStepExportPlan {
    pub(crate) fn new(document: &CadDocument) -> Result<Self, KernelError> {
        let assembly_features = document
            .assembly_feature_instances()
            .into_iter()
            .map(|(feature, instance)| (feature, instance.definition_body()))
            .collect::<HashMap<_, _>>();
        let mut active_occurrences = BTreeMap::new();
        let mut representative_bodies = HashMap::new();

        for assembly in &document.assemblies {
            let suppression = assembly
                .effective_suppression()
                .map_err(cadx_core::domain::DocumentError::from)?;
            let active = assembly
                .occurrences
                .iter()
                .filter(|occurrence| !suppression[&occurrence.id])
                .map(|occurrence| occurrence.id)
                .collect::<BTreeSet<_>>();
            if active.is_empty() {
                continue;
            }
            validate_definition_instances(document, assembly, &active)?;
            for definition in &assembly.definitions {
                let Some(representative) = assembly.occurrences.iter().find(|occurrence| {
                    active.contains(&occurrence.id) && occurrence.definition_id == definition.id
                }) else {
                    continue;
                };
                for (body_slot, feature_id) in representative.feature_ids.iter().enumerate() {
                    representative_bodies.insert(
                        *feature_id,
                        AssemblyDefinitionBody {
                            assembly_id: assembly.id,
                            definition_id: definition.id,
                            body_slot,
                        },
                    );
                }
            }
            active_occurrences.insert(assembly.id, active);
        }

        Ok(Self {
            active_occurrences,
            representative_bodies,
            assembly_features,
        })
    }

    pub(crate) fn has_active_assemblies(&self) -> bool {
        !self.active_occurrences.is_empty()
    }

    pub(crate) fn body_owner(&self, feature_id: FeatureId) -> Option<StepExportBodyOwner> {
        if let Some(definition) = self.representative_bodies.get(&feature_id) {
            return Some(StepExportBodyOwner::AssemblyDefinition(*definition));
        }
        (!self.assembly_features.contains_key(&feature_id))
            .then_some(StepExportBodyOwner::Standalone(feature_id))
    }
}

fn validate_definition_instances(
    document: &CadDocument,
    assembly: &Assembly,
    active: &BTreeSet<ComponentOccurrenceId>,
) -> Result<(), KernelError> {
    let mut occurrences_by_definition = BTreeMap::<_, Vec<_>>::new();
    for occurrence in &assembly.occurrences {
        if active.contains(&occurrence.id) {
            occurrences_by_definition
                .entry(occurrence.definition_id)
                .or_default()
                .push(occurrence);
        }
    }

    for (definition_id, occurrences) in occurrences_by_definition {
        let representative = occurrences[0];
        let representative_children = active_children(assembly, active, representative.id);
        for occurrence in occurrences.into_iter().skip(1) {
            if occurrence.feature_ids.len() != representative.feature_ids.len() {
                return Err(unrepresentable_definition(assembly.id, definition_id));
            }
            for (reference, candidate) in representative
                .feature_ids
                .iter()
                .zip(&occurrence.feature_ids)
            {
                let reference =
                    document
                        .feature(*reference)
                        .ok_or_else(|| KernelError::Exchange {
                            format: "STEP",
                            message: format!(
                                "assembly export references missing feature {reference}"
                            ),
                        })?;
                let candidate =
                    document
                        .feature(*candidate)
                        .ok_or_else(|| KernelError::Exchange {
                            format: "STEP",
                            message: format!(
                                "assembly export references missing feature {candidate}"
                            ),
                        })?;
                if reference.primitive != candidate.primitive
                    || reference.visible != candidate.visible
                    || reference
                        .color
                        .iter()
                        .zip(candidate.color)
                        .any(|(reference, candidate)| reference.to_bits() != candidate.to_bits())
                {
                    return Err(unrepresentable_definition(assembly.id, definition_id));
                }
            }

            let candidate_children = active_children(assembly, active, occurrence.id);
            if representative_children.len() != candidate_children.len()
                || representative_children.iter().zip(candidate_children).any(
                    |(reference, candidate)| {
                        reference.definition_id != candidate.definition_id
                            || reference.name != candidate.name
                            || !reference
                                .transform
                                .approximately_equals(candidate.transform, 1.0e-8)
                    },
                )
            {
                return Err(unrepresentable_definition(assembly.id, definition_id));
            }
        }
    }
    Ok(())
}

fn active_children<'a>(
    assembly: &'a Assembly,
    active: &BTreeSet<ComponentOccurrenceId>,
    parent: ComponentOccurrenceId,
) -> Vec<&'a ComponentOccurrence> {
    assembly
        .occurrences
        .iter()
        .filter(|occurrence| {
            occurrence.parent_id == Some(parent) && active.contains(&occurrence.id)
        })
        .collect()
}

fn unrepresentable_definition(
    assembly: AssemblyId,
    definition: ComponentDefinitionId,
) -> KernelError {
    KernelError::Exchange {
        format: "STEP",
        message: format!(
            "assembly {assembly} definition {definition} has occurrence-specific geometry, visibility, color, or child structure that one reusable AP242 product definition cannot represent"
        ),
    }
}

#[derive(Debug, Clone, Copy)]
struct ProductDefinitionStepIds {
    product_definition: u64,
    shape_representation: u64,
}

struct EntityWriter {
    next_id: u64,
    output: String,
}

impl EntityWriter {
    fn new(next_id: u64) -> Self {
        Self {
            next_id,
            output: String::new(),
        }
    }

    fn allocate(&mut self) -> Result<u64, KernelError> {
        let id = self.next_id;
        self.next_id = id.checked_add(1).ok_or_else(|| KernelError::Exchange {
            format: "STEP",
            message: "generated model exhausted STEP entity ids while exporting AP242 structure"
                .into(),
        })?;
        Ok(id)
    }

    fn entity(&mut self, body: impl std::fmt::Display) -> Result<u64, KernelError> {
        let id = self.allocate()?;
        writeln!(&mut self.output, "#{id}={body};")
            .expect("writing a STEP entity to a String cannot fail");
        Ok(id)
    }
}

pub(crate) fn append_ap242_product_structure(
    source: String,
    document: &CadDocument,
    plan: &AssemblyStepExportPlan,
    owners: &[StepExportBodyOwner],
    body_targets: &[u64],
) -> Result<String, KernelError> {
    if !plan.has_active_assemblies() {
        return Ok(source);
    }
    if owners.len() != body_targets.len() {
        return Err(KernelError::Exchange {
            format: "STEP",
            message: "generated STEP body ownership does not match exact solid records".into(),
        });
    }

    let mut definition_bodies = BTreeMap::<(AssemblyId, ComponentDefinitionId), Vec<u64>>::new();
    let mut standalone_bodies = BTreeMap::<FeatureId, Vec<u64>>::new();
    for (owner, target) in owners.iter().zip(body_targets) {
        match owner {
            StepExportBodyOwner::AssemblyDefinition(definition) => definition_bodies
                .entry((definition.assembly_id, definition.definition_id))
                .or_default()
                .push(*target),
            StepExportBodyOwner::Standalone(feature) => {
                standalone_bodies.entry(*feature).or_default().push(*target);
            }
        }
    }

    let exchange = truck_stepio::r#in::ruststep::parser::parse(&source).map_err(|error| {
        KernelError::Exchange {
            format: "STEP",
            message: format!("generated model could not be parsed for AP242 export: {error}"),
        }
    })?;
    let data = exchange.data.first().ok_or_else(|| KernelError::Exchange {
        format: "STEP",
        message: "generated model has no DATA section for AP242 export".into(),
    })?;
    let maximum_id = data.entities.iter().map(entity_id).max().unwrap_or(15);
    let next_id = maximum_id
        .checked_add(1)
        .ok_or_else(|| KernelError::Exchange {
            format: "STEP",
            message: "generated model exhausted STEP entity ids before AP242 export".into(),
        })?;
    let mut writer = EntityWriter::new(next_id);
    let product_context = writer.entity("PRODUCT_CONTEXT('',#2,'mechanical')")?;
    let definition_context =
        writer.entity("PRODUCT_DEFINITION_CONTEXT('part definition',#2,'design')")?;
    let identity_frame = write_axis_placement(&mut writer, AssemblyTransform::IDENTITY)?;

    let mut definition_ids = BTreeMap::new();
    let mut assembly_root_ids = BTreeMap::new();
    for assembly in &document.assemblies {
        let Some(active) = plan.active_occurrences.get(&assembly.id) else {
            continue;
        };
        let root = write_product_definition(
            &mut writer,
            &format!("CADX-A{}", assembly.id),
            &assembly.name,
            &[],
            product_context,
            definition_context,
            identity_frame,
        )?;
        assembly_root_ids.insert(assembly.id, root);
        for definition in &assembly.definitions {
            if !assembly.occurrences.iter().any(|occurrence| {
                active.contains(&occurrence.id) && occurrence.definition_id == definition.id
            }) {
                continue;
            }
            let bodies = definition_bodies
                .get(&(assembly.id, definition.id))
                .map(Vec::as_slice)
                .unwrap_or_default();
            let ids = write_product_definition(
                &mut writer,
                &format!("CADX-A{}-D{}", assembly.id, definition.id),
                &definition.name,
                bodies,
                product_context,
                definition_context,
                identity_frame,
            )?;
            definition_ids.insert((assembly.id, definition.id), ids);
        }
    }

    for (feature_id, targets) in standalone_bodies {
        let feature = document
            .feature(feature_id)
            .ok_or_else(|| KernelError::Exchange {
                format: "STEP",
                message: format!(
                    "assembly export references missing standalone feature {feature_id}"
                ),
            })?;
        write_product_definition(
            &mut writer,
            &format!("CADX-F{feature_id}"),
            &feature.name,
            &targets,
            product_context,
            definition_context,
            identity_frame,
        )?;
    }

    for assembly in &document.assemblies {
        let Some(active) = plan.active_occurrences.get(&assembly.id) else {
            continue;
        };
        let root_ids = assembly_root_ids[&assembly.id];
        for occurrence in assembly
            .roots()
            .filter(|occurrence| active.contains(&occurrence.id))
        {
            let child_ids = definition_ids[&(assembly.id, occurrence.definition_id)];
            write_occurrence(
                &mut writer,
                &format!("CADX-A{}-O{}", assembly.id, occurrence.id),
                &occurrence.name,
                root_ids,
                child_ids,
                occurrence.transform,
                identity_frame,
            )?;
        }

        let mut emitted_parent_definitions = BTreeSet::new();
        for parent in &assembly.occurrences {
            if !active.contains(&parent.id)
                || !emitted_parent_definitions.insert(parent.definition_id)
            {
                continue;
            }
            let parent_ids = definition_ids[&(assembly.id, parent.definition_id)];
            for child in active_children(assembly, active, parent.id) {
                let child_ids = definition_ids[&(assembly.id, child.definition_id)];
                write_occurrence(
                    &mut writer,
                    &format!("CADX-A{}-O{}", assembly.id, child.id),
                    &child.name,
                    parent_ids,
                    child_ids,
                    child.transform,
                    identity_frame,
                )?;
            }
        }
    }

    rebuild_ap242_source(&source, &writer.output)
}

fn write_product_definition(
    writer: &mut EntityWriter,
    identifier: &str,
    name: &str,
    body_targets: &[u64],
    product_context: u64,
    definition_context: u64,
    identity_frame: u64,
) -> Result<ProductDefinitionStepIds, KernelError> {
    let identifier = encode_step_string(identifier);
    let name = encode_step_string(name);
    let product = writer.entity(format!(
        "PRODUCT('{identifier}','{name}','',(#{product_context}))"
    ))?;
    let formation = writer.entity(format!("PRODUCT_DEFINITION_FORMATION('','',#{product})"))?;
    let product_definition = writer.entity(format!(
        "PRODUCT_DEFINITION('design','',#{formation},#{definition_context})"
    ))?;
    let product_shape = writer.entity(format!(
        "PRODUCT_DEFINITION_SHAPE('','',#{product_definition})"
    ))?;
    let mut items = body_targets
        .iter()
        .map(|id| format!("#{id}"))
        .collect::<Vec<_>>();
    items.push(format!("#{identity_frame}"));
    let items = items.join(",");
    let representation_type = if body_targets.is_empty() {
        "SHAPE_REPRESENTATION"
    } else {
        "ADVANCED_BREP_SHAPE_REPRESENTATION"
    };
    let shape_representation =
        writer.entity(format!("{representation_type}('{name}',({items}),#11)"))?;
    writer.entity(format!(
        "SHAPE_DEFINITION_REPRESENTATION(#{product_shape},#{shape_representation})"
    ))?;
    Ok(ProductDefinitionStepIds {
        product_definition,
        shape_representation,
    })
}

#[allow(clippy::too_many_arguments)]
fn write_occurrence(
    writer: &mut EntityWriter,
    identifier: &str,
    name: &str,
    parent: ProductDefinitionStepIds,
    child: ProductDefinitionStepIds,
    transform: AssemblyTransform,
    identity_frame: u64,
) -> Result<(), KernelError> {
    let identifier = encode_step_string(identifier);
    let name = encode_step_string(name);
    let usage = writer.entity(format!(
        "NEXT_ASSEMBLY_USAGE_OCCURRENCE('{identifier}','{name}','',#{},#{},$)",
        parent.product_definition, child.product_definition
    ))?;
    let usage_shape = writer.entity(format!("PRODUCT_DEFINITION_SHAPE('','',#{usage})"))?;
    let parent_frame = write_axis_placement(writer, transform)?;
    let item_transform = writer.entity(format!(
        "ITEM_DEFINED_TRANSFORMATION('','',#{parent_frame},#{identity_frame})"
    ))?;
    let relationship = writer.entity(format!(
        "(REPRESENTATION_RELATIONSHIP('','',#{},#{}) REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION(#{item_transform}) SHAPE_REPRESENTATION_RELATIONSHIP())",
        parent.shape_representation, child.shape_representation
    ))?;
    writer.entity(format!(
        "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION(#{relationship},#{usage_shape})"
    ))?;
    Ok(())
}

fn write_axis_placement(
    writer: &mut EntityWriter,
    transform: AssemblyTransform,
) -> Result<u64, KernelError> {
    let [x, y, z] = transform.translation;
    let point = writer.entity(format!(
        "CARTESIAN_POINT('',({},{},{}))",
        step_number(x),
        step_number(y),
        step_number(z)
    ))?;
    let axis = writer.entity(format!(
        "DIRECTION('',({},{},{}))",
        step_number(transform.rotation[0][2]),
        step_number(transform.rotation[1][2]),
        step_number(transform.rotation[2][2])
    ))?;
    let reference = writer.entity(format!(
        "DIRECTION('',({},{},{}))",
        step_number(transform.rotation[0][0]),
        step_number(transform.rotation[1][0]),
        step_number(transform.rotation[2][0])
    ))?;
    writer.entity(format!(
        "AXIS2_PLACEMENT_3D('',#{point},#{axis},#{reference})"
    ))
}

fn step_number(value: f64) -> String {
    if value == 0.0 {
        "0.".into()
    } else {
        format!("{value:.17E}")
    }
}

fn rebuild_ap242_source(source: &str, product_structure: &str) -> Result<String, KernelError> {
    const DATA_MARKER: &str = "DATA;\n";
    const TRAILER: &str = "ENDSEC;\nEND-ISO-10303-21;\n";
    let data_start = source
        .find(DATA_MARKER)
        .ok_or_else(|| KernelError::Exchange {
            format: "STEP",
            message: "generated model has no DATA marker for AP242 export".into(),
        })?;
    let data_end =
        source
            .strip_suffix(TRAILER)
            .map(str::len)
            .ok_or_else(|| KernelError::Exchange {
                format: "STEP",
                message: "generated model has an unexpected trailer for AP242 export".into(),
            })?;
    let mut header = source[..data_start + DATA_MARKER.len()].to_owned();
    header = header.replace(
        "FILE_DESCRIPTION(('CADX B-Rep model'), '2;1');",
        "FILE_DESCRIPTION(('CADX AP242 assembly model'), '2;1');",
    );
    header = header.replace(
        "FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));",
        "FILE_SCHEMA(('AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF { 1 0 10303 442 3 1 4 }'));",
    );

    let data = &source[data_start + DATA_MARKER.len()..data_end];
    let mut retained = String::new();
    for statement in data.split_inclusive(';') {
        let trimmed = statement.trim_start();
        let generated_wrapper = trimmed.strip_prefix('#').and_then(|rest| {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            rest[..digits].parse::<u64>().ok()
        });
        if generated_wrapper.is_some_and(|id| id <= 10) {
            continue;
        }
        retained.push_str(statement);
    }

    let mut result = header;
    result.push_str("#1=APPLICATION_PROTOCOL_DEFINITION('international standard','ap242_managed_model_based_3d_engineering',2014,#2);\n");
    result.push_str("#2=APPLICATION_CONTEXT('managed model based 3d engineering');\n");
    result.push_str(&retained);
    if !retained.ends_with('\n') {
        result.push('\n');
    }
    result.push_str(product_structure);
    result.push_str(TRAILER);
    Ok(result)
}

fn entity_id(entity: &EntityInstance) -> u64 {
    match entity {
        EntityInstance::Simple { id, .. } | EntityInstance::Complex { id, .. } => *id,
    }
}

fn encode_step_string(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut unicode = false;
    for character in value.chars() {
        let printable_ascii = character.is_ascii() && !character.is_ascii_control();
        if printable_ascii {
            if unicode {
                output.push_str("\\X0\\");
                unicode = false;
            }
            match character {
                '\'' => output.push('_'),
                '\\' => output.push_str("\\\\"),
                _ => output.push(character),
            }
        } else {
            if !unicode {
                output.push_str("\\X2\\");
                unicode = true;
            }
            let mut units = [0; 2];
            for unit in character.encode_utf16(&mut units) {
                write!(&mut output, "{unit:04X}")
                    .expect("writing STEP text to a String cannot fail");
            }
        }
    }
    if unicode {
        output.push_str("\\X0\\");
    }
    output
}
