//! Ports of `StructureTemplatePool`, `StructurePoolElement`, and the
//! one-element jigsaw expansion path.
//!
//! The catalog stores all 188 shipped `worldgen/template_pool` documents in
//! compact, sorted JSON. Parsing covers `single_pool_element`,
//! `legacy_single_pool_element`, `feature_pool_element`,
//! `empty_pool_element`, and `list_pool_element`, including projection,
//! per-entry weight, fallback, processor-list references, and
//! `override_liquid_settings`. Unknown element types fail explicitly.
//!
//! [`StructureTemplatePool::weighted_element`] builds Java's expanded
//! `templates` list (each entry repeated `weight` times) and draws one
//! `nextInt(size)`, so the produced sequence is identical to
//! `StructureTemplatePool.getRandomTemplate`.
//!
//! [`PoolCatalog::expand_start`] resolves a start pool into one
//! [`PoolExpansion`] and [`PoolExpansion::place`] writes that element through
//! the existing [`TemplateStructurePiece`] geometry. Recursive jigsaw
//! connection, junction/expansion-hack placement, terrain adaptation, and
//! `FeaturePoolElement` world placement remain explicit later slices.

use crate::nbt::Tag;
use crate::worldgen::feature::FeatureLevel;
use crate::worldgen::structure::piece::{
    BoundingBox, PiecePlacement, StructureDataMarker, StructurePiece, TemplateStructurePiece,
    template_bounding_box,
};
use crate::worldgen::structure::processor::{
    ProcessorApplyContext, StructureProcessor, StructureProcessorCatalog, StructureProcessorList,
    apply_processors, template_block_infos,
};
use crate::worldgen::structure::start::TemplateProvider;
use crate::worldgen::structure::template::{
    Mirror, PlacedBlock, PlacedEntity, Rotation, StructurePlaceSettings, transform,
    transform_state, transform_vec3,
};
use folia_math::BlockPos;
use folia_random::RandomSource;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

const TARGET_VERSION: &str = "1.21.11";
const TARGET_DATA_VERSION: i32 = 4788;
const SHIPPED_POOLS: &str = include_str!("../../../assets/template_pools.json");

/// `StructureTemplatePool.Projection`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Projection {
    TerrainMatching,
    Rigid,
}

impl Projection {
    pub const ALL: [Projection; 2] = [Projection::TerrainMatching, Projection::Rigid];

    pub fn name(self) -> &'static str {
        match self {
            Projection::TerrainMatching => "terrain_matching",
            Projection::Rigid => "rigid",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|projection| projection.name() == name)
    }

    /// `Projection.getProcessors`: terrain matching adds a `gravity` processor
    /// with `WORLD_SURFACE_WG` and offset -1; rigid has none.
    pub fn processors(self) -> Vec<StructureProcessor> {
        match self {
            Projection::TerrainMatching => vec![StructureProcessor::Gravity {
                heightmap: crate::worldgen::feature::Heightmap::WorldSurfaceWg,
                offset: -1,
            }],
            Projection::Rigid => Vec::new(),
        }
    }
}

/// `LiquidSettings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiquidSettings {
    IgnoreWaterlogging,
    ApplyWaterlogging,
}

impl LiquidSettings {
    pub fn name(self) -> &'static str {
        match self {
            LiquidSettings::IgnoreWaterlogging => "ignore_waterlogging",
            LiquidSettings::ApplyWaterlogging => "apply_waterlogging",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "ignore_waterlogging" => Some(LiquidSettings::IgnoreWaterlogging),
            "apply_waterlogging" => Some(LiquidSettings::ApplyWaterlogging),
            _ => None,
        }
    }
}

/// A pool element's processor field: a registry reference or an inline list.
#[derive(Debug, Clone, PartialEq)]
pub enum ProcessorListRef {
    Named(String),
    Inline(StructureProcessorList),
}

impl ProcessorListRef {
    pub fn from_value(value: &Value, context: &str) -> Result<Self, PoolError> {
        match value {
            Value::String(name) => Ok(ProcessorListRef::Named(normalized_id(name))),
            Value::Array(_) | Value::Object(_) => Ok(ProcessorListRef::Inline(
                StructureProcessorList::from_value(context, value)
                    .map_err(|error| PoolError::InvalidDefinition(error.to_string()))?,
            )),
            _ => Err(PoolError::InvalidDefinition(format!(
                "{context} processors must be an id or an inline list"
            ))),
        }
    }

