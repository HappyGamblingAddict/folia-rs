//! Template-backed ruined portals.
//!
//! Setup choice, common/giant template choice, rotation, mirror, pivot, and
//! vertical-placement rules follow `RuinedPortalStructure` for the shipped
//! structure ids; arbitrary datapack setup overrides are not interpreted. The
//! terrain API in this crate exposes heightmaps rather than complete column
//! samples, so the vanilla three-of-four-corner solid test is approximated by
//! treating every position below a corner's heightmap as solid.
//!
//! Template processor replacements (gold, moss, lava, and blackstone), vines,
//! overgrown leaves, netherrack spreading/drip columns, and chest loot seeding
//! are not applied yet. The shipped templates contain obsidian frames and lava
//! interiors rather than `nether_portal` states. Starts use the existing legacy
//! large-feature seed path; an exact Java `WorldgenRandom` differential remains
//! pending.

use crate::worldgen::feature::{FeatureLevel, Heightmap};
use crate::worldgen::structure::StructureError;
use crate::worldgen::structure::piece::{
    BoundingBox, PiecePlacement, TemplateBlockFilter, TemplateStructurePiece, template_bounding_box,
};
use crate::worldgen::structure::set::set_large_feature_seed;
use crate::worldgen::structure::start::{
    StartPlacement, StructureGenerationContext, StructureStart, TerrainAdjustment,
};
use crate::worldgen::structure::template::{
    Mirror, Rotation, StructurePlaceSettings, StructureTemplate,
};
use folia_math::BlockPos;
use folia_random::{LegacyRandomSource, RandomSource};

pub const COMMON_TEMPLATES: [&str; 10] = [
    "minecraft:ruined_portal/portal_1",
    "minecraft:ruined_portal/portal_2",
    "minecraft:ruined_portal/portal_3",
    "minecraft:ruined_portal/portal_4",
    "minecraft:ruined_portal/portal_5",
    "minecraft:ruined_portal/portal_6",
    "minecraft:ruined_portal/portal_7",
    "minecraft:ruined_portal/portal_8",
    "minecraft:ruined_portal/portal_9",
    "minecraft:ruined_portal/portal_10",
];

pub const GIANT_TEMPLATES: [&str; 3] = [
    "minecraft:ruined_portal/giant_portal_1",
    "minecraft:ruined_portal/giant_portal_2",
    "minecraft:ruined_portal/giant_portal_3",
];

const GIANT_PROBABILITY: f32 = 0.05;
const MIN_BLOCKS_ABOVE_BOTTOM: i32 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuinedPortalVariant {
    Standard,
    Desert,
    Jungle,
    Swamp,
    Mountain,
    Ocean,
    Nether,
}