    pub fn resolve<'a>(
        &'a self,
        catalog: &'a StructureProcessorCatalog,
    ) -> Result<&'a [StructureProcessor], PoolError> {
        match self {
            ProcessorListRef::Named(name) => catalog
                .list(name)
                .map(StructureProcessorList::processors)
                .ok_or_else(|| PoolError::UnknownProcessorList { list: name.clone() }),
            ProcessorListRef::Inline(list) => Ok(list.processors()),
        }
    }
}

/// A parsed `StructurePoolElement`.
#[derive(Debug, Clone, PartialEq)]
pub enum PoolElement {
    Single {
        template: String,
        processors: ProcessorListRef,
        projection: Projection,
        liquid_settings: Option<LiquidSettings>,
    },
    LegacySingle {
        template: String,
        processors: ProcessorListRef,
        projection: Projection,
        liquid_settings: Option<LiquidSettings>,
    },
    Feature {
        feature: String,
        projection: Projection,
    },
    Empty,
    List {
        elements: Vec<PoolElement>,
        projection: Projection,
    },
}

impl PoolElement {
    pub fn from_json(value: &Value) -> Result<Self, PoolError> {
        let object = value.as_object().ok_or_else(|| {
            PoolError::InvalidDefinition("pool element must be an object".to_owned())
        })?;
        let element_type = normalized_id(required_str(object, "element_type", "pool element")?);
        match element_type.as_str() {
            "minecraft:single_pool_element" => Ok(PoolElement::Single {
                template: normalized_id(required_str(object, "location", "single element")?),
                processors: ProcessorListRef::from_value(
                    required(object, "processors", "single element")?,
                    "single element",
                )?,
                projection: parse_projection(object, "single element")?,
                liquid_settings: parse_liquid_settings(object, "single element")?,
            }),
            "minecraft:legacy_single_pool_element" => Ok(PoolElement::LegacySingle {
                template: normalized_id(required_str(object, "location", "legacy single element")?),
                processors: ProcessorListRef::from_value(
                    required(object, "processors", "legacy single element")?,
                    "legacy single element",
                )?,
                projection: parse_projection(object, "legacy single element")?,
                liquid_settings: parse_liquid_settings(object, "legacy single element")?,
            }),
            "minecraft:feature_pool_element" => Ok(PoolElement::Feature {
                feature: normalized_id(required_str(object, "feature", "feature element")?),
                projection: parse_projection(object, "feature element")?,
            }),
            "minecraft:empty_pool_element" => Ok(PoolElement::Empty),
            "minecraft:list_pool_element" => {
                let projection = parse_projection(object, "list element")?;
                let entries = required(object, "elements", "list element")?
                    .as_array()
                    .ok_or_else(|| {
                        PoolError::InvalidDefinition(
                            "list element `elements` must be an array".to_owned(),
                        )
                    })?;
                if entries.is_empty() {
                    return Err(PoolError::InvalidDefinition(
                        "list element must contain at least one element".to_owned(),
                    ));
                }
                let mut elements = entries
                    .iter()
                    .map(PoolElement::from_json)
                    .collect::<Result<Vec<_>, _>>()?;
                // `ListPoolElement` forces its projection onto every child.
                for element in &mut elements {
                    element.set_projection(projection);
                }
                Ok(PoolElement::List {
                    elements,
                    projection,
                })
            }
            other => Err(PoolError::UnknownElementType {
                type_name: other.to_owned(),
            }),
        }
    }