impl RuinedPortalVariant {
    pub const ALL: [Self; 7] = [
        Self::Standard,
        Self::Desert,
        Self::Jungle,
        Self::Swamp,
        Self::Mountain,
        Self::Ocean,
        Self::Nether,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Desert => "desert",
            Self::Jungle => "jungle",
            Self::Swamp => "swamp",
            Self::Mountain => "mountain",
            Self::Ocean => "ocean",
            Self::Nether => "nether",
        }
    }

    pub fn from_structure_id(structure: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|variant| {
            structure
                == match variant {
                    Self::Standard => "minecraft:ruined_portal",
                    Self::Desert => "minecraft:ruined_portal_desert",
                    Self::Jungle => "minecraft:ruined_portal_jungle",
                    Self::Swamp => "minecraft:ruined_portal_swamp",
                    Self::Mountain => "minecraft:ruined_portal_mountain",
                    Self::Ocean => "minecraft:ruined_portal_ocean",
                    Self::Nether => "minecraft:ruined_portal_nether",
                }
        })
    }

    pub const fn heightmap(self) -> Heightmap {
        match self {
            Self::Swamp | Self::Ocean => Heightmap::OceanFloorWg,
            Self::Standard | Self::Desert | Self::Jungle | Self::Mountain | Self::Nether => {
                Heightmap::WorldSurfaceWg
            }
        }
    }

    pub const fn setups(self) -> &'static [RuinedPortalSetup] {
        match self {
            Self::Standard => &STANDARD_SETUPS,
            Self::Desert => &DESERT_SETUPS,
            Self::Jungle => &JUNGLE_SETUPS,
            Self::Swamp => &SWAMP_SETUPS,
            Self::Mountain => &MOUNTAIN_SETUPS,
            Self::Ocean => &OCEAN_SETUPS,
            Self::Nether => &NETHER_SETUPS,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuinedPortalPlacement {
    OnLandSurface,
    PartlyBuried,
    OnOceanFloor,
    InMountain,
    Underground,
    InNether,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RuinedPortalSetup {
    pub placement: RuinedPortalPlacement,
    pub air_pocket_probability: f32,
    pub mossiness: f32,
    pub overgrown: bool,
    pub vines: bool,
    pub can_be_cold: bool,
    pub replace_with_blackstone: bool,
    pub weight: f32,
}

const STANDARD_SETUPS: [RuinedPortalSetup; 2] = [
    RuinedPortalSetup {
        placement: RuinedPortalPlacement::Underground,
        air_pocket_probability: 1.0,
        mossiness: 0.2,
        overgrown: false,
        vines: false,
        can_be_cold: true,
        replace_with_blackstone: false,
        weight: 0.5,
    },
    RuinedPortalSetup {
        placement: RuinedPortalPlacement::OnLandSurface,
        air_pocket_probability: 0.0,
        mossiness: 0.2,
        overgrown: false,
        vines: false,
        can_be_cold: true,
        replace_with_blackstone: false,
        weight: 0.5,
    },
];

const DESERT_SETUPS: [RuinedPortalSetup; 1] = [RuinedPortalSetup {
    placement: RuinedPortalPlacement::PartlyBuried,
    air_pocket_probability: 0.0,
    mossiness: 0.0,
    overgrown: false,
    vines: false,
    can_be_cold: false,
    replace_with_blackstone: false,
    weight: 1.0,
}];

const JUNGLE_SETUPS: [RuinedPortalSetup; 1] = [RuinedPortalSetup {
    placement: RuinedPortalPlacement::OnLandSurface,
    air_pocket_probability: 0.0,
    mossiness: 0.8,
    overgrown: true,
    vines: true,
    can_be_cold: false,
    replace_with_blackstone: false,
    weight: 1.0,
}];

const SWAMP_SETUPS: [RuinedPortalSetup; 1] = [RuinedPortalSetup {
    placement: RuinedPortalPlacement::OnOceanFloor,
    air_pocket_probability: 0.0,
    mossiness: 0.5,
    overgrown: false,
    vines: true,
    can_be_cold: false,
    replace_with_blackstone: false,
    weight: 1.0,
}];

const MOUNTAIN_SETUPS: [RuinedPortalSetup; 2] = [
    RuinedPortalSetup {
        placement: RuinedPortalPlacement::InMountain,
        air_pocket_probability: 1.0,
        mossiness: 0.2,
        overgrown: false,
        vines: false,
        can_be_cold: true,
        replace_with_blackstone: false,
        weight: 0.5,
    },
    RuinedPortalSetup {
        placement: RuinedPortalPlacement::OnLandSurface,
        air_pocket_probability: 0.0,
        mossiness: 0.2,
        overgrown: false,
        vines: false,
        can_be_cold: true,
        replace_with_blackstone: false,
        weight: 0.5,
    },
];

const OCEAN_SETUPS: [RuinedPortalSetup; 1] = [RuinedPortalSetup {
    placement: RuinedPortalPlacement::OnOceanFloor,
    air_pocket_probability: 0.0,
    mossiness: 0.8,
    overgrown: false,
    vines: false,
    can_be_cold: true,
    replace_with_blackstone: false,
    weight: 1.0,
}];

const NETHER_SETUPS: [RuinedPortalSetup; 1] = [RuinedPortalSetup {
    placement: RuinedPortalPlacement::InNether,
    air_pocket_probability: 0.0,
    mossiness: 0.0,
    overgrown: false,
    vines: false,
    can_be_cold: false,
    replace_with_blackstone: true,
    weight: 1.0,
}];

#[derive(Debug, Clone, PartialEq)]
pub struct RuinedPortalPiece {
    template: TemplateStructurePiece,
    variant: RuinedPortalVariant,
    placement: RuinedPortalPlacement,
    air_pocket: bool,
}

impl RuinedPortalPiece {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        template_name: impl Into<String>,
        template: StructureTemplate,
        position: BlockPos,
        rotation: Rotation,
        mirror: Mirror,
        variant: RuinedPortalVariant,
        setup: RuinedPortalSetup,
        air_pocket: bool,
    ) -> Self {
        let settings = StructurePlaceSettings::new()
            .with_rotation(rotation)
            .with_mirror(mirror)
            .with_rotation_pivot(BlockPos::new(
                template.size_x() / 2,
                0,
                template.size_z() / 2,
            ))
            .with_ignore_entities(false)
            .with_ignore_air(!air_pocket);
        let template = TemplateStructurePiece::new(template_name, template, position, settings)
            .with_block_filter(TemplateBlockFilter::IgnoreStructureBlock);
        Self {
            template,
            variant,
            placement: setup.placement,
            air_pocket,
        }
    }

    pub fn template(&self) -> &TemplateStructurePiece {
        &self.template
    }

    pub fn variant(&self) -> RuinedPortalVariant {
        self.variant
    }

    pub fn placement(&self) -> RuinedPortalPlacement {
        self.placement
    }

    pub fn air_pocket(&self) -> bool {
        self.air_pocket
    }

    pub fn bounding_box(&self) -> BoundingBox {
        self.template.bounding_box()
    }

    pub fn place(
        &self,
        level: &mut dyn FeatureLevel,
        chunk_box: BoundingBox,
    ) -> Result<StartPlacement, StructureError> {
        if !chunk_box.is_inside(self.bounding_box().center()) {
            return Ok(StartPlacement::default());
        }
        let placement: PiecePlacement = self.template.place(level, chunk_box);
        Ok(StartPlacement {
            placed_pieces: 1,
            written: placement.written,
            rejected: placement.rejected,
            written_blocks: placement.written_blocks,
            block_entities: placement.block_entities,
            entities: placement.entities,
            data_markers: placement.data_markers,
            ..StartPlacement::default()
        })
    }
}

pub struct RuinedPortalStart;

impl RuinedPortalStart {
    pub const COMMON_TEMPLATES: [&str; 10] = COMMON_TEMPLATES;
    pub const GIANT_TEMPLATES: [&str; 3] = GIANT_TEMPLATES;

    pub fn generate(
        structure_id: &str,
        context: &StructureGenerationContext<'_>,
        terrain_adaptation: TerrainAdjustment,
        variant: RuinedPortalVariant,
    ) -> Result<StructureStart, StructureError> {
        let source = context
            .height_source
            .ok_or_else(|| StructureError::MissingHeightSource {
                structure: structure_id.to_owned(),
            })?;
        let mut random = LegacyRandomSource::new(0);
        set_large_feature_seed(
            &mut random,
            context.seed,
            context.chunk_pos.x,
            context.chunk_pos.z,
        );
        let setup = choose_setup(variant.setups(), &mut random);
        let air_pocket = sample_air_pocket(&mut random, setup.air_pocket_probability);
        let template_name = choose_template(&mut random);
        let template = context
            .templates
            .template(template_name)
            .cloned()
            .ok_or_else(|| StructureError::UnknownTemplate {
                structure: structure_id.to_owned(),
                template: template_name.to_owned(),
            })?;
        let rotation = Rotation::ALL[random.next_int_bound(4) as usize];
        let mirror = if random.next_float() < 0.5 {
            Mirror::None
        } else {
            Mirror::FrontBack
        };
        let pivot = BlockPos::new(template.size_x() / 2, 0, template.size_z() / 2);
        let origin = BlockPos::new(context.chunk_pos.x << 4, 0, context.chunk_pos.z << 4);
        let preview = template_bounding_box(&template, origin, rotation, pivot, mirror);
        let center = preview.center();
        let heightmap = variant.heightmap();
        let surface_y = source
            .heightmap(heightmap, center.x(), center.z())
            .ok_or_else(|| StructureError::MissingHeightSource {
                structure: structure_id.to_owned(),
            })?
            - 1;
        let minimum_y = source.min_y() + MIN_BLOCKS_ABOVE_BOTTOM;
        let target_y = match setup.placement {
            RuinedPortalPlacement::OnLandSurface | RuinedPortalPlacement::OnOceanFloor => surface_y,
            RuinedPortalPlacement::Underground => {
                random_between_if_ordered(&mut random, minimum_y, surface_y - preview.y_span())
            }
            RuinedPortalPlacement::InMountain => {
                random_between_if_ordered(&mut random, 70, surface_y - preview.y_span())
            }
            RuinedPortalPlacement::PartlyBuried => {
                surface_y - preview.y_span() + random.next_int_between_inclusive(2, 8)
            }
            RuinedPortalPlacement::InNether => {
                if air_pocket {
                    random.next_int_between_inclusive(32, 100)
                } else if random.next_float() < 0.5 {
                    random.next_int_between_inclusive(27, 29)
                } else {
                    random.next_int_between_inclusive(29, 100)
                }
            }
        };
        let y = find_suitable_y(source, preview, heightmap, target_y, minimum_y);
        let piece = RuinedPortalPiece::new(
            template_name,
            template,
            BlockPos::new(origin.x(), y, origin.z()),
            rotation,
            mirror,
            variant,
            *setup,
            air_pocket,
        );
        Ok(StructureStart::new_ruined_portal(
            structure_id,
            context.chunk_pos,
            context.references,
            piece,
            terrain_adaptation,
        ))
    }
}

fn choose_setup<'a>(
    setups: &'a [RuinedPortalSetup],
    random: &mut dyn RandomSource,
) -> &'a RuinedPortalSetup {
    if setups.len() == 1 {
        return &setups[0];
    }
    let total: f32 = setups.iter().map(|setup| setup.weight).sum();
    let mut pick = random.next_float();
    for setup in setups {
        pick -= setup.weight / total;
        if pick < 0.0 {
            return setup;
        }
    }
    &setups[setups.len() - 1]
}