    pub fn element_type(&self) -> &'static str {
        match self {
            PoolElement::Single { .. } => "minecraft:single_pool_element",
            PoolElement::LegacySingle { .. } => "minecraft:legacy_single_pool_element",
            PoolElement::Feature { .. } => "minecraft:feature_pool_element",
            PoolElement::Empty => "minecraft:empty_pool_element",
            PoolElement::List { .. } => "minecraft:list_pool_element",
        }
    }

    pub fn projection(&self) -> Projection {
        match self {
            PoolElement::Single { projection, .. }
            | PoolElement::LegacySingle { projection, .. }
            | PoolElement::Feature { projection, .. }
            | PoolElement::List { projection, .. } => *projection,
            // `EmptyPoolElement` is always TERRAIN_MATCHING in Java.
            PoolElement::Empty => Projection::TerrainMatching,
        }
    }

    pub fn set_projection(&mut self, projection: Projection) {
        match self {
            PoolElement::Single {
                projection: current,
                ..
            }
            | PoolElement::LegacySingle {
                projection: current,
                ..
            }
            | PoolElement::Feature {
                projection: current,
                ..
            }
            | PoolElement::List {
                projection: current,
                ..
            } => *current = projection,
            PoolElement::Empty => {}
        }
    }

    /// `SinglePoolElement.getTemplateLocation`.
    pub fn template_location(&self) -> Option<&str> {
        match self {
            PoolElement::Single { template, .. } | PoolElement::LegacySingle { template, .. } => {
                Some(template)
            }
            _ => None,
        }
    }

    pub fn processors(&self) -> Option<&ProcessorListRef> {
        match self {
            PoolElement::Single { processors, .. }
            | PoolElement::LegacySingle { processors, .. } => Some(processors),
            _ => None,
        }
    }

    pub fn feature(&self) -> Option<&str> {
        match self {
            PoolElement::Feature { feature, .. } => Some(feature),
            _ => None,
        }
    }

    pub fn children(&self) -> Option<&[PoolElement]> {
        match self {
            PoolElement::List { elements, .. } => Some(elements),
            _ => None,
        }
    }
}

/// `StructureTemplatePool.StructureSelectionEntry`.
#[derive(Debug, Clone, PartialEq)]
pub struct PoolElementEntry {
    pub element: PoolElement,
    pub weight: i32,
}

/// `StructureTemplatePool`.
#[derive(Debug, Clone, PartialEq)]
pub struct StructureTemplatePool {
    name: String,
    fallback: String,
    entries: Vec<PoolElementEntry>,
    /// Java's expanded `templates` list: entry indices repeated by weight.
    templates: Vec<usize>,
}