fn sample_air_pocket(random: &mut dyn RandomSource, probability: f32) -> bool {
    if probability <= 0.0 {
        false
    } else if probability >= 1.0 {
        true
    } else {
        random.next_float() < probability
    }
}

fn choose_template(random: &mut dyn RandomSource) -> &'static str {
    if random.next_float() < GIANT_PROBABILITY {
        GIANT_TEMPLATES[random.next_int_bound(GIANT_TEMPLATES.len() as i32) as usize]
    } else {
        COMMON_TEMPLATES[random.next_int_bound(COMMON_TEMPLATES.len() as i32) as usize]
    }
}

fn random_between_if_ordered(random: &mut dyn RandomSource, minimum: i32, maximum: i32) -> i32 {
    if minimum < maximum {
        random.next_int_between_inclusive(minimum, maximum)
    } else {
        maximum
    }
}

fn find_suitable_y(
    source: &dyn FeatureLevel,
    preview: BoundingBox,
    heightmap: Heightmap,
    mut y: i32,
    minimum_y: i32,
) -> i32 {
    let corners = [
        (preview.min_x(), preview.min_z()),
        (preview.max_x(), preview.min_z()),
        (preview.min_x(), preview.max_z()),
        (preview.max_x(), preview.max_z()),
    ];
    let heights: Vec<Option<i32>> = corners
        .into_iter()
        .map(|(x, z)| source.heightmap(heightmap, x, z))
        .collect();
    while y > minimum_y {
        let mut solid = 0usize;
        for height in &heights {
            if height.is_some_and(|height| height > y) {
                solid += 1;
                if solid == 3 {
                    return y;
                }
            }
        }
        y -= 1;
    }
    y
}