impl StructureTemplatePool {
    pub fn from_value(name: &str, value: &Value) -> Result<Self, PoolError> {
        let object = value.as_object().ok_or_else(|| {
            PoolError::InvalidDefinition(format!("template pool `{name}` must be an object"))
        })?;
        let fallback = normalized_id(required_str(object, "fallback", "template pool")?);
        let elements = required(object, "elements", "template pool")?
            .as_array()
            .ok_or_else(|| {
                PoolError::InvalidDefinition(format!(
                    "template pool `{name}` elements must be an array"
                ))
            })?;

        let mut entries = Vec::with_capacity(elements.len());
        let mut templates = Vec::new();
        for (index, entry) in elements.iter().enumerate() {
            let entry = entry.as_object().ok_or_else(|| {
                PoolError::InvalidDefinition(format!(
                    "template pool `{name}` entry {index} must be an object"
                ))
            })?;
            let element = PoolElement::from_json(required(entry, "element", "pool entry")?)
                .map_err(|error| {
                    PoolError::InvalidDefinition(format!(
                        "template pool `{name}` entry {index}: {error}"
                    ))
                })?;
            let weight = required(entry, "weight", "pool entry")?
                .as_i64()
                .ok_or_else(|| {
                    PoolError::InvalidDefinition(format!(
                        "template pool `{name}` entry {index} weight must be an integer"
                    ))
                })?;
            let weight = i32::try_from(weight).map_err(|_| {
                PoolError::InvalidDefinition(format!(
                    "template pool `{name}` entry {index} weight is outside the int range"
                ))
            })?;
            if !(1..=150).contains(&weight) {
                return Err(PoolError::InvalidDefinition(format!(
                    "template pool `{name}` entry {index} weight {weight} is outside 1..=150"
                )));
            }
            for _ in 0..weight {
                templates.push(entries.len());
            }
            entries.push(PoolElementEntry { element, weight });
        }

        Ok(Self {
            name: name.to_owned(),
            fallback,
            entries,
            templates,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn fallback(&self) -> &str {
        &self.fallback
    }

    pub fn entries(&self) -> &[PoolElementEntry] {
        &self.entries
    }

    /// `StructureTemplatePool.size`: the number of expanded template slots.
    pub fn size(&self) -> usize {
        self.templates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }

    /// `StructureTemplatePool.getRandomTemplate`.
    ///
    /// `None` means Java's `EmptyPoolElement.INSTANCE` fallback for an empty
    /// pool. A parsed empty element is a distinct `Some(PoolElement::Empty)`.
    pub fn weighted_element(&self, random: &mut impl RandomSource) -> Option<&PoolElement> {
        if self.templates.is_empty() {
            return None;
        }
        let index = random.next_int_bound(self.templates.len() as i32) as usize;
        let entry = self.templates[index];
        Some(&self.entries[entry].element)
    }

    /// Alias matching Java's method name.
    pub fn get_random_template(&self, random: &mut impl RandomSource) -> Option<&PoolElement> {
        self.weighted_element(random)
    }
}

/// Header of the generated template-pool catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolCatalogIndex {
    pub version: String,
    pub data_version: i32,
    pub pools: usize,
    pub entries: usize,
    pub elements: usize,
    pub templates: usize,
    pub by_element_type: BTreeMap<String, usize>,
    pub by_projection: BTreeMap<String, usize>,
}

/// The embedded shipped template-pool catalog.
#[derive(Debug, Clone)]
pub struct PoolCatalog {
    index: PoolCatalogIndex,
    pools: BTreeMap<String, StructureTemplatePool>,
}

static POOL_CATALOG: OnceLock<PoolCatalog> = OnceLock::new();

impl PoolCatalog {
    pub fn from_json(text: &str) -> Result<Self, PoolError> {
        let value: Value = serde_json::from_str(text).map_err(|error| {
            PoolError::InvalidDefinition(format!("invalid template-pool catalog JSON: {error}"))
        })?;
        let root = value.as_object().ok_or_else(|| {
            PoolError::InvalidDefinition("template-pool catalog root must be an object".to_owned())
        })?;

        let version = required_str(root, "version", "template-pool catalog")?.to_owned();
        if version != TARGET_VERSION {
            return Err(PoolError::InvalidDefinition(format!(
                "template-pool catalog version {version} does not match target {TARGET_VERSION}"
            )));
        }
        let data_version = required_i32(root, "data_version", "template-pool catalog")?;
        if data_version != TARGET_DATA_VERSION {
            return Err(PoolError::InvalidDefinition(format!(
                "template-pool catalog data version {data_version} does not match target \
                 {TARGET_DATA_VERSION}"
            )));
        }

        let counts = required_object(root, "counts", "template-pool catalog")?;
        let pools_count = required_usize(counts, "pools", "template-pool catalog counts")?;
        let entries_count = required_usize(counts, "entries", "template-pool catalog counts")?;
        let elements_count = required_usize(counts, "elements", "template-pool catalog counts")?;
        let templates_count = required_usize(counts, "templates", "template-pool catalog counts")?;
        let by_element_type = count_map(required_object(
            counts,
            "by_element_type",
            "template-pool counts",
        )?)?;
        let by_projection = count_map(required_object(
            counts,
            "by_projection",
            "template-pool counts",
        )?)?;

        let pools_object = required_object(root, "template_pools", "template-pool catalog")?;
        let mut pools = BTreeMap::new();
        for (name, value) in pools_object {
            let name = normalized_id(name);
            pools.insert(
                name.clone(),
                StructureTemplatePool::from_value(&name, value)?,
            );
        }

        if pools.len() != pools_count {
            return Err(PoolError::InvalidDefinition(format!(
                "template-pool catalog has {} pools but index reports {pools_count}",
                pools.len()
            )));
        }
        let parsed_entries: usize = pools.values().map(|pool| pool.entries.len()).sum();
        if parsed_entries != entries_count {
            return Err(PoolError::InvalidDefinition(format!(
                "template-pool catalog has {parsed_entries} entries but index reports \
                 {entries_count}"
            )));
        }
        let parsed_templates: usize = pools.values().map(StructureTemplatePool::size).sum();
        if parsed_templates != templates_count {
            return Err(PoolError::InvalidDefinition(format!(
                "template-pool catalog has {parsed_templates} template slots but index reports \
                 {templates_count}"
            )));
        }
        let parsed_elements: usize = by_element_type.values().sum();
        if parsed_elements != elements_count {
            return Err(PoolError::InvalidDefinition(format!(
                "template-pool catalog by_element_type sum {parsed_elements} does not match \
                 {elements_count}"
            )));
        }

        Ok(Self {
            index: PoolCatalogIndex {
                version,
                data_version,
                pools: pools_count,
                entries: entries_count,
                elements: elements_count,
                templates: templates_count,
                by_element_type,
                by_projection,
            },
            pools,
        })
    }

    /// Loads the embedded 1.21.11 catalog once.
    pub fn shipped() -> &'static Self {
        POOL_CATALOG.get_or_init(|| {
            Self::from_json(SHIPPED_POOLS)
                .unwrap_or_else(|error| panic!("embedded template_pools.json is invalid: {error}"))
        })
    }

    pub fn index(&self) -> &PoolCatalogIndex {
        &self.index
    }

    pub fn pool(&self, name: &str) -> Option<&StructureTemplatePool> {
        self.pools.get(&normalized_id(name))
    }

    pub fn pools(&self) -> impl Iterator<Item = (&str, &StructureTemplatePool)> {
        self.pools.iter().map(|(name, pool)| (name.as_str(), pool))
    }

    pub fn len(&self) -> usize {
        self.pools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pools.is_empty()
    }

    /// Resolves a start pool exactly like the opening of
    /// `JigsawPlacement.addPieces`: one rotation draw, then one weighted
    /// element draw. Empty pools/elements return `None`.
    pub fn expand_start(
        &self,
        templates: &dyn TemplateProvider,
        start_pool: &str,
        position: BlockPos,
        random: &mut impl RandomSource,
    ) -> Result<Option<PoolExpansion>, PoolError> {
        let pool = self
            .pool(start_pool)
            .ok_or_else(|| PoolError::UnknownPool {
                pool: normalized_id(start_pool),
            })?;
        let rotation = Rotation::ALL[random.next_int_bound(4) as usize];
        let Some(element) = pool.weighted_element(random) else {
            return Ok(None);
        };
        if matches!(element, PoolElement::Empty) {
            return Ok(None);
        }
        let element = element.clone();
        let piece = build_template_piece(&element, templates, pool.name(), position, rotation)?;
        let bounding_box = element_bounds(&element, templates, pool.name(), position, rotation)?;
        Ok(Some(PoolExpansion {
            pool: pool.name().to_owned(),
            rotation,
            position,
            element,
            piece,
            bounding_box,
        }))
    }
}

/// One resolved pool element ready for placement.
#[derive(Debug, Clone, PartialEq)]
pub struct PoolExpansion {
    pub pool: String,
    pub rotation: Rotation,
    /// Java's `PoolElementStructurePiece.position`.
    pub position: BlockPos,
    pub element: PoolElement,
    /// Template piece for single/legacy elements. Other element kinds have no
    /// single template piece.
    pub piece: Option<StructurePiece>,
    pub bounding_box: Option<BoundingBox>,
}

impl PoolExpansion {
    /// Places this element through the template-piece path.
    ///
    /// `structure_pos` is Java's `pos` argument to `placeInWorld` (used by
    /// position predicates); `chunk_box` is the write box. Processing happens
    /// over the whole template, like Java, and only processed blocks inside
    /// `chunk_box` are written.
    #[allow(clippy::too_many_arguments)]
    pub fn place(
        &self,
        level: &mut dyn FeatureLevel,
        templates: &dyn TemplateProvider,
        processors: &StructureProcessorCatalog,
        chunk_box: BoundingBox,
        structure_pos: BlockPos,
        world_seed: i64,
        keep_jigsaws: bool,
    ) -> Result<PiecePlacement, PoolError> {
        let mut result = PiecePlacement::default();
        self.place_element(
            &self.element,
            level,
            templates,
            processors,
            chunk_box,
            structure_pos,
            world_seed,
            keep_jigsaws,
            &mut result,
        )?;
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    fn place_element(
        &self,
        element: &PoolElement,
        level: &mut dyn FeatureLevel,
        templates: &dyn TemplateProvider,
        processors: &StructureProcessorCatalog,
        chunk_box: BoundingBox,
        structure_pos: BlockPos,
        world_seed: i64,
        keep_jigsaws: bool,
        result: &mut PiecePlacement,
    ) -> Result<(), PoolError> {
        match element {
            PoolElement::Empty => Ok(()),
            PoolElement::Feature { feature, .. } => Err(PoolError::UnsupportedFeatureElement {
                pool: self.pool.clone(),
                feature: feature.clone(),
            }),
            PoolElement::List { elements, .. } => {
                for child in elements {
                    self.place_element(
                        child,
                        level,
                        templates,
                        processors,
                        chunk_box,
                        structure_pos,
                        world_seed,
                        keep_jigsaws,
                        result,
                    )?;
                }
                Ok(())
            }
            PoolElement::Single { .. } => self.place_single(
                element,
                false,
                level,
                templates,
                processors,
                chunk_box,
                structure_pos,
                world_seed,
                keep_jigsaws,
                result,
            ),
            PoolElement::LegacySingle { .. } => self.place_single(
                element,
                true,
                level,
                templates,
                processors,
                chunk_box,
                structure_pos,
                world_seed,
                keep_jigsaws,
                result,
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn place_single(
        &self,
        element: &PoolElement,
        legacy: bool,
        level: &mut dyn FeatureLevel,
        templates: &dyn TemplateProvider,
        processors: &StructureProcessorCatalog,
        chunk_box: BoundingBox,
        structure_pos: BlockPos,
        world_seed: i64,
        keep_jigsaws: bool,
        result: &mut PiecePlacement,
    ) -> Result<(), PoolError> {
        let (location, processor_ref, projection) = match element {
            PoolElement::Single {
                template,
                processors,
                projection,
                ..
            }
            | PoolElement::LegacySingle {
                template,
                processors,
                projection,
                ..
            } => (template, processors, *projection),
            _ => unreachable!("place_single only receives single pool elements"),
        };

        let template = templates
            .template(location)
            .ok_or_else(|| PoolError::UnknownTemplate {
                pool: self.pool.clone(),
                template: location.clone(),
            })?;

        let settings = StructurePlaceSettings::new().with_rotation(self.rotation);
        let piece =
            TemplateStructurePiece::new(location, template.clone(), self.position, settings);
        let origin = piece.effective_position(level);

        let raw_blocks = template_block_infos(&piece.template, &piece.place_settings, origin);
        let mut chain = Vec::new();
        // `SinglePoolElement.getSettings` adds STRUCTURE_BLOCK first.
        if !legacy {
            chain.push(StructureProcessor::block_ignore_structure_block());
        }
        if !keep_jigsaws {
            chain.push(StructureProcessor::JigsawReplacement);
        }
        chain.extend_from_slice(processor_ref.resolve(processors)?);
        chain.extend(projection.processors());
        // `LegacySinglePoolElement.getSettings` pops STRUCTURE_BLOCK from the
        // chain and appends STRUCTURE_AND_AIR last.
        if legacy {
            chain.push(StructureProcessor::block_ignore_structure_and_air());
        }

        let context = ProcessorApplyContext::new(world_seed, origin, structure_pos);
        let processed = apply_processors(&chain, level, &context, &raw_blocks);

        for block in processed {
            if !chunk_box.is_inside(block.pos) {
                continue;
            }
            let state = transform_state(
                &block.state,
                piece.place_settings.mirror(),
                piece.place_settings.rotation(),
            );
            if level.set_block(block.pos, state.clone()) {
                result.written += 1;
                if block.nbt.is_some() {
                    result.block_entities.push(PlacedBlock {
                        pos: block.pos,
                        state,
                        nbt: block.nbt,
                    });
                }
            } else {
                result.rejected += 1;
            }
        }

        collect_data_markers(&piece, origin, chunk_box, result);
        collect_entities(&piece, origin, chunk_box, result);
        Ok(())
    }
}

fn build_template_piece(
    element: &PoolElement,
    templates: &dyn TemplateProvider,
    pool: &str,
    position: BlockPos,
    rotation: Rotation,
) -> Result<Option<StructurePiece>, PoolError> {
    let Some(location) = element.template_location() else {
        return Ok(None);
    };
    let template = templates
        .template(location)
        .ok_or_else(|| PoolError::UnknownTemplate {
            pool: pool.to_owned(),
            template: location.to_owned(),
        })?;
    let settings = StructurePlaceSettings::new().with_rotation(rotation);
    Ok(Some(StructurePiece::template(TemplateStructurePiece::new(
        location,
        template.clone(),
        position,
        settings,
    ))))
}

fn element_bounds(
    element: &PoolElement,
    templates: &dyn TemplateProvider,
    pool: &str,
    position: BlockPos,
    rotation: Rotation,
) -> Result<Option<BoundingBox>, PoolError> {
    match element {
        PoolElement::Empty => Ok(None),
        PoolElement::Feature { .. } => Ok(Some(BoundingBox::from_pos(position))),
        PoolElement::Single { template, .. } | PoolElement::LegacySingle { template, .. } => {
            let template =
                templates
                    .template(template)
                    .ok_or_else(|| PoolError::UnknownTemplate {
                        pool: pool.to_owned(),
                        template: template.clone(),
                    })?;
            Ok(Some(template_bounding_box(
                template,
                position,
                rotation,
                BlockPos::ZERO,
                Mirror::None,
            )))
        }
        PoolElement::List { elements, .. } => {
            let boxes = elements
                .iter()
                .map(|child| element_bounds(child, templates, pool, position, rotation))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flatten();
            Ok(BoundingBox::encapsulating(boxes))
        }
    }
}

fn collect_data_markers(
    piece: &TemplateStructurePiece,
    origin: BlockPos,
    chunk_box: BoundingBox,
    result: &mut PiecePlacement,
) {
    for block in piece.template.blocks() {
        let Some(name) = data_marker_name(block.nbt.as_ref()) else {
            continue;
        };
        let pos = origin.offset_by(transform(
            block.pos,
            piece.place_settings.mirror(),
            piece.place_settings.rotation(),
            piece.place_settings.rotation_pivot(),
        ));
        if chunk_box.is_inside(pos) {
            result.data_markers.push(StructureDataMarker {
                name,
                pos,
                nbt: block.nbt.clone(),
            });
        }
    }
}

fn collect_entities(
    piece: &TemplateStructurePiece,
    origin: BlockPos,
    chunk_box: BoundingBox,
    result: &mut PiecePlacement,
) {
    if piece.place_settings.ignore_entities() {
        return;
    }
    let origin_vec = folia_math::Vec3::new(
        f64::from(origin.x()),
        f64::from(origin.y()),
        f64::from(origin.z()),
    );
    for entity in piece.template.entities() {
        let pos = transform_vec3(
            entity.pos,
            piece.place_settings.mirror(),
            piece.place_settings.rotation(),
            piece.place_settings.rotation_pivot(),
        )
        .add(origin_vec);
        let block_pos = origin.offset_by(transform(
            entity.block_pos,
            piece.place_settings.mirror(),
            piece.place_settings.rotation(),
            piece.place_settings.rotation_pivot(),
        ));
        if chunk_box.is_inside(block_pos) {
            result.entities.push(PlacedEntity {
                pos,
                block_pos,
                nbt: entity.nbt.clone(),
            });
        }
    }
}

fn data_marker_name(nbt: Option<&Tag>) -> Option<String> {
    let nbt = nbt?;
    if nbt.get("id").and_then(Tag::as_str) != Some("minecraft:structure_block") {
        return None;
    }
    match nbt.get("mode").and_then(Tag::as_str) {
        Some("DATA") | Some("data") => Some(
            nbt.get("metadata")
                .and_then(Tag::as_str)
                .unwrap_or_default()
                .to_owned(),
        ),
        _ => None,
    }
}

/// Errors from template-pool parsing or expansion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolError {
    InvalidDefinition(String),
    UnknownElementType { type_name: String },
    UnknownPool { pool: String },
    UnknownTemplate { pool: String, template: String },
    UnknownProcessorList { list: String },
    UnsupportedFeatureElement { pool: String, feature: String },
}

impl fmt::Display for PoolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PoolError::InvalidDefinition(message) => formatter.write_str(message),
            PoolError::UnknownElementType { type_name } => {
                write!(
                    formatter,
                    "unknown template-pool element type `{type_name}`"
                )
            }
            PoolError::UnknownPool { pool } => {
                write!(formatter, "unknown template pool `{pool}`")
            }
            PoolError::UnknownTemplate { pool, template } => write!(
                formatter,
                "template pool `{pool}` references missing template `{template}`"
            ),
            PoolError::UnknownProcessorList { list } => write!(
                formatter,
                "template-pool element references missing processor list `{list}`"
            ),
            PoolError::UnsupportedFeatureElement { pool, feature } => write!(
                formatter,
                "template pool `{pool}` selected feature element `{feature}`; feature pool \
                 placement is not implemented"
            ),
        }
    }
}

impl std::error::Error for PoolError {}

fn parse_projection(object: &Map<String, Value>, context: &str) -> Result<Projection, PoolError> {
    let name = required_str(object, "projection", context)?;
    Projection::from_name(name).ok_or_else(|| {
        PoolError::InvalidDefinition(format!("{context} has unknown projection `{name}`"))
    })
}

fn parse_liquid_settings(
    object: &Map<String, Value>,
    context: &str,
) -> Result<Option<LiquidSettings>, PoolError> {
    match object.get("override_liquid_settings") {
        None => Ok(None),
        Some(value) => {
            let name = value.as_str().ok_or_else(|| {
                PoolError::InvalidDefinition(format!(
                    "{context} override_liquid_settings must be a string"
                ))
            })?;
            LiquidSettings::from_name(name).map(Some).ok_or_else(|| {
                PoolError::InvalidDefinition(format!(
                    "{context} has unknown liquid setting `{name}`"
                ))
            })
        }
    }
}

fn count_map(object: &Map<String, Value>) -> Result<BTreeMap<String, usize>, PoolError> {
    let mut counts = BTreeMap::new();
    for (key, value) in object {
        let value = value.as_u64().ok_or_else(|| {
            PoolError::InvalidDefinition(format!("count for `{key}` must be an integer"))
        })?;
        counts.insert(
            key.clone(),
            usize::try_from(value).map_err(|_| {
                PoolError::InvalidDefinition(format!("count for `{key}` is too large"))
            })?,
        );
    }
    Ok(counts)
}

fn normalized_id(name: &str) -> String {
    if name.contains(':') {
        name.to_owned()
    } else {
        format!("minecraft:{name}")
    }
}

fn required<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    what: &str,
) -> Result<&'a Value, PoolError> {
    object
        .get(key)
        .ok_or_else(|| PoolError::InvalidDefinition(format!("{what} is missing `{key}`")))
}

fn required_str<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    what: &str,
) -> Result<&'a str, PoolError> {
    required(object, key, what)?.as_str().ok_or_else(|| {
        PoolError::InvalidDefinition(format!("{what} field `{key}` must be a string"))
    })
}

fn required_object<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    what: &str,
) -> Result<&'a Map<String, Value>, PoolError> {
    required(object, key, what)?.as_object().ok_or_else(|| {
        PoolError::InvalidDefinition(format!("{what} field `{key}` must be an object"))
    })
}

fn required_i32(object: &Map<String, Value>, key: &str, what: &str) -> Result<i32, PoolError> {
    let value = required(object, key, what)?.as_i64().ok_or_else(|| {
        PoolError::InvalidDefinition(format!("{what} field `{key}` must be an integer"))
    })?;
    i32::try_from(value).map_err(|_| {
        PoolError::InvalidDefinition(format!("{what} field `{key}` is outside the int range"))
    })
}

fn required_usize(object: &Map<String, Value>, key: &str, what: &str) -> Result<usize, PoolError> {
    let value = required(object, key, what)?.as_u64().ok_or_else(|| {
        PoolError::InvalidDefinition(format!(
            "{what} field `{key}` must be a non-negative integer"
        ))
    })?;
    usize::try_from(value)
        .map_err(|_| PoolError::InvalidDefinition(format!("{what} field `{key}` is too large")))
}
