use folia_core::chunk::BlockState;
use folia_core::math::BlockPos;
use folia_core::nbt::Tag;
use folia_core::region::ChunkPos;
use folia_core::worldgen::feature::{Decoration, FeatureLevel, Heightmap};
use folia_core::worldgen::structure::StructureError;
use folia_core::worldgen::structure::assets::StructureTemplateCatalog;
use folia_core::worldgen::structure::jungle_pyramid::{JunglePyramidPiece, JunglePyramidStart};
use folia_core::worldgen::structure::ocean_monument::{OceanMonumentPiece, OceanMonumentStart};
use folia_core::worldgen::structure::ocean_ruin::{OceanRuinPiece, OceanRuinStart};
use folia_core::worldgen::structure::piece::{BoundingBox, StructurePiece, TemplateStructurePiece};
use folia_core::worldgen::structure::ruined_portal::{
    RuinedPortalPlacement, RuinedPortalStart, RuinedPortalVariant,
};
use folia_core::worldgen::structure::set::{
    PlacementError, RandomSpreadType, StructureBiomeLookup, StructureDimension, StructurePlacement,
    StructureSetError, StructureSetRegistry,
};
use folia_core::worldgen::structure::shipwreck::ShipwreckStart;
use folia_core::worldgen::structure::start::{
    BiomeSet, IglooStructure, StructureDefinition, StructureGenerationContext, StructureGenerator,
    StructureKind, TemplateProvider,
};
use folia_core::worldgen::structure::template::{
    Mirror, Rotation, StructurePlaceSettings, StructureTemplate,
};
use folia_core::worldgen::structure::witch_hut::WitchHutStart;
use folia_math::Direction;
use std::collections::HashMap;

// Byte-for-byte gzip copies of shipped 1.21.11 templates. They are embedded so
// the test does not depend on the out-of-tree decompiled reference directory.
// Sources:
//   data/minecraft/structure/igloo/top.nbt
//   data/minecraft/structure/igloo/middle.nbt
//   data/minecraft/structure/igloo/bottom.nbt
const IGLOO_TOP_NBT: &str = "1f8b08000000000000ffa598c18adb301086156b9438595ae8524aa16fd24b2fbdf4524aa1bd06c55112b38e656485d03e491fb7729a714cd69e99a581ecb2f667fdff3f1a24ad574a2d15b4e56fa79552dd7791be267df3a5ca5d1dcb58ba56759fa59a6f2a5f3cb5abf4c79fa5d28d6ff121fc68ad4c1b6d74d727c618103086606657261330941f64283fc848fccc0926eb0bc433330143654786ca8e0c951d192a3b3292ec0b82e9e75ec050f54186aa0f32547d90a1ea830c951d192a3b08b22343654786ca8e0c951d192a3b32546f2043d50719aa3e46900b192a1732542e64a85cc850b9e6023fc8507e90a1fc749f19a385cc506b36c1706bdd8c998bd9c838d904c3ad1b97b555c0706b66c770eb0fc780c00f08fc8040cb08ea8c0cd7875c6f2043f506325c1f668c1632dc192063b4665786ebc34cd06399a0c732418f65821ee31810f801811f106819410d91e17a8c9b7764b8b54e32ef9ad142863bd769462bc3df02863b0368664e7b5f02863bdb707e40300e08c63182b94086db732573018c1632dc9916182d2dd0d2022d2dd002811608b440a0856beb90d113fbf2500b26f6d361ff980966986b3ec18080310266d8ab8b89fd5d119efbfc0286cadeaf2f846764a8ecc850d991a1b283c033083c83c033083c83c0b319f173ef19f777090302c630fd7cbf574e31949f4ce0277b819f39d387f767892986eb792e3b3254f67efd1530547664a8ec20f00c02cf20f00c02cf20f06c047e8cc08f11f8c1730bd7ab5ad0ab5a305f9c96166869811608b440a005a35a2ba5eb4dccd4ebaa8cebe8a3add6b13cbaae0c8f85f74f65bdbf5c58b78dab63b7ff9a2fd11dffbd3e1d3283477395955bf5e658d6ae0876173fee4ea1b6854bf845a51b2eb8a32debf4a44a161ebebba26c5cfba375dba97fbd87f1f3abed7f42af6e429bf4fcf8aa3bacccf2e58f0f8bb64acca2b1958bd175ef8d1f72055f6d0afef636525bfbf3faf26a59e1dd814e59b8fef2fbdbe5cbcf4b31eda6724967f52df8c685ee7575c20fb6da291d7d93ab8773b2122abfdf27c76667abd6e5c9933fbb30b830dfd9a2abb0a97d888734421aacbedeedf5dfddf4bd7d5ac7609badf7e15e5da7995310c369d479526da3af5d6a83501cc622db328c499e0f6574ebc286c6c5fefe87dbfdaadc1fe27a1fec2f84c67cdd070667dbdb70cf1bf159691b1b52ba9df77150b5d69fbaaae5be284e4dd9d7756cdc947fdd35cff8b80767b7ff37ae560f9f6db43f5d684b5f2bf5f849fd05c19352bee6180000";
const IGLOO_MIDDLE_NBT: &str = "1f8b08000000000000ff8d92c16ac330104447729cdacea125877c4c2fbdf45c72eab56cecb523a248465a28e4eb6b4344db40622de820789a9915d3003556d15cb800f0efd4a8d88911c311f3d4581fac6f4fb1992ecf358ad1c704a75105ca28247c7df1975109ca60f403466778e1aab5944765e451197996bc90b8052f9d914767e4c9f1bafd437567f725e6d66b669e46b22cc273557485d5079d19bbb371dc06eae5358a77fc750866aa131a34fbe0470e73d52a6cbe279d60fd307087b2271bb9c2baa7d6b801a5f3418e48922fbf9296ba8ec3946af34e429f1ca2f10ed8bee1076063782fe4020000";
const IGLOO_BOTTOM_NBT: &str = "1f8b08000000000000ffbd5b4b8c1c4719aee9c7bcfc58bf9d8828e19088201129b13845c07aedb5b125631b270ec1426a6aa66b665adbd33554d778bc1648ce15210241404ec8089038704002ce3611120819290724ae96b8c08d430e1c40e6afeefe676b67bbabcac1f24863cf747ff5d7f7fdafee69ffee13d223419edc663e2144bd3bf06ec3bbd7235d96c944262cefc377af4ffc6c207db276612ee499ed379329cb259dce48b1b07d2d194e580e9fd75aa47f9ea6e9f9743bc9c670aa779e8b218b37c64c415b64ffc5ece63ccd98a0839491901cda18e45ccc64c2b38d299f67b2b478f02a1792a667394f63bec8d4418ff436199513b5b782786fcf4a9347af329127b964d9905d63df982782c5ad2ef192981c9926191b0a3a92afdf4cd2948e99d84782ebd72f6ec2cae0277f7ee1fb077f7125befbe8f32fdd78fe870f7de2573c7ba4fd25ae38b52bc714af7bbf3d127ef0eed73e8ddf5ba47389d17cc26210d2bec0682a271b77d5f1fe253692176816c3290fac26a2f519f0c52650cf7230ab56f749fbca6804d47ba4738d0d9359e56a9f74a6f4d6f5bcf067c727c1bcfc08667b822da888cfdd9ab5202083f9b64fc261e5b4174bc52776140b2ec127d12865f904760b7296a6da82ca45877716b02984258d95e28f45a1575a3cb663710ce18b2011b87c3c02b051f74af64501c8b8d52607469052510c31a61063743fe4e8352ea90a53a81c77ffdbdfbc73ffc62f7fdf233de5fa8b924d4b8f2aa87f95e74530ef7ff7afc5ead39f2caddcffde7b95b5fe869872b15c169449179c877c7af4a8450e9ca5d9d564b8757d76897309eaf7bf55a5d42695b44bfa33c1472c2fc27b6847d33065221982ec94dd6469293b90db33a68366294db25c71a0528a6430976532b4f77814e2124d8a4c6b9360407376fab3953b4ae8f33bd0ad8c0fb70674b815099657beab16a10bab45cf68f6f94d884226a37cc6585cc1d71fee82afedc0a97259bdd167575091e4f3f124030fd5e3b5cc1df134e58b48d06ccc209a531e27a3042a45b904d2a14d8b4e71ef37ff7977bd73f4e572f9735ae243f4f91414d045160d7836cfbba4c767905b2a57c8511ac7d1749eca6496262c8e0a2ee8cd572b4e1ee9aa5657361be0006d0bbcc2c53656fe5720f8aac5a9341ca4e06948306ca35e018137e4ddaccabbd3416978fd5fd5dfffae7632b4d66fedeea64fbb7b6a11bccda7838445354df4537fbff2daf4e767def9d187c7effefae47fcfc1ee6739384ba84250bb3d82d7ffd14fdf1f8ddcfae9c769191fdcfbc13fcae37b5a4659f06704a35b9b9c8b7c770ff9c36a0f79f0e0c163f710f2547a48f749f790e68a5cffe9a9d7dff9e7fbbf7adc8aeca98abc49d3f9b20e1fb741fd65f6b387d60675fa6937a8ef643f7ef16f5f5f7ba20d6ae38ecee9a4a6414a15a7984e219f10dd6d0a6cb9b7604936523768caa3287bfd777f7cefcbfbfff4caee1e58d7e782ba3e7764a5cf7df4e0c187658db58bf585933e2a57a0a1167a1a721c924cb2aa5aeb302d078ce780f1354cab01136818af01136a18bf01d376b0d331d8f11cfc8318dd3f4103c633f0418cef80d1fd13366042073b26ff20a663c02c2f2906ff20c6943f8831e50f627c43bc1063ca1fc498f2073126ff20c6943f81837f1063ca1fc498f20731a6fc418cc93f8831e50f624cfe418c297f4207ff20c6943f8831e50f6274ffb41b3026ff20c6943f88691bea1431a6fc2974af70ee346002074ce880693b603a064cab7a13830f11d372c0780e7b750d18cf818f5763a75b53ef363bbe832edf4197efa02b70e01338d8091dec840eba42075da1039fb643ce23c694f38831e53c624c398f1853ce932a8774ceab3d1c31a6de821853ef454cdb612f53ef6d55185b9d7a96b823c6746d428c1ef7bafb161b1fafc64e5d7dd9ecf80ebafc1a5dab7ef66bf8d45dbb6d7c02075da1839dd0415758a3ab0963d2d5aec9f9553b8831e53c624287bd4cd753c498729e60dc1c30a67b6cc4d8ead427e67b24c4d8ead4b7c41d31b63af52d71478c290f3d073e9ec35e9ec35ebec35e4bee0e18131fdf814fe0c02770d82b70d82b74d80b31b67ab7f1091df8e023445bbdafd64e13c6543b8831d50e626cb513583823c6f6cc21b070468ced994360e1ec5518534c1163ab2f9b76c498b423c6f63c61557bddf5dd55bbad27d8fc83185bbddbfc8318dbf38455ffacfeae448ced79824d7be0a01d31b6fe63d38e18dbb3025b6e20c6f6acc0961b88b1f54c9baed04157e8a02b74d0153ae8f22a9ced395be8903fa143fed8f6c2dfd4ba0f7b0d98c001a3fbb0df80e91830cb7fd473c0e839bfaf01e33bd8091c30265d88693b604cda7d07cebe0367df81b3efc0d977e0bcfcc709078c295e883169478c493b624cda1163d28e1893f6d0817358c3797f0346e77ca001a3733ed880d139af35fc363771468cc9cf88b1d5fbea6fb4baba58fd0ddb84b1d5bb4d97e7a0cb73d085185bbddbb4fb0e7c7c073ebe031fbf86cfa186e712b65ab6c50231b65ab669478cad966dda1163ab651be7d08173e8c039ace173b8e1b7b9cee74803c656a7be850f624c3e448cad967d4bfe7835ba9a30b63ab5e9f21c747935ba8e363c2bd0391f6bc0d86ad9c6d977e0ecd770aeab655b2c821a5d4d185b9dda74050eba821a5dabb1081d38870e9c4307cea103e7b086735dbc567f17acc60b6dedc694536b3d1216b34e44bdf6ccdea89960d970a7ad333f5e59eb92ee94491a534949582cee92f63411820b125cbe72f95c8b1c4cc61917ec5c359bac46c6667cc1048b414790331693ea054be95c4e6069fb0d46c598817551cd7e95d6ba2498f298916073e3cd0d9f04c0f2ed322fd5c7af628a26b7993aac9e232499646391c8edf53b4a8d82dd503060914ff8822602b60d32aae665f6cc18e552cc87722e58548cc154a6d536a0624dad1fa8d9b5241b0ff82d529dbe5168a9bf79d15d78a2722104b898d692ec16382f1cf2141c100e5238d653decd733a2ec7c202354206a8c255a64f2d726842f3689cf205708bcaa3a5ba839a3a084c9ff4478267b2c0f4c8e151924a159ac8b26ff8b957e0b5f3557dfb82c6c0cedc66a14ec38a7beac63c562fbb272b2fb748707ece5282e9dfaf726539d90c8037d26248b83fe4d319cfd478555fcf8659310e190dc15dea1c646b79441f595f30baa5c6d16a86bdf2594af34954add9737a205821148867b147ba67e0fb728cabee82afe7d23380e9cc68caa4644ad9b3e0c7cb2aa7d7f464064d044f7c42d795266a823352c39a792277402756564703910cb7f2e5f917f4c691e42c0523b5406d8a0dfe1c6e35e134834936826ed2047cb90668a6f09c3e8f98e7dbf5a8976aec1ae0da84a62a9a2115332697670f68937ed06720b3ae0a3532a8da20601885561b408b813eb76f018114291f8fd524ed88a6391c0c730eed70e72bb451ed6bb060b87eb9e3518d3e947534a0225fddb79a486d9d5aaed246350b1aa0639ec6b07e0fe5094d47c4977cd6c878426740071a274dc613753118d1a11a84aec8e396bbea029a2c53799fec255bbb4b3947ab682ced1ddf6b2fa58327c9bf8cc513e1dfe0136d3038a571cc44dd6e33f53f4454a6c17529af5b3ae48305dba3bc61cb637ae4d334925c0c278d6b77bbc069ad517f510118ce760e87d29d54de7353b21acbf246a0b8f3c035cd97eec78b4b63ac39dd8a0ab1eadab96a73bfba5e0d203e298b5e5d1ad58fbe86e5ae1f3c5541ebf6db755180eebf4fcd9cbf550eeac34fb875f23fa75ade2610360000";

fn hex(source: &str) -> Vec<u8> {
    assert!(
        source.len().is_multiple_of(2),
        "hex input has an odd length"
    );
    (0..source.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&source[index..index + 2], 16).expect("valid hex"))
        .collect()
}

fn load_template(source: &str) -> StructureTemplate {
    StructureTemplate::from_gzip(&hex(source)).expect("embedded template parses")
}

fn igloo_templates() -> HashMap<String, StructureTemplate> {
    let mut templates = HashMap::new();
    templates.insert(
        IglooStructure::TOP_TEMPLATE.to_owned(),
        load_template(IGLOO_TOP_NBT),
    );
    templates.insert(
        IglooStructure::MIDDLE_TEMPLATE.to_owned(),
        load_template(IGLOO_MIDDLE_NBT),
    );
    templates.insert(
        IglooStructure::BOTTOM_TEMPLATE.to_owned(),
        load_template(IGLOO_BOTTOM_NBT),
    );
    templates
}

const IGLOO_JSON: &str = r##"{ "type": "minecraft:igloo", "biomes": "#minecraft:has_structure/igloo", "spawn_overrides": {}, "step": "surface_structures" }"##;

const JUNGLE_PYRAMID_JSON: &str = r##"{ "type": "minecraft:jungle_pyramid", "biomes": "#minecraft:has_structure/jungle_temple", "spawn_overrides": {}, "step": "surface_structures" }"##;

const SHIPWRECK_JSON: &str = r##"{ "type": "minecraft:shipwreck", "biomes": "#minecraft:has_structure/shipwreck", "is_beached": false, "spawn_overrides": {}, "step": "surface_structures" }"##;

const SHIPWRECK_BEACHED_JSON: &str = r##"{ "type": "minecraft:shipwreck", "biomes": "#minecraft:has_structure/shipwreck_beached", "is_beached": true, "spawn_overrides": {}, "step": "surface_structures" }"##;

const OCEAN_RUIN_COLD_JSON: &str = r##"{ "type": "minecraft:ocean_ruin", "biomes": "#minecraft:has_structure/ocean_ruin_cold", "spawn_overrides": {}, "step": "surface_structures" }"##;

const OCEAN_RUIN_WARM_JSON: &str = r##"{ "type": "minecraft:ocean_ruin", "biomes": "#minecraft:has_structure/ocean_ruin_warm", "spawn_overrides": {}, "step": "surface_structures" }"##;

const OCEAN_MONUMENT_JSON: &str = r##"{ "type": "minecraft:ocean_monument", "biomes": "#minecraft:has_structure/ocean_monument", "spawn_overrides": {}, "step": "surface_structures" }"##;

const VILLAGE_PLAINS_JSON: &str = r##"{ "type": "minecraft:jigsaw", "biomes": "#minecraft:has_structure/village_plains", "max_distance_from_center": 80, "project_start_to_heightmap": "WORLD_SURFACE_WG", "size": 6, "spawn_overrides": {}, "start_height": { "absolute": 0 }, "start_pool": "minecraft:village/plains/town_centers", "step": "surface_structures", "terrain_adaptation": "beard_thin", "use_expansion_hack": true }"##;

#[derive(Default)]
struct MockLevel {
    min_y: i32,
    height: i32,
    fixed_height: Option<i32>,
    ocean_floor_height: Option<i32>,
    heights: HashMap<(i32, i32), i32>,
    blocks: HashMap<BlockPos, BlockState>,
}

impl MockLevel {
    fn new(min_y: i32, height: i32) -> Self {
        Self {
            min_y,
            height,
            fixed_height: None,
            ocean_floor_height: None,
            heights: HashMap::new(),
            blocks: HashMap::new(),
        }
    }
}

impl FeatureLevel for MockLevel {
    fn min_y(&self) -> i32 {
        self.min_y
    }

    fn height(&self) -> i32 {
        self.height
    }

    fn get_block(&self, pos: BlockPos) -> Option<&BlockState> {
        self.blocks.get(&pos)
    }

    fn set_block(&mut self, pos: BlockPos, state: BlockState) -> bool {
        if pos.y() < self.min_y || pos.y() >= self.min_y + self.height {
            return false;
        }
        self.blocks.insert(pos, state);
        true
    }

    fn heightmap(&self, kind: Heightmap, x: i32, z: i32) -> Option<i32> {
        self.heights.get(&(x, z)).copied().or_else(|| {
            if matches!(kind, Heightmap::OceanFloor | Heightmap::OceanFloorWg) {
                self.ocean_floor_height.or(self.fixed_height)
            } else {
                self.fixed_height
            }
        })
    }
}

#[test]
fn shipped_structure_sets_parse_with_expected_counts() {
    let registry = StructureSetRegistry::shipped().expect("shipped sets parse");
    assert_eq!(registry.sets().len(), 20);

    let mut entries = 0;
    let mut random_spread = 0;
    let mut concentric = 0;
    let mut triangular = 0;
    let mut legacy_type_1 = 0;
    let mut legacy_type_2 = 0;
    let mut legacy_type_3 = 0;
    for set in registry.sets() {
        entries += set.structures.len();
        match &set.placement {
            StructurePlacement::RandomSpread(placement) => {
                random_spread += 1;
                if placement.spread_type == RandomSpreadType::Triangular {
                    triangular += 1;
                }
                match placement.frequency_reduction_method {
                    folia_core::worldgen::structure::FrequencyReductionMethod::LegacyType1 => {
                        legacy_type_1 += 1
                    }
                    folia_core::worldgen::structure::FrequencyReductionMethod::LegacyType2 => {
                        legacy_type_2 += 1
                    }
                    folia_core::worldgen::structure::FrequencyReductionMethod::LegacyType3 => {
                        legacy_type_3 += 1
                    }
                    folia_core::worldgen::structure::FrequencyReductionMethod::Default => {}
                }
            }
            StructurePlacement::ConcentricRings(_) => concentric += 1,
        }
    }
    assert_eq!(entries, 34);
    assert_eq!(random_spread, 19);
    assert_eq!(concentric, 1);
    assert_eq!(triangular, 3);
    assert_eq!((legacy_type_1, legacy_type_2, legacy_type_3), (1, 1, 1));
    assert_eq!(
        registry.unsupported_placements(),
        vec![("minecraft:strongholds", "minecraft:concentric_rings")]
    );
    assert_eq!(
        registry
            .get("villages")
            .expect("villages set")
            .structures
            .len(),
        5
    );
    assert_eq!(
        registry
            .get("nether_complexes")
            .expect("nether set")
            .structures[1]
            .weight,
        3
    );
}

const IGLOO_CHUNKS: &[(i32, i32)] = &[
    (-62, 44),
    (-49, -57),
    (-46, -11),
    (-42, 16),
    (-26, 49),
    (-23, -46),
    (-20, 13),
    (-19, -14),
    (3, 13),
    (7, -25),
    (13, -49),
    (14, 34),
    (32, -53),
    (36, 41),
    (38, 12),
    (45, -10),
];

#[test]
fn random_spread_matches_java_for_fixed_seed() {
    let registry = StructureSetRegistry::shipped().expect("shipped sets parse");
    let set = registry.get("igloos").expect("igloos set");
    let StructurePlacement::RandomSpread(placement) = &set.placement else {
        panic!("igloos must use random_spread");
    };
    assert_eq!(placement.spacing, 32);
    assert_eq!(placement.separation, 8);
    assert_eq!(placement.salt, 14_357_618);

    let mut chunks = Vec::new();
    for x in -64..=64 {
        for z in -64..=64 {
            if placement.is_placement_chunk(12_345, x, z) {
                chunks.push((x, z));
            }
        }
    }
    assert_eq!(chunks, IGLOO_CHUNKS);

    assert_eq!(
        placement.get_potential_structure_chunk(12_345, 0, 0),
        ChunkPos::new(3, 13)
    );
    assert_eq!(
        placement.get_potential_structure_chunk(12_345, -1, 0),
        ChunkPos::new(-20, 13)
    );
    assert!(placement.is_structure_chunk(12_345, 3, 13, None));
    assert!(!placement.is_structure_chunk(12_345, 0, 0, None));
}

const END_CITY_CHUNKS: &[(i32, i32)] = &[
    (1, 81),
    (1, 142),
    (1, 163),
    (2, 43),
    (3, 1),
    (3, 65),
    (3, 184),
    (4, 22),
    (4, 121),
    (5, 102),
    (22, 64),
    (22, 85),
    (22, 161),
    (23, 145),
    (24, 6),
    (24, 26),
    (25, 41),
    (25, 104),
    (25, 123),
    (27, 182),
];

#[test]
fn triangular_spread_and_frequency_reducers_match_java() {
    let registry = StructureSetRegistry::shipped().expect("shipped sets parse");
    let end = registry.get("end_cities").expect("end cities");
    let StructurePlacement::RandomSpread(end) = &end.placement else {
        panic!("end cities must use random_spread");
    };
    assert_eq!(end.spread_type, RandomSpreadType::Triangular);
    let mut end_chunks = Vec::new();
    for x in 0..=200 {
        for z in 0..=200 {
            if end_chunks.len() < 20 && end.is_placement_chunk(12_345, x, z) {
                end_chunks.push((x, z));
            }
        }
    }
    assert_eq!(end_chunks, END_CITY_CHUNKS);
    assert_eq!(
        registry
            .get("woodland_mansions")
            .and_then(|set| match &set.placement {
                StructurePlacement::RandomSpread(placement) =>
                    Some(placement.get_potential_structure_chunk(12_345, 0, 0)),
                StructurePlacement::ConcentricRings(_) => None,
            }),
        Some(ChunkPos::new(40, 30))
    );

    let buried = registry.get("buried_treasures").expect("buried treasures");
    let StructurePlacement::RandomSpread(buried) = &buried.placement else {
        panic!("buried treasures must use random_spread");
    };
    let mut buried_chunks = Vec::new();
    for x in 0..=200 {
        for z in 0..=200 {
            if buried_chunks.len() < 10 && buried.is_structure_chunk(12_345, x, z, None) {
                buried_chunks.push((x, z));
            }
        }
    }
    assert_eq!(
        buried_chunks,
        vec![
            (0, 24),
            (0, 138),
            (1, 193),
            (2, 87),
            (4, 23),
            (4, 193),
            (5, 141),
            (5, 153),
            (6, 37),
            (6, 120)
        ]
    );

    let mineshaft = registry.get("mineshafts").expect("mineshafts");
    let StructurePlacement::RandomSpread(mineshaft) = &mineshaft.placement else {
        panic!("mineshafts must use random_spread");
    };
    let mut mineshaft_chunks = Vec::new();
    for x in 0..=200 {
        for z in 0..=200 {
            if mineshaft_chunks.len() < 10 && mineshaft.is_structure_chunk(12_345, x, z, None) {
                mineshaft_chunks.push((x, z));
            }
        }
    }
    assert_eq!(
        mineshaft_chunks,
        vec![
            (4, 83),
            (5, 107),
            (8, 112),
            (8, 167),
            (10, 97),
            (11, 194),
            (13, 88),
            (14, 30),
            (14, 48),
            (16, 11)
        ]
    );

    let outpost = registry.get("pillager_outposts").expect("outposts");
    let StructurePlacement::RandomSpread(outpost) = &outpost.placement else {
        panic!("outposts must use random_spread");
    };
    let mut outpost_chunks = Vec::new();
    for x in 0..=200 {
        for z in 0..=200 {
            if outpost_chunks.len() < 10 && outpost.is_structure_chunk(12_345, x, z, None) {
                outpost_chunks.push((x, z));
            }
        }
    }
    assert_eq!(
        outpost_chunks,
        vec![
            (5, 2),
            (9, 69),
            (11, 196),
            (53, 134),
            (81, 140),
            (108, 112),
            (180, 116)
        ]
    );
}

#[test]
fn weighted_structure_selection_matches_java() {
    let registry = StructureSetRegistry::shipped().expect("shipped sets parse");
    let villages = registry.get("villages").expect("villages");
    for (chunk, expected) in [
        ((0, 0), "minecraft:village_desert"),
        ((3, 7), "minecraft:village_plains"),
        ((-5, 9), "minecraft:village_savanna"),
        ((12, -4), "minecraft:village_desert"),
        ((100, 100), "minecraft:village_taiga"),
    ] {
        assert_eq!(
            villages
                .select_structure(12_345, chunk.0, chunk.1)
                .expect("village selection")
                .structure,
            expected,
            "village at {chunk:?}"
        );
    }

    let nether = registry.get("nether_complexes").expect("nether complexes");
    assert_eq!(
        nether
            .select_structure(12_345, 0, 0)
            .expect("nether selection")
            .structure,
        "minecraft:fortress"
    );
    assert_eq!(
        nether
            .select_structure(12_345, -5, 9)
            .expect("nether selection")
            .structure,
        "minecraft:bastion_remnant"
    );

    let portals = registry.get("ruined_portals").expect("ruined portals");
    assert_eq!(
        portals
            .select_structure(12_345, -5, 9)
            .expect("portal selection")
            .structure,
        "minecraft:ruined_portal_mountain"
    );
}

#[test]
fn structure_at_chunk_selects_only_placement_chunks() {
    let registry = StructureSetRegistry::shipped().expect("shipped sets parse");

    let igloo = registry
        .structure_at_chunk(3, 13, 12_345)
        .expect("placement evaluation")
        .expect("igloo chunk");
    assert_eq!(igloo.set, "minecraft:igloos");
    assert_eq!(igloo.structure, "minecraft:igloo");
    assert_eq!(igloo.chunk, ChunkPos::new(3, 13));

    assert!(
        !registry
            .is_structure_chunk("igloos", 0, 0, 12_345)
            .expect("igloo placement evaluates")
    );

    let village = registry
        .structure_at_chunk(-59, 17, 12_345)
        .expect("placement evaluation")
        .expect("village chunk");
    assert_eq!(village.set, "minecraft:villages");
    assert_eq!(village.structure, "minecraft:village_plains");

    let outpost = registry
        .structure_at_chunk(5, 2, 12_345)
        .expect("placement evaluation")
        .expect("outpost chunk");
    assert_eq!(outpost.set, "minecraft:pillager_outposts");
    assert_eq!(outpost.structure, "minecraft:pillager_outpost");

    let error = registry
        .is_structure_chunk("strongholds", 0, 0, 12_345)
        .expect_err("concentric rings need a biome source");
    assert!(matches!(
        error,
        StructureSetError::Placement {
            source: PlacementError::RequiresBiomeSource { .. },
            ..
        }
    ));
}

struct StaticBiomes(&'static str);

impl StructureBiomeLookup for StaticBiomes {
    fn biome_at(&self, _x: i32, _y: i32, _z: i32) -> &str {
        self.0
    }
}

#[test]
fn desert_pyramid_set_pipeline_places_surface_body_and_four_chests() {
    let seed = 12_345;
    let structures = StructureGenerator::shipped(HashMap::new())
        .expect("structure generator")
        .with_enabled_structures(["minecraft:desert_pyramid"]);
    let desert = StaticBiomes("minecraft:desert");
    let snowy = StaticBiomes("minecraft:snowy_taiga");
    let set = structures
        .sets()
        .get("desert_pyramids")
        .expect("desert set");
    let StructurePlacement::RandomSpread(spread) = &set.placement else {
        panic!("desert pyramids must use random spread");
    };
    let chunk = spread.get_potential_structure_chunk(seed, 0, 0);

    let selection = structures
        .structure_at_chunk(
            chunk.x,
            chunk.z,
            seed,
            StructureDimension::Overworld,
            63,
            &desert,
        )
        .expect("desert selection")
        .expect("desert pyramid placement chunk");
    assert_eq!(selection.structure, "minecraft:desert_pyramid");
    assert_eq!(selection.set, "minecraft:desert_pyramids");
    assert_eq!(selection.chunk, chunk);
    assert!(
        structures
            .structure_at_chunk(
                chunk.x,
                chunk.z,
                seed,
                StructureDimension::Overworld,
                63,
                &snowy,
            )
            .expect("snowy selection")
            .is_none()
    );

    let mut height_source = MockLevel::new(0, 384);
    height_source.fixed_height = Some(64);
    let templates = HashMap::new();
    let definition = structures
        .definitions()
        .get("minecraft:desert_pyramid")
        .expect("desert definition");
    let context =
        StructureGenerationContext::new(seed, chunk, 0, &templates).with_level(&height_source);
    let start = definition
        .generate_start(&context)
        .expect("desert pyramid start");
    assert!(start.is_valid());
    let pyramid = start.desert_pyramid().expect("desert pyramid piece");
    let min_x = chunk.x << 4;
    let min_z = chunk.z << 4;
    let bounds = start.bounding_box().expect("pyramid bounds");
    assert_eq!(
        bounds,
        BoundingBox::new(min_x, 64, min_z, min_x + 20, 78, min_z + 20)
    );
    assert_eq!(pyramid.bounding_box(), bounds);

    let mut level = MockLevel::new(0, 384);
    let placement = structures
        .place_for_chunk_with_height_source(
            &mut level,
            &height_source,
            chunk,
            seed,
            StructureDimension::Overworld,
            63,
            &desert,
            8,
            Decoration::SurfaceStructures,
        )
        .expect("desert pyramid structure pass");
    assert_eq!(placement.generated_starts, 1);
    assert_eq!(placement.placed_pieces, 1);
    assert!(placement.written > 0);

    let chests: Vec<_> = placement
        .block_entities
        .iter()
        .filter(|block| block.state.name == "minecraft:chest")
        .collect();
    assert_eq!(chests.len(), 4);
    let base_y = chests[0].pos.y() + 11;
    assert!((62..=64).contains(&base_y));
    let mut chest_positions: Vec<_> = chests
        .iter()
        .map(|chest| (chest.pos.x(), chest.pos.y(), chest.pos.z()))
        .collect();
    chest_positions.sort_unstable();
    let mut expected_positions = vec![
        (min_x + 8, base_y - 11, min_z + 10),
        (min_x + 10, base_y - 11, min_z + 8),
        (min_x + 10, base_y - 11, min_z + 12),
        (min_x + 12, base_y - 11, min_z + 10),
    ];
    expected_positions.sort_unstable();
    assert_eq!(chest_positions, expected_positions);
    for chest in chests {
        assert!(chest.nbt.as_ref().is_some_and(|nbt| {
            nbt.get("LootTable").and_then(Tag::as_str) == Some("minecraft:chests/desert_pyramid")
        }));
    }
    assert_eq!(
        level
            .get_block(BlockPos::new(min_x, base_y, min_z))
            .expect("pyramid floor")
            .name,
        "minecraft:sandstone"
    );
    assert_eq!(
        level
            .get_block(BlockPos::new(min_x + 10, base_y, min_z + 10))
            .expect("pyramid floor center")
            .name,
        "minecraft:blue_terracotta"
    );
}

#[test]
fn jungle_pyramid_set_pipeline_places_temple_traps_and_two_loot_chests() {
    let seed = 12_345;
    let structures = StructureGenerator::shipped(HashMap::new())
        .expect("structure generator")
        .with_enabled_structures(["minecraft:jungle_pyramid"]);
    let jungle = StaticBiomes("minecraft:jungle");
    let desert = StaticBiomes("minecraft:desert");
    let set = structures
        .sets()
        .get("jungle_temples")
        .expect("jungle temple set");
    let StructurePlacement::RandomSpread(spread) = &set.placement else {
        panic!("jungle temples must use random spread");
    };
    let chunk = spread.get_potential_structure_chunk(seed, 0, 0);
    assert_eq!(chunk, ChunkPos::new(2, 13));
    assert_eq!(spread.spacing, 32);
    assert_eq!(spread.separation, 8);
    assert_eq!(spread.salt, 14_357_619);

    let selection = structures
        .structure_at_chunk(
            chunk.x,
            chunk.z,
            seed,
            StructureDimension::Overworld,
            63,
            &jungle,
        )
        .expect("jungle selection")
        .expect("jungle pyramid placement chunk");
    assert_eq!(selection.structure, "minecraft:jungle_pyramid");
    assert_eq!(selection.set, "minecraft:jungle_temples");
    assert_eq!(selection.chunk, chunk);
    assert!(
        structures
            .structure_at_chunk(
                chunk.x,
                chunk.z,
                seed,
                StructureDimension::Overworld,
                63,
                &desert,
            )
            .expect("desert selection")
            .is_none()
    );

    let definition = structures
        .definitions()
        .get("minecraft:jungle_pyramid")
        .expect("jungle pyramid definition");
    assert_eq!(definition.kind, StructureKind::JunglePyramid);
    assert_eq!(definition.step(), Decoration::SurfaceStructures);
    assert_eq!(
        StructureDefinition::from_json("minecraft:jungle_pyramid", JUNGLE_PYRAMID_JSON)
            .expect("jungle pyramid JSON")
            .kind,
        StructureKind::JunglePyramid
    );

    let mut height_source = MockLevel::new(-64, 384);
    height_source.fixed_height = Some(68);
    let templates = HashMap::new();
    let context =
        StructureGenerationContext::new(seed, chunk, 0, &templates).with_level(&height_source);
    let start = definition
        .generate_start(&context)
        .expect("jungle pyramid start");
    assert!(start.is_valid());
    let pyramid = start.jungle_pyramid().expect("jungle pyramid piece");
    let min_x = chunk.x << 4;
    let min_z = chunk.z << 4;
    let bounds = start.bounding_box().expect("jungle pyramid bounds");
    let (width, depth) = match pyramid.direction() {
        Direction::North | Direction::South => {
            (JunglePyramidStart::WIDTH, JunglePyramidStart::DEPTH)
        }
        Direction::East | Direction::West => (JunglePyramidStart::DEPTH, JunglePyramidStart::WIDTH),
        _ => panic!("jungle pyramid must face horizontally"),
    };
    assert_eq!(
        bounds,
        BoundingBox::new(
            min_x,
            JunglePyramidStart::GENERATION_HEIGHT,
            min_z,
            min_x + width - 1,
            JunglePyramidStart::GENERATION_HEIGHT + JunglePyramidStart::HEIGHT - 1,
            min_z + depth - 1,
        )
    );
    assert_eq!(pyramid.bounding_box(), bounds);

    let mut level = MockLevel::new(-64, 384);
    level.fixed_height = Some(64);
    let placement = structures
        .place_for_chunk_with_height_source(
            &mut level,
            &height_source,
            chunk,
            seed,
            StructureDimension::Overworld,
            63,
            &jungle,
            0,
            Decoration::SurfaceStructures,
        )
        .expect("jungle pyramid structure pass");
    assert_eq!(placement.generated_starts, 1);
    assert_eq!(placement.placed_pieces, 1);
    assert_eq!(placement.skipped_pieces, 0);
    assert_eq!(placement.rejected, 0);
    assert!(placement.written > 0);

    let base_y = 68;
    for (local, expected) in [
        ((0, -4, 0), "minecraft:mossy_cobblestone"),
        ((4, 9, 4), "minecraft:mossy_cobblestone"),
        ((5, 9, 6), "minecraft:cobblestone_stairs"),
        ((5, 1, 5), "minecraft:air"),
        ((5, -2, 13), "minecraft:air"),
    ] {
        let pos = jungle_world_pos(min_x, base_y, min_z, pyramid.direction(), local);
        assert!(
            matches!(
                level.get_block(pos).expect("sampled jungle temple block").name.as_str(),
                name if name == expected
                    || (matches!(local, (0, -4, 0) | (4, 9, 4))
                        && name == "minecraft:cobblestone")
            ),
            "unexpected block at local {local:?}: {:?}",
            level.get_block(pos)
        );
    }
    assert!(placement.written_blocks.iter().any(|pos| {
        level
            .get_block(*pos)
            .is_some_and(|block| block.name == "minecraft:mossy_cobblestone")
    }));

    let chests: Vec<_> = placement
        .block_entities
        .iter()
        .filter(|block| block.state.name == "minecraft:chest")
        .collect();
    assert_eq!(chests.len(), 2);
    let mut chest_positions: Vec<_> = chests
        .iter()
        .map(|chest| (chest.pos.x(), chest.pos.y(), chest.pos.z()))
        .collect();
    chest_positions.sort_unstable();
    let expected_chests = [
        jungle_world_pos(min_x, base_y, min_z, pyramid.direction(), (8, -3, 3)),
        jungle_world_pos(min_x, base_y, min_z, pyramid.direction(), (9, -3, 10)),
    ];
    let mut expected_chests: Vec<_> = expected_chests
        .into_iter()
        .map(|pos| (pos.x(), pos.y(), pos.z()))
        .collect();
    expected_chests.sort_unstable();
    assert_eq!(chest_positions, expected_chests);
    for chest in chests {
        assert!(chest.nbt.as_ref().is_some_and(|nbt| {
            nbt.get("id").and_then(Tag::as_str) == Some("minecraft:chest")
                && nbt.get("LootTable").and_then(Tag::as_str)
                    == Some("minecraft:chests/jungle_temple")
                && nbt.get("LootTableSeed").and_then(Tag::as_long).is_some()
        }));
    }

    let dispensers: Vec<_> = placement
        .block_entities
        .iter()
        .filter(|block| block.state.name == "minecraft:dispenser")
        .collect();
    assert_eq!(dispensers.len(), 2);
    for dispenser in dispensers {
        assert!(dispenser.nbt.as_ref().is_some_and(|nbt| {
            nbt.get("id").and_then(Tag::as_str) == Some("minecraft:dispenser")
                && nbt.get("LootTable").and_then(Tag::as_str)
                    == Some("minecraft:chests/jungle_temple_dispenser")
                && nbt.get("LootTableSeed").and_then(Tag::as_long).is_some()
        }));
    }
}

fn jungle_world_pos(
    min_x: i32,
    base_y: i32,
    min_z: i32,
    direction: Direction,
    (x, y, z): (i32, i32, i32),
) -> BlockPos {
    let (world_x, world_z) = match direction {
        Direction::North => (min_x + x, min_z + JunglePyramidStart::DEPTH - 1 - z),
        Direction::South => (min_x + x, min_z + z),
        Direction::West => (min_x + JunglePyramidStart::WIDTH - 1 - z, min_z + x),
        Direction::East => (min_x + z, min_z + JunglePyramidStart::DEPTH - 1 - x),
        _ => (min_x + x, min_z + z),
    };
    BlockPos::new(world_x, base_y + y, world_z)
}

#[test]
fn jungle_pyramid_averages_the_height_source_and_skips_missing_heights() {
    let piece = JunglePyramidPiece::new(12_345, 8, 0, Direction::North);
    let mut source = MockLevel::new(-64, 384);
    for x in 8..16 {
        for z in 0..JunglePyramidStart::DEPTH {
            source.heights.insert((x, z), 60 + x - 8);
        }
    }
    let mut level = MockLevel::new(-64, 384);
    level.fixed_height = Some(100);
    let chunk_box = BoundingBox::new(0, -64, 0, 15, 319, 15);
    let placement = piece
        .place_with_height_source(&mut level, &source, chunk_box)
        .expect("jungle pyramid places from averaged heights");
    assert_eq!(placement.placed_pieces, 1);
    assert_eq!(placement.rejected, 0);
    let floor = level
        .get_block(BlockPos::new(8, 59, 14))
        .expect("temple floor at average height minus four");
    assert!(matches!(
        floor.name.as_str(),
        "minecraft:mossy_cobblestone" | "minecraft:cobblestone"
    ));
    assert!(placement.written_blocks.contains(&BlockPos::new(8, 59, 14)));

    let missing = MockLevel::new(-64, 384);
    let skipped = piece
        .place_with_height_source(&mut level, &missing, chunk_box)
        .expect("missing height source skips the piece");
    assert_eq!(skipped.placed_pieces, 0);
    assert_eq!(skipped.written, 0);

    let outside = piece
        .place_with_height_source(
            &mut level,
            &source,
            BoundingBox::new(32, -64, 32, 47, 319, 47),
        )
        .expect("non-intersecting chunk skips the piece");
    assert_eq!(outside.placed_pieces, 0);
}

#[test]
fn witch_hut_set_pipeline_places_supported_spruce_hut_and_cauldron() {
    let seed = 12_345;
    let structures = StructureGenerator::shipped(HashMap::new())
        .expect("structure generator")
        .with_enabled_structures(["minecraft:swamp_hut"]);
    let swamp = StaticBiomes("minecraft:swamp");
    let desert = StaticBiomes("minecraft:desert");
    let set = structures.sets().get("swamp_huts").expect("swamp hut set");
    let StructurePlacement::RandomSpread(spread) = &set.placement else {
        panic!("swamp huts must use random spread");
    };
    let chunk = spread.get_potential_structure_chunk(seed, 0, 0);
    assert_eq!(chunk, ChunkPos::new(12, 21));
    assert_eq!(spread.spacing, 32);
    assert_eq!(spread.separation, 8);
    assert_eq!(spread.salt, 14_357_620);

    let selection = structures
        .structure_at_chunk(
            chunk.x,
            chunk.z,
            seed,
            StructureDimension::Overworld,
            63,
            &swamp,
        )
        .expect("swamp hut selection")
        .expect("swamp hut placement chunk");
    assert_eq!(selection.structure, "minecraft:swamp_hut");
    assert_eq!(selection.set, "minecraft:swamp_huts");
    assert_eq!(selection.chunk, chunk);
    assert!(
        structures
            .structure_at_chunk(
                chunk.x,
                chunk.z,
                seed,
                StructureDimension::Overworld,
                63,
                &desert,
            )
            .expect("desert selection")
            .is_none()
    );

    let mut height_source = MockLevel::new(-64, 384);
    height_source.fixed_height = Some(64);
    let templates = HashMap::new();
    let definition = structures
        .definitions()
        .get("minecraft:swamp_hut")
        .expect("swamp hut definition");
    assert_eq!(definition.kind, StructureKind::WitchHut);
    assert_eq!(definition.step(), Decoration::SurfaceStructures);
    let context =
        StructureGenerationContext::new(seed, chunk, 0, &templates).with_level(&height_source);
    let start = definition
        .generate_start(&context)
        .expect("swamp hut start");
    assert!(start.is_valid());
    let hut = start.witch_hut().expect("witch hut piece");
    let min_x = chunk.x << 4;
    let min_z = chunk.z << 4;
    let bounds = start.bounding_box().expect("witch hut bounds");
    assert_eq!(
        bounds,
        BoundingBox::new(
            min_x,
            WitchHutStart::GENERATION_HEIGHT - WitchHutStart::FLOOR_OFFSET,
            min_z,
            min_x + 4,
            WitchHutStart::GENERATION_HEIGHT + 6,
            min_z + 4,
        )
    );
    assert_eq!(hut.bounding_box(), bounds);

    let mut level = MockLevel::new(-64, 384);
    for x in min_x..=min_x + 4 {
        for z in min_z..=min_z + 4 {
            level.set_block(
                BlockPos::new(x, 63, z),
                BlockState {
                    name: "minecraft:grass_block".to_owned(),
                    properties: Vec::new(),
                },
            );
        }
    }
    let placement = structures
        .place_for_chunk_with_height_source(
            &mut level,
            &height_source,
            chunk,
            seed,
            StructureDimension::Overworld,
            63,
            &swamp,
            0,
            Decoration::SurfaceStructures,
        )
        .expect("witch hut structure pass");
    assert_eq!(placement.generated_starts, 1);
    assert_eq!(placement.placed_pieces, 1);
    assert_eq!(placement.skipped_pieces, 0);
    assert_eq!(placement.rejected, 0);
    assert!(placement.written > 0);

    let floor_y = 67;
    for (pos, expected) in [
        (
            BlockPos::new(min_x + 2, floor_y, min_z + 2),
            "minecraft:spruce_planks",
        ),
        (
            BlockPos::new(min_x, floor_y + 3, min_z),
            "minecraft:spruce_log",
        ),
        (
            BlockPos::new(min_x + 2, floor_y + 1, min_z + 4),
            "minecraft:air",
        ),
        (
            BlockPos::new(min_x + 2, floor_y + 2, min_z + 4),
            "minecraft:air",
        ),
        (
            BlockPos::new(min_x, floor_y + 2, min_z + 2),
            "minecraft:glass_pane",
        ),
        (
            BlockPos::new(min_x + 2, floor_y + 6, min_z + 2),
            "minecraft:spruce_stairs",
        ),
        (
            BlockPos::new(min_x, floor_y - 3, min_z),
            "minecraft:spruce_log",
        ),
        (
            BlockPos::new(min_x, floor_y - 1, min_z),
            "minecraft:spruce_log",
        ),
    ] {
        assert_eq!(
            level.get_block(pos).expect("sampled witch hut block").name,
            expected,
            "block at {pos:?}"
        );
    }

    let cauldrons: Vec<_> = placement
        .block_entities
        .iter()
        .filter(|block| block.state.name == "minecraft:cauldron")
        .collect();
    assert_eq!(cauldrons.len(), 1);
    let cauldron = cauldrons[0];
    assert_eq!(
        cauldron.pos,
        BlockPos::new(min_x + 2, floor_y + 1, min_z + 2)
    );
    assert!(
        cauldron.nbt.as_ref().is_some_and(|nbt| {
            nbt.get("id").and_then(Tag::as_str) == Some("minecraft:cauldron")
        })
    );
    assert!(placement.written_blocks.contains(&cauldron.pos));
}

#[test]
fn buried_treasure_set_pipeline_places_chest_and_surroundings_at_height_source() {
    let seed = 12_345;
    let structures = StructureGenerator::shipped(HashMap::new())
        .expect("structure generator")
        .with_enabled_structures(["minecraft:buried_treasure"]);
    let beach = StaticBiomes("minecraft:beach");
    let desert = StaticBiomes("minecraft:desert");
    let set = structures
        .sets()
        .get("buried_treasures")
        .expect("buried treasure set");
    let StructurePlacement::RandomSpread(spread) = &set.placement else {
        panic!("buried treasures must use random spread");
    };
    let chunk = ChunkPos::new(0, 24);
    let center_x = (chunk.x << 4) + 9;
    let center_z = (chunk.z << 4) + 9;
    assert!(spread.is_structure_chunk(seed, chunk.x, chunk.z, None));

    let selection = structures
        .structure_at_chunk(
            chunk.x,
            chunk.z,
            seed,
            StructureDimension::Overworld,
            63,
            &beach,
        )
        .expect("buried treasure selection")
        .expect("buried treasure placement chunk");
    assert_eq!(selection.structure, "minecraft:buried_treasure");
    assert_eq!(selection.set, "minecraft:buried_treasures");
    assert_eq!(selection.chunk, chunk);
    assert!(
        structures
            .structure_at_chunk(
                chunk.x,
                chunk.z,
                seed,
                StructureDimension::Overworld,
                63,
                &desert,
            )
            .expect("desert selection")
            .is_none()
    );

    let mut height_source = MockLevel::new(0, 384);
    height_source.fixed_height = Some(64);
    let templates = HashMap::new();
    let definition = structures
        .definitions()
        .get("minecraft:buried_treasure")
        .expect("buried treasure definition");
    assert_eq!(
        definition.kind,
        folia_core::worldgen::structure::StructureKind::BuriedTreasure
    );
    assert_eq!(definition.step(), Decoration::UndergroundStructures);
    let context =
        StructureGenerationContext::new(seed, chunk, 0, &templates).with_level(&height_source);
    let start = definition
        .generate_start(&context)
        .expect("buried treasure start");
    assert!(start.is_valid());
    let treasure = start.buried_treasure().expect("buried treasure piece");
    let bounds = start.bounding_box().expect("buried treasure bounds");
    assert_eq!(
        bounds,
        BoundingBox::new(center_x, 90, center_z, center_x, 90, center_z),
        "the saved start box remains the generation hint"
    );
    assert_eq!(treasure.bounding_box(), bounds);

    let mut level = MockLevel::new(0, 384);
    level.fixed_height = Some(100);
    for (x, z, block) in [
        (center_x, center_z, "minecraft:sandstone"),
        (center_x, center_z - 1, "minecraft:stone"),
        (center_x + 1, center_z, "minecraft:sandstone"),
        (center_x - 1, center_z, "minecraft:stone"),
    ] {
        level.set_block(
            BlockPos::new(x, 63, z),
            BlockState {
                name: block.to_owned(),
                properties: Vec::new(),
            },
        );
    }
    level.set_block(
        BlockPos::new(center_x - 1, 64, center_z),
        BlockState {
            name: "minecraft:chiseled_sandstone".to_owned(),
            properties: Vec::new(),
        },
    );
    level.set_block(
        BlockPos::new(center_x, 64, center_z - 1),
        BlockState {
            name: "minecraft:water".to_owned(),
            properties: Vec::new(),
        },
    );
    assert_eq!(
        treasure.find_chest_position(&height_source, &level),
        Some(BlockPos::new(center_x, 64, center_z))
    );

    let placement = structures
        .place_for_chunk_with_height_source(
            &mut level,
            &height_source,
            chunk,
            seed,
            StructureDimension::Overworld,
            63,
            &beach,
            0,
            Decoration::UndergroundStructures,
        )
        .expect("buried treasure structure pass");
    assert_eq!(placement.generated_starts, 1);
    assert_eq!(placement.placed_pieces, 1);
    assert_eq!(placement.skipped_pieces, 0);
    assert_eq!(placement.rejected, 0);
    assert_eq!(placement.written, 5);
    assert_eq!(placement.written_blocks.len(), 5);

    let chests: Vec<_> = placement
        .block_entities
        .iter()
        .filter(|block| block.state.name == "minecraft:chest")
        .collect();
    assert_eq!(chests.len(), 1);
    let chest = chests[0];
    assert_eq!(chest.pos, BlockPos::new(center_x, 64, center_z));
    assert!(
        chest
            .state
            .properties
            .contains(&("facing".to_owned(), "east".to_owned()))
    );
    assert!(chest.nbt.as_ref().is_some_and(|nbt| {
        nbt.get("id").and_then(Tag::as_str) == Some("minecraft:chest")
            && nbt.get("LootTable").and_then(Tag::as_str)
                == Some("minecraft:chests/buried_treasure")
    }));

    for (pos, expected) in [
        (BlockPos::new(center_x, 63, center_z), "minecraft:sandstone"),
        (BlockPos::new(center_x - 1, 63, center_z), "minecraft:stone"),
        (
            BlockPos::new(center_x - 1, 64, center_z),
            "minecraft:chiseled_sandstone",
        ),
        (BlockPos::new(center_x, 64, center_z - 1), "minecraft:sand"),
        (
            BlockPos::new(center_x, 64, center_z + 1),
            "minecraft:sandstone",
        ),
        (BlockPos::new(center_x, 65, center_z), "minecraft:sand"),
        (BlockPos::new(center_x + 1, 64, center_z), "minecraft:sand"),
    ] {
        assert_eq!(
            level.get_block(pos).expect("sampled treasure block").name,
            expected,
            "block at {pos:?}"
        );
    }
}

#[test]
fn exclusion_zones_forbid_overlapping_sets() {
    let registry = StructureSetRegistry::from_json_documents([
        (
            "a",
            r#"{ "placement": { "type": "minecraft:random_spread", "salt": 1, "spacing": 1, "separation": 0 }, "structures": [ { "structure": "minecraft:a", "weight": 1 } ] }"#,
        ),
        (
            "b",
            r#"{ "placement": { "type": "minecraft:random_spread", "salt": 2, "spacing": 1, "separation": 0, "exclusion_zone": { "other_set": "a", "chunk_count": 1 } }, "structures": [ { "structure": "minecraft:b", "weight": 1 } ] }"#,
        ),
    ])
    .expect("synthetic sets parse");
    assert!(
        registry
            .is_structure_chunk("a", 0, 0, 12_345)
            .expect("a evaluates")
    );
    assert!(
        !registry
            .is_structure_chunk("b", 0, 0, 12_345)
            .expect("b evaluates")
    );
}

fn generate_igloo(
    chunk_x: i32,
    chunk_z: i32,
) -> (
    StructureDefinition,
    HashMap<String, StructureTemplate>,
    folia_core::worldgen::structure::StructureStart,
) {
    let definition =
        StructureDefinition::from_json("minecraft:igloo", IGLOO_JSON).expect("igloo JSON parses");
    assert_eq!(
        definition.kind,
        folia_core::worldgen::structure::StructureKind::Igloo
    );
    let templates = igloo_templates();
    let context =
        StructureGenerationContext::new(12_345, ChunkPos::new(chunk_x, chunk_z), 0, &templates);
    let start = definition
        .generate_start(&context)
        .expect("igloo start generates");
    (definition, templates, start)
}

#[test]
fn igloo_generation_is_deterministic_with_expected_pieces_and_bounds() {
    let (definition, _templates, start) = generate_igloo(-20, -20);
    assert_eq!(start.structure_id(), "minecraft:igloo");
    assert_eq!(start.chunk_pos(), ChunkPos::new(-20, -20));
    assert!(start.is_valid());
    assert_eq!(start.pieces().len(), 7);
    assert_eq!(start.references(), 0);

    let names: Vec<&str> = start
        .pieces()
        .iter()
        .map(|piece| {
            piece
                .as_template()
                .expect("template piece")
                .template_name
                .as_str()
        })
        .collect();
    assert_eq!(
        names,
        vec![
            IglooStructure::BOTTOM_TEMPLATE,
            IglooStructure::MIDDLE_TEMPLATE,
            IglooStructure::MIDDLE_TEMPLATE,
            IglooStructure::MIDDLE_TEMPLATE,
            IglooStructure::MIDDLE_TEMPLATE,
            IglooStructure::MIDDLE_TEMPLATE,
            IglooStructure::TOP_TEMPLATE,
        ]
    );
    for piece in start.pieces() {
        assert_eq!(piece.rotation(), Rotation::None);
        assert_eq!(
            piece.orientation(),
            Some(folia_core::math::Direction::North)
        );
    }

    assert_eq!(
        start.bounding_box(),
        Some(BoundingBox::new(-320, 69, -322, -314, 94, -313))
    );
    assert_eq!(start.pieces_bounding_box(), start.bounding_box());

    let templates = igloo_templates();
    let context = StructureGenerationContext::new(12_345, ChunkPos::new(-20, -20), 0, &templates);
    let again = definition
        .generate_start(&context)
        .expect("second generation succeeds");
    assert_eq!(start, again);

    let mut referenced = start;
    assert!(referenced.try_reference());
    assert!(!referenced.try_reference());
    assert_eq!(referenced.references(), 1);
}

#[test]
fn igloo_rotation_and_bounds_match_java_for_a_single_piece() {
    let (_definition, _templates, start) = generate_igloo(0, 0);
    assert_eq!(start.pieces().len(), 1);
    assert_eq!(start.pieces()[0].rotation(), Rotation::Clockwise90);
    assert_eq!(
        start.bounding_box(),
        Some(BoundingBox::new(1, 90, 2, 8, 94, 8))
    );
}

#[test]
fn template_piece_places_blocks_at_a_fixed_origin() {
    let template = load_template(IGLOO_MIDDLE_NBT);
    let settings = StructurePlaceSettings::new()
        .with_rotation(Rotation::None)
        .with_rotation_pivot(BlockPos::new(1, 3, 1));
    let piece = StructurePiece::template(TemplateStructurePiece::new(
        IglooStructure::MIDDLE_TEMPLATE,
        template,
        BlockPos::new(100, 64, 200),
        settings,
    ));
    assert_eq!(piece.rotation(), Rotation::None);
    assert_eq!(piece.mirror(), Mirror::None);
    assert_eq!(
        piece.bounding_box(),
        BoundingBox::new(100, 64, 200, 102, 66, 202)
    );

    let mut level = MockLevel::new(0, 384);
    let chunk_box = BoundingBox::new(96, 60, 196, 112, 72, 212);
    let placement = piece
        .place(&mut level, chunk_box)
        .expect("template piece places")
        .expect("piece intersects the box");
    assert_eq!(placement.written, 15);
    assert_eq!(placement.rejected, 0);
    assert_eq!(level.blocks.len(), 15);
    assert!(placement.entities.is_empty());
    assert!(placement.data_markers.is_empty());

    let block = level
        .get_block(BlockPos::new(100, 64, 201))
        .expect("middle-template stone bricks at relative (0, 0, 1)");
    assert_eq!(block.name, "minecraft:stone_bricks");
    let ladders: Vec<&BlockState> = level
        .blocks
        .values()
        .filter(|state| state.name == "minecraft:ladder")
        .collect();
    assert_eq!(ladders.len(), 3);
    for ladder in ladders {
        assert!(
            ladder
                .properties
                .contains(&("facing".to_owned(), "north".to_owned()))
        );
    }
}

#[test]
fn template_structure_block_data_markers_are_returned() {
    let template = load_template(IGLOO_BOTTOM_NBT);
    let settings = StructurePlaceSettings::new()
        .with_rotation(Rotation::None)
        .with_rotation_pivot(BlockPos::new(3, 6, 7));
    let piece = StructurePiece::template(
        TemplateStructurePiece::new(
            IglooStructure::BOTTOM_TEMPLATE,
            template,
            BlockPos::new(100, 64, 200),
            settings,
        )
        .with_block_filter(
            folia_core::worldgen::structure::TemplateBlockFilter::IgnoreStructureBlock,
        ),
    );
    let mut level = MockLevel::new(0, 384);
    let chunk_box = BoundingBox::new(90, 50, 190, 120, 80, 220);
    let placement = piece
        .place(&mut level, chunk_box)
        .expect("bottom piece places")
        .expect("piece intersects the box");

    let marker = placement
        .data_markers
        .iter()
        .find(|marker| marker.name == "chest")
        .expect("bottom template chest marker");
    assert_eq!(marker.pos, BlockPos::new(101, 66, 206));
    assert!(
        level.get_block(marker.pos).is_none(),
        "structure blocks are ignored by the template filter"
    );
    assert!(
        level
            .get_block(BlockPos::new(101, 65, 206))
            .is_some_and(|state| state.name == "minecraft:chest")
    );
}

#[test]
fn igloo_start_places_into_a_chunk_at_the_surface() {
    let (_definition, _templates, start) = generate_igloo(0, 0);
    let mut level = MockLevel::new(0, 384);
    // The top template snaps its y=90 generation height to y=63, exactly like
    // IglooPieces.IglooPiece.postProcess with world height 64.
    level.fixed_height = Some(64);

    let placement = start
        .place_in_chunk(&mut level, ChunkPos::new(0, 0))
        .expect("igloo places");
    assert_eq!(placement.placed_pieces, 1);
    assert_eq!(placement.skipped_pieces, 0);
    assert_eq!(placement.rejected, 0);
    assert!(!level.blocks.is_empty());
    assert!(
        placement.data_markers.is_empty(),
        "the top template has no structure-block data marker"
    );

    let top = load_template(IGLOO_TOP_NBT);
    let settings = StructurePlaceSettings::new()
        .with_rotation(Rotation::Clockwise90)
        .with_rotation_pivot(BlockPos::new(3, 5, 5));
    let expected: Vec<_> = top
        .placement_blocks(&settings)
        .into_iter()
        .filter(|block| block.state.name != "minecraft:structure_block")
        .filter(|block| {
            let pos = BlockPos::new(0, 63, 0).offset_by(block.pos);
            BoundingBox::new(0, 0, 0, 15, 383, 15).is_inside(pos)
        })
        .collect();
    assert_eq!(placement.written, expected.len());
}

#[test]
fn jigsaw_structure_generation_fails_explicitly() {
    let definition =
        StructureDefinition::from_json("minecraft:village_plains", VILLAGE_PLAINS_JSON)
            .expect("jigsaw definition parses");
    assert!(matches!(
        definition.kind,
        folia_core::worldgen::structure::StructureKind::Jigsaw { .. }
    ));

    let templates = HashMap::new();
    let context = StructureGenerationContext::new(12_345, ChunkPos::new(-59, 17), 0, &templates);
    let error = definition
        .generate_start(&context)
        .expect_err("jigsaw expansion without a level must fail loudly");
    assert!(matches!(
        error,
        StructureError::MissingHeightSource { ref structure }
            if structure == "minecraft:village_plains"
    ));

    // With a level the shipped start pool is selected, but a minimal template
    // provider does not contain the chosen town-center NBT. That must surface
    // as a missing-template error, never as an empty valid start.
    let level = MockLevel::new(0, 384);
    let context = StructureGenerationContext::new(12_345, ChunkPos::new(-59, 17), 0, &templates)
        .with_level(&level);
    let error = definition
        .generate_start(&context)
        .expect_err("missing jigsaw templates must fail loudly");
    assert!(matches!(
        error,
        StructureError::UnknownTemplate { ref structure, .. }
            if structure == "minecraft:village/plains/town_centers"
    ));
}

#[test]
fn igloo_generation_fails_on_missing_templates() {
    let definition =
        StructureDefinition::from_json("minecraft:igloo", IGLOO_JSON).expect("igloo parses");
    let templates = HashMap::new();
    let context = StructureGenerationContext::new(12_345, ChunkPos::new(0, 0), 0, &templates);
    let error = definition
        .generate_start(&context)
        .expect_err("missing templates must fail");
    assert!(matches!(
        error,
        StructureError::UnknownTemplate {
            structure,
            template
        } if structure == "minecraft:igloo" && template == IglooStructure::TOP_TEMPLATE
    ));
}

#[test]
fn ocean_ruin_definitions_and_selection_cover_cold_and_warm_biomes() {
    for (name, json, warm, biomes) in [
        (
            "minecraft:ocean_ruin_cold",
            OCEAN_RUIN_COLD_JSON,
            false,
            "#minecraft:has_structure/ocean_ruin_cold",
        ),
        (
            "minecraft:ocean_ruin_warm",
            OCEAN_RUIN_WARM_JSON,
            true,
            "#minecraft:has_structure/ocean_ruin_warm",
        ),
    ] {
        let definition =
            StructureDefinition::from_json(name, json).expect("ocean ruin JSON parses");
        assert_eq!(definition.kind, StructureKind::OceanRuin { warm });
        assert_eq!(definition.step(), Decoration::SurfaceStructures);
        assert_eq!(definition.biomes, BiomeSet::Tag(biomes.to_owned()));
    }

    let structures = StructureGenerator::shipped(HashMap::new()).expect("ocean ruin generator");
    let set = structures
        .sets()
        .get("ocean_ruins")
        .expect("ocean ruin set");
    let StructurePlacement::RandomSpread(spread) = &set.placement else {
        panic!("ocean ruins must use random spread");
    };
    assert_eq!(spread.spacing, 20);
    assert_eq!(spread.separation, 8);
    assert_eq!(spread.salt, 14_357_621);

    let cold_chunk = ChunkPos::new(-120, -9);
    let warm_chunk = ChunkPos::new(-120, -39);

    for (chunk, biome, wrong_biome, expected) in [
        (
            cold_chunk,
            "minecraft:ocean",
            "minecraft:warm_ocean",
            "minecraft:ocean_ruin_cold",
        ),
        (
            warm_chunk,
            "minecraft:warm_ocean",
            "minecraft:ocean",
            "minecraft:ocean_ruin_warm",
        ),
    ] {
        let selection = structures
            .structure_at_chunk(
                chunk.x,
                chunk.z,
                12_345,
                StructureDimension::Overworld,
                63,
                &StaticBiomes(biome),
            )
            .expect("ocean ruin selection")
            .expect("matching ocean ruin biome");
        assert_eq!(selection.structure, expected);
        assert_eq!(selection.set, "minecraft:ocean_ruins");
        assert_eq!(selection.chunk, chunk);
        assert!(
            structures
                .structure_at_chunk(
                    chunk.x,
                    chunk.z,
                    12_345,
                    StructureDimension::Overworld,
                    63,
                    &StaticBiomes(wrong_biome),
                )
                .expect("mismatched ocean ruin biome")
                .is_none()
        );
        assert!(
            structures
                .structure_at_chunk(
                    chunk.x,
                    chunk.z,
                    12_345,
                    StructureDimension::Nether,
                    63,
                    &StaticBiomes(biome),
                )
                .expect("Nether ocean ruin selection")
                .is_none()
        );
    }
}

#[test]
fn ocean_ruin_fixed_seed_pins_bounds_ocean_floor_height_and_two_chest_markers() {
    let seed = 12_345;
    let structures = StructureGenerator::shipped(HashMap::new())
        .expect("ocean ruin generator")
        .with_enabled_structures(["minecraft:ocean_ruin_cold"]);
    let ocean = StaticBiomes("minecraft:ocean");
    let definition = structures
        .definitions()
        .get("minecraft:ocean_ruin_cold")
        .expect("cold ocean ruin definition");
    let mut height_source = MockLevel::new(-64, 384);
    height_source.fixed_height = Some(40);
    let templates = HashMap::new();
    let chunk = ChunkPos::new(-120, -9);
    let selection = structures
        .structure_at_chunk(
            chunk.x,
            chunk.z,
            seed,
            StructureDimension::Overworld,
            63,
            &ocean,
        )
        .expect("ocean ruin selection")
        .expect("cold ocean ruin placement chunk");
    assert_eq!(selection.structure, "minecraft:ocean_ruin_cold");
    let context =
        StructureGenerationContext::new(seed, chunk, 0, &templates).with_level(&height_source);
    let start = definition
        .generate_start(&context)
        .expect("cold ocean ruin start");
    assert!(start.is_valid());
    assert_eq!(start.pieces().len(), 0);
    let ruin = start.ocean_ruin().expect("ocean ruin piece");
    assert!(!ruin.warm());
    assert_eq!(ruin.rotation(), Rotation::None);
    assert_eq!(ruin.chest_count(), 2);
    let min_x = chunk.x << 4;
    let min_z = chunk.z << 4;
    assert_eq!(
        start.bounding_box(),
        Some(BoundingBox::new(
            min_x,
            OceanRuinStart::GENERATION_HEIGHT - OceanRuinStart::FOUNDATION_DEPTH,
            min_z,
            min_x + OceanRuinStart::WIDTH - 1,
            OceanRuinStart::GENERATION_HEIGHT + OceanRuinStart::HEIGHT - 1,
            min_z + OceanRuinStart::DEPTH - 1,
        ))
    );
    assert_eq!(ruin.bounding_box(), start.bounding_box().unwrap());
    assert_eq!(
        definition
            .generate_start(&context)
            .expect("second ocean ruin start"),
        start
    );

    let mut level = MockLevel::new(-64, 384);
    level.fixed_height = Some(100);
    let placement = structures
        .place_for_chunk_with_height_source(
            &mut level,
            &height_source,
            chunk,
            seed,
            StructureDimension::Overworld,
            63,
            &ocean,
            0,
            Decoration::SurfaceStructures,
        )
        .expect("ocean ruin structure pass");
    assert_eq!(placement.generated_starts, 1);
    assert_eq!(placement.placed_pieces, 1);
    assert_eq!(placement.skipped_pieces, 0);
    assert_eq!(placement.rejected, 0);
    assert!(placement.written > 0);
    assert!(placement.written_blocks.iter().all(|pos| {
        (min_x..=min_x + 15).contains(&pos.x())
            && (min_z..=min_z + 15).contains(&pos.z())
            && (-64..=319).contains(&pos.y())
    }));
    assert!(
        level
            .get_block(BlockPos::new(min_x + 5, 39, min_z + 5))
            .expect("ocean ruin floor")
            .name
            != "minecraft:air"
    );
    assert!(
        !level
            .blocks
            .contains_key(&BlockPos::new(min_x + 5, 100, min_z + 5))
    );

    let mut chests: Vec<_> = placement
        .block_entities
        .iter()
        .filter(|block| block.state.name == "minecraft:chest")
        .cloned()
        .collect();
    chests.sort_by_key(|chest| (chest.pos.x(), chest.pos.y(), chest.pos.z()));
    assert_eq!(
        chests.iter().map(|chest| chest.pos).collect::<Vec<_>>(),
        vec![
            BlockPos::new(min_x + 5, 40, min_z + 5),
            BlockPos::new(min_x + 10, 40, min_z + 10),
        ]
    );
    for (chest, expected_seed) in chests
        .into_iter()
        .zip([6_050_390_359_932_273_478_i64, 1_792_119_960_895_996_393])
    {
        assert!(chest.nbt.as_ref().is_some_and(|nbt| {
            nbt.get("id").and_then(Tag::as_str) == Some("minecraft:chest")
                && nbt.get("LootTable").and_then(Tag::as_str) == Some("minecraft:chests/ocean_ruin")
                && nbt.get("LootTableSeed").and_then(Tag::as_long) == Some(expected_seed)
        }));
    }

    let missing = MockLevel::new(-64, 384);
    let mut skipped_level = MockLevel::new(-64, 384);
    let skipped = start
        .place_in_chunk_with_height_source(&mut skipped_level, &missing, chunk)
        .expect("missing ocean-floor heights skip placement");
    assert_eq!(skipped.placed_pieces, 0);
    assert_eq!(skipped.written, 0);
}

#[test]
fn ocean_ruin_rotation_clipping_and_material_palettes_are_deterministic() {
    let source = MockLevel {
        fixed_height: Some(40),
        ..MockLevel::new(-64, 384)
    };
    for rotation in Rotation::ALL {
        let piece = OceanRuinPiece::new(12_346, 8, 8, rotation, true);
        let bounds = piece.bounding_box();
        let (width, depth) = if rotation.swaps_horizontal_axes() {
            (OceanRuinStart::DEPTH, OceanRuinStart::WIDTH)
        } else {
            (OceanRuinStart::WIDTH, OceanRuinStart::DEPTH)
        };
        assert_eq!(bounds.x_span(), width);
        assert_eq!(bounds.z_span(), depth);
        assert_eq!(
            bounds.y_span(),
            OceanRuinStart::HEIGHT + OceanRuinStart::FOUNDATION_DEPTH
        );
        let full_box = BoundingBox::new(
            bounds.min_x(),
            -64,
            bounds.min_z(),
            bounds.max_x(),
            319,
            bounds.max_z(),
        );
        let mut full_level = MockLevel::new(-64, 384);
        let full = piece
            .place_with_height_source(&mut full_level, &source, full_box)
            .expect("full ocean ruin placement");
        assert_eq!(full.placed_pieces, 1);
        assert!(full.written > 0);

        let mut split_level = MockLevel::new(-64, 384);
        let mut split_chests = Vec::new();
        for chunk_box in [
            BoundingBox::new(0, -64, 0, 15, 319, 15),
            BoundingBox::new(16, -64, 0, 31, 319, 15),
            BoundingBox::new(0, -64, 16, 15, 319, 31),
            BoundingBox::new(16, -64, 16, 31, 319, 31),
        ] {
            let split = piece
                .place_with_height_source(&mut split_level, &source, chunk_box)
                .expect("chunk-clipped ocean ruin placement");
            assert_eq!(split.placed_pieces, 1);
            assert!(
                split
                    .written_blocks
                    .iter()
                    .all(|pos| chunk_box.is_inside(*pos))
            );
            assert!(split.written_blocks.windows(2).all(|pair| {
                (pair[0].x(), pair[0].y(), pair[0].z()) < (pair[1].x(), pair[1].y(), pair[1].z())
            }));
            split_chests.extend(
                split
                    .block_entities
                    .iter()
                    .filter(|block| block.state.name == "minecraft:chest")
                    .cloned(),
            );
        }
        assert_eq!(split_level.blocks, full_level.blocks);
        split_chests.sort_by_key(|chest| (chest.pos.x(), chest.pos.y(), chest.pos.z()));
        let mut full_chests: Vec<_> = full
            .block_entities
            .iter()
            .filter(|block| block.state.name == "minecraft:chest")
            .cloned()
            .collect();
        full_chests.sort_by_key(|chest| (chest.pos.x(), chest.pos.y(), chest.pos.z()));
        assert_eq!(split_chests, full_chests);
        assert!((1..=2).contains(&full_chests.len()));
    }

    let cold = OceanRuinPiece::new(12_346, 0, 0, Rotation::None, false);
    let warm = OceanRuinPiece::new(12_346, 0, 0, Rotation::None, true);
    let cold_box = BoundingBox::new(0, -64, 0, 20, 319, 16);
    let mut cold_level = MockLevel::new(-64, 384);
    let mut warm_level = MockLevel::new(-64, 384);
    cold.place_with_height_source(&mut cold_level, &source, cold_box)
        .expect("cold ocean ruin materials");
    warm.place_with_height_source(&mut warm_level, &source, cold_box)
        .expect("warm ocean ruin materials");
    let cold_names: std::collections::HashSet<_> = cold_level
        .blocks
        .values()
        .map(|block| block.name.as_str())
        .collect();
    let warm_names: std::collections::HashSet<_> = warm_level
        .blocks
        .values()
        .map(|block| block.name.as_str())
        .collect();
    for name in [
        "minecraft:stone_bricks",
        "minecraft:cobblestone",
        "minecraft:andesite",
        "minecraft:prismarine",
    ] {
        assert!(cold_names.contains(name), "cold ruin missing {name}");
    }
    for name in [
        "minecraft:prismarine_bricks",
        "minecraft:dark_prismarine",
        "minecraft:andesite",
        "minecraft:prismarine",
    ] {
        assert!(warm_names.contains(name), "warm ruin missing {name}");
    }
    assert!(!cold_names.contains("minecraft:dark_prismarine"));
    assert!(!warm_names.contains("minecraft:mossy_stone_bricks"));

    let mut repeated_level = MockLevel::new(-64, 384);
    cold.place_with_height_source(&mut repeated_level, &source, cold_box)
        .expect("repeated cold ocean ruin materials");
    assert_eq!(repeated_level.blocks, cold_level.blocks);
}

#[test]
fn ocean_monument_definition_and_selection_use_the_shipped_deep_ocean_tag() {
    let definition = StructureDefinition::from_json("minecraft:monument", OCEAN_MONUMENT_JSON)
        .expect("ocean monument JSON parses");
    assert_eq!(definition.kind, StructureKind::OceanMonument);
    assert_eq!(definition.step(), Decoration::SurfaceStructures);
    assert_eq!(
        definition.biomes,
        BiomeSet::Tag("#minecraft:has_structure/ocean_monument".to_owned())
    );
    assert_eq!(
        StructureGenerator::shipped(HashMap::new())
            .expect("ocean monument generator")
            .definitions()
            .get("minecraft:monument")
            .expect("shipped monument definition")
            .kind,
        StructureKind::OceanMonument
    );

    let structures = StructureGenerator::shipped(HashMap::new()).expect("monument generator");
    let set = structures
        .sets()
        .get("ocean_monuments")
        .expect("ocean monument set");
    let StructurePlacement::RandomSpread(spread) = &set.placement else {
        panic!("ocean monuments must use random spread");
    };
    assert_eq!(spread.spacing, 32);
    assert_eq!(spread.separation, 5);
    assert_eq!(spread.spread_type, RandomSpreadType::Triangular);
    assert_eq!(spread.salt, 10_387_313);

    let chunk = ChunkPos::new(42, -58);
    assert!(
        spread.is_placement_chunk(12_345, chunk.x, chunk.z),
        "potential monument chunk: {:?}",
        spread.get_potential_structure_chunk(12_345, chunk.x, chunk.z)
    );
    for biome in [
        "minecraft:deep_cold_ocean",
        "minecraft:deep_frozen_ocean",
        "minecraft:deep_lukewarm_ocean",
        "minecraft:deep_ocean",
    ] {
        let selection = structures
            .structure_at_chunk(
                chunk.x,
                chunk.z,
                12_345,
                StructureDimension::Overworld,
                63,
                &StaticBiomes(biome),
            )
            .expect("monument selection")
            .expect("deep ocean monument placement chunk");
        assert_eq!(selection.structure, "minecraft:monument");
        assert_eq!(selection.set, "minecraft:ocean_monuments");
        assert_eq!(selection.chunk, chunk);
    }
    for biome in [
        "minecraft:ocean",
        "minecraft:cold_ocean",
        "minecraft:frozen_ocean",
        "minecraft:lukewarm_ocean",
        "minecraft:warm_ocean",
        "minecraft:river",
        "minecraft:plains",
    ] {
        assert!(
            structures
                .structure_at_chunk(
                    chunk.x,
                    chunk.z,
                    12_345,
                    StructureDimension::Overworld,
                    63,
                    &StaticBiomes(biome),
                )
                .expect("shallow biome monument selection")
                .is_none(),
            "monuments must reject {biome}"
        );
    }
    assert!(
        structures
            .structure_at_chunk(
                chunk.x,
                chunk.z,
                12_345,
                StructureDimension::Nether,
                63,
                &StaticBiomes("minecraft:deep_ocean"),
            )
            .expect("Nether monument selection")
            .is_none()
    );
}

#[test]
fn ocean_monument_fixed_seed_pins_bounds_height_support_and_core_blocks() {
    let seed = 12_345;
    let chunk = ChunkPos::new(42, -58);
    let structures = StructureGenerator::shipped(HashMap::new())
        .expect("monument generator")
        .with_enabled_structures(["minecraft:monument"]);
    let ocean = StaticBiomes("minecraft:deep_ocean");
    let definition = structures
        .definitions()
        .get("minecraft:monument")
        .expect("monument definition");
    let mut height_source = MockLevel::new(-64, 384);
    height_source.fixed_height = Some(40);
    let templates = HashMap::new();
    let context =
        StructureGenerationContext::new(seed, chunk, 0, &templates).with_level(&height_source);
    let start = definition
        .generate_start(&context)
        .expect("ocean monument start");
    assert!(start.is_valid());
    assert!(start.pieces().is_empty());
    let monument = start.ocean_monument().expect("ocean monument piece");
    assert_eq!(monument.rotation(), Rotation::None);
    let min_x = chunk.x << 4;
    let min_z = chunk.z << 4;
    let bounds = BoundingBox::new(
        min_x,
        OceanMonumentStart::GENERATION_HEIGHT - OceanMonumentStart::FOUNDATION_DEPTH,
        min_z,
        min_x + OceanMonumentStart::WIDTH - 1,
        OceanMonumentStart::GENERATION_HEIGHT + OceanMonumentStart::HEIGHT - 1,
        min_z + OceanMonumentStart::DEPTH - 1,
    );
    assert_eq!(start.bounding_box(), Some(bounds));
    assert_eq!(monument.bounding_box(), bounds);
    assert_eq!(
        definition
            .generate_start(&context)
            .expect("second monument start"),
        start
    );

    let mut level = MockLevel::new(-64, 384);
    level.fixed_height = Some(100);
    let placement = structures
        .place_for_chunk_with_height_source(
            &mut level,
            &height_source,
            chunk,
            seed,
            StructureDimension::Overworld,
            63,
            &ocean,
            0,
            Decoration::SurfaceStructures,
        )
        .expect("monument structure pass");
    assert_eq!(placement.generated_starts, 1);
    assert_eq!(placement.placed_pieces, 1);
    assert_eq!(placement.skipped_pieces, 0);
    assert_eq!(placement.rejected, 0);
    assert!(placement.written > 0);
    assert!(placement.written_blocks.iter().all(|pos| {
        (min_x..=min_x + 15).contains(&pos.x())
            && (min_z..=min_z + 15).contains(&pos.z())
            && (-64..=319).contains(&pos.y())
    }));
    assert!(placement.written_blocks.windows(2).all(|pair| {
        (pair[0].x(), pair[0].y(), pair[0].z()) < (pair[1].x(), pair[1].y(), pair[1].z())
    }));

    let mut level = MockLevel::new(-64, 384);
    level.fixed_height = Some(100);
    let full_box = BoundingBox::new(
        bounds.min_x(),
        -64,
        bounds.min_z(),
        bounds.max_x(),
        319,
        bounds.max_z(),
    );
    let full = monument
        .place_with_height_source(&mut level, &height_source, full_box)
        .expect("full monument core placement");
    assert_eq!(full.placed_pieces, 1);
    assert_eq!(full.rejected, 0);

    let base_y = 39;
    assert_eq!(
        level
            .get_block(BlockPos::new(min_x + 17, base_y, min_z + 12))
            .expect("elder guardian gold block")
            .name,
        "minecraft:gold_block"
    );
    for (x, z) in [(13, 12), (21, 12), (17, 8)] {
        assert_eq!(
            level
                .get_block(BlockPos::new(min_x + x, base_y + 3, min_z + z))
                .expect("elder guardian sea lantern")
                .name,
            "minecraft:sea_lantern"
        );
    }
    assert!(matches!(
        level
            .get_block(BlockPos::new(min_x + 2, base_y, min_z + 10))
            .expect("left sponge room floor")
            .name
            .as_str(),
        "minecraft:sponge" | "minecraft:wet_sponge"
    ));
    for depth in 1..=OceanMonumentStart::FOUNDATION_DEPTH {
        assert!(
            level
                .get_block(BlockPos::new(min_x + 2, base_y - depth, min_z + 10))
                .is_some_and(|block| block.name != "minecraft:air"),
            "foundation support at depth {depth}"
        );
    }
    assert!(
        !level
            .blocks
            .contains_key(&BlockPos::new(min_x + 2, base_y - 6, min_z + 10))
    );
    assert!(
        !level
            .blocks
            .contains_key(&BlockPos::new(min_x + 17, 100, min_z + 12)),
        "placement must use the supplied ocean-floor height source"
    );

    let missing = MockLevel::new(-64, 384);
    let mut skipped_level = MockLevel::new(-64, 384);
    let skipped = start
        .place_in_chunk_with_height_source(&mut skipped_level, &missing, chunk)
        .expect("missing ocean-floor heights skip placement");
    assert_eq!(skipped.placed_pieces, 0);
    assert_eq!(skipped.written, 0);
}

#[test]
fn ocean_monument_rotation_clipping_and_geometry_are_deterministic() {
    let source = MockLevel {
        fixed_height: Some(40),
        ..MockLevel::new(-64, 384)
    };
    for rotation in Rotation::ALL {
        let piece = OceanMonumentPiece::new(12_346, 8, 8, rotation);
        let bounds = piece.bounding_box();
        let (width, depth) = if rotation.swaps_horizontal_axes() {
            (OceanMonumentStart::DEPTH, OceanMonumentStart::WIDTH)
        } else {
            (OceanMonumentStart::WIDTH, OceanMonumentStart::DEPTH)
        };
        assert_eq!(bounds.x_span(), width);
        assert_eq!(bounds.z_span(), depth);
        assert_eq!(
            bounds.y_span(),
            OceanMonumentStart::HEIGHT + OceanMonumentStart::FOUNDATION_DEPTH
        );
        let full_box = BoundingBox::new(
            bounds.min_x(),
            -64,
            bounds.min_z(),
            bounds.max_x(),
            319,
            bounds.max_z(),
        );
        let mut full_level = MockLevel::new(-64, 384);
        let full = piece
            .place_with_height_source(&mut full_level, &source, full_box)
            .expect("full ocean monument placement");
        assert_eq!(full.placed_pieces, 1);
        assert_eq!(full.rejected, 0);
        assert!(full.written > 0);

        let base_y = 39;
        let rotated_local = |x, z| match rotation {
            Rotation::None => (x, z),
            Rotation::Clockwise90 => (OceanMonumentStart::DEPTH - 1 - z, x),
            Rotation::Clockwise180 => (
                OceanMonumentStart::WIDTH - 1 - x,
                OceanMonumentStart::DEPTH - 1 - z,
            ),
            Rotation::CounterClockwise90 => (z, OceanMonumentStart::WIDTH - 1 - x),
        };
        let gold_blocks = full_level
            .blocks
            .values()
            .filter(|block| block.name == "minecraft:gold_block")
            .count();
        assert_eq!(gold_blocks, 1);
        let (core_x, core_z) = rotated_local(17, 12);
        assert_eq!(
            full_level
                .blocks
                .get(&BlockPos::new(8 + core_x, base_y, 8 + core_z))
                .expect("guardian room gold")
                .name,
            "minecraft:gold_block"
        );
        let sea_lanterns = full_level
            .blocks
            .values()
            .filter(|block| block.name == "minecraft:sea_lantern")
            .count();
        assert_eq!(sea_lanterns, 4);
        assert!(
            full_level
                .blocks
                .values()
                .any(|block| block.name == "minecraft:sponge")
        );
        assert!(
            full_level
                .blocks
                .values()
                .any(|block| block.name == "minecraft:wet_sponge")
        );
        for name in [
            "minecraft:prismarine",
            "minecraft:prismarine_bricks",
            "minecraft:dark_prismarine",
            "minecraft:prismarine_brick_stairs",
        ] {
            assert!(
                full_level.blocks.values().any(|block| block.name == name),
                "monument missing {name}"
            );
        }
        for (x, z, expected) in [
            (2, 10, "minecraft:sponge"),
            (29, 10, "minecraft:wet_sponge"),
        ] {
            let (room_x, room_z) = rotated_local(x, z);
            assert_eq!(
                full_level
                    .blocks
                    .get(&BlockPos::new(8 + room_x, base_y, 8 + room_z))
                    .expect("sponge room floor")
                    .name,
                expected
            );
        }
        let (wing_x, wing_z) = rotated_local(3, 12);
        assert!(matches!(
            full_level
                .blocks
                .get(&BlockPos::new(8 + wing_x, base_y, 8 + wing_z))
                .expect("left wing floor")
                .name
                .as_str(),
            "minecraft:sponge" | "minecraft:wet_sponge"
        ));
        let (wall_x, wall_z) = rotated_local(8, 7);
        assert!(
            full_level
                .blocks
                .contains_key(&BlockPos::new(8 + wall_x, base_y + 2, 8 + wall_z))
        );
        let (entrance_wall_x, entrance_wall_z) = rotated_local(14, 25);
        assert!(full_level.blocks.contains_key(&BlockPos::new(
            8 + entrance_wall_x,
            base_y + 2,
            8 + entrance_wall_z
        )));
        let (corner_x, corner_z) = rotated_local(0, 0);
        assert!(!full_level.blocks.contains_key(&BlockPos::new(
            8 + corner_x,
            base_y,
            8 + corner_z
        )));

        let (entrance_x, entrance_z) = rotated_local(17, 27);
        assert_eq!(
            full_level
                .blocks
                .get(&BlockPos::new(8 + entrance_x, base_y + 1, 8 + entrance_z))
                .expect("entrance doorway")
                .name,
            "minecraft:air"
        );
        assert_eq!(
            full_level
                .blocks
                .get(&BlockPos::new(8 + entrance_x, base_y + 2, 8 + entrance_z))
                .expect("entrance doorway")
                .name,
            "minecraft:air"
        );
        assert!(full_level.blocks.contains_key(&BlockPos::new(
            8 + entrance_x,
            base_y + 4,
            8 + entrance_z
        )));
        assert!(full_level.blocks.contains_key(&BlockPos::new(
            8 + core_x,
            base_y + 21,
            8 + core_z
        )));
        for (x, z) in [(8, 3), (26, 3), (8, 21), (26, 21)] {
            let (finial_x, finial_z) = rotated_local(x, z);
            assert!(full_level.blocks.contains_key(&BlockPos::new(
                8 + finial_x,
                base_y + 25,
                8 + finial_z
            )));
        }

        let mut split_level = MockLevel::new(-64, 384);
        let mut split_written = 0;
        for chunk_x in [0, 16, 32] {
            for chunk_z in [0, 16, 32] {
                let chunk_box =
                    BoundingBox::new(chunk_x, -64, chunk_z, chunk_x + 15, 319, chunk_z + 15);
                let split = piece
                    .place_with_height_source(&mut split_level, &source, chunk_box)
                    .expect("chunk-clipped monument placement");
                assert_eq!(split.placed_pieces, 1);
                assert_eq!(split.rejected, 0);
                assert!(
                    split
                        .written_blocks
                        .iter()
                        .all(|pos| chunk_box.is_inside(*pos))
                );
                assert!(split.written_blocks.windows(2).all(|pair| {
                    (pair[0].x(), pair[0].y(), pair[0].z())
                        < (pair[1].x(), pair[1].y(), pair[1].z())
                }));
                split_written += split.written;
            }
        }
        assert_eq!(split_written, full.written);
        assert_eq!(split_level.blocks, full_level.blocks);

        let mut repeated_level = MockLevel::new(-64, 384);
        piece
            .place_with_height_source(&mut repeated_level, &source, full_box)
            .expect("repeated monument placement");
        assert_eq!(repeated_level.blocks, full_level.blocks);
    }
}

#[test]
fn shipwreck_definitions_parse_both_biome_variants() {
    for (name, json, beached, biomes) in [
        (
            "minecraft:shipwreck",
            SHIPWRECK_JSON,
            false,
            "#minecraft:has_structure/shipwreck",
        ),
        (
            "minecraft:shipwreck_beached",
            SHIPWRECK_BEACHED_JSON,
            true,
            "#minecraft:has_structure/shipwreck_beached",
        ),
    ] {
        let definition = StructureDefinition::from_json(name, json).expect("shipwreck JSON parses");
        assert_eq!(definition.kind, StructureKind::Shipwreck { beached });
        assert_eq!(definition.step(), Decoration::SurfaceStructures);
        assert_eq!(definition.biomes, BiomeSet::Tag(biomes.to_owned()));
    }

    let catalog = StructureTemplateCatalog::shipped();
    let definition =
        StructureDefinition::from_json("minecraft:shipwreck_beached", SHIPWRECK_BEACHED_JSON)
            .expect("beached definition parses");
    let context = StructureGenerationContext::new(12_345, ChunkPos::new(0, 0), 0, catalog);
    let start = definition
        .generate_start(&context)
        .expect("beached shipwreck start");
    assert!(start.is_valid());
    assert!(start.shipwreck().expect("shipwreck piece").is_beached());
}

#[test]
fn shipwreck_fixed_seed_selects_bounds_samples_and_chest_markers() {
    let seed = 12_345;
    let chunk = ChunkPos::new(75, -44);
    let catalog = StructureTemplateCatalog::shipped();
    let structures = StructureGenerator::from_catalog(catalog.clone())
        .expect("shipwreck generator")
        .with_enabled_structures(["minecraft:shipwreck"]);
    let ocean = StaticBiomes("minecraft:ocean");
    let desert = StaticBiomes("minecraft:desert");
    let selection = structures
        .structure_at_chunk(
            chunk.x,
            chunk.z,
            seed,
            StructureDimension::Overworld,
            63,
            &ocean,
        )
        .expect("shipwreck selection")
        .expect("ocean shipwreck placement chunk");
    assert_eq!(selection.structure, "minecraft:shipwreck");
    assert_eq!(selection.set, "minecraft:shipwrecks");
    assert_eq!(selection.chunk, chunk);
    assert!(
        structures
            .structure_at_chunk(
                chunk.x,
                chunk.z,
                seed,
                StructureDimension::Overworld,
                63,
                &desert,
            )
            .expect("desert selection")
            .is_none()
    );

    let definition = structures
        .definitions()
        .get("minecraft:shipwreck")
        .expect("shipwreck definition");
    let mut height_source = MockLevel::new(-64, 384);
    height_source.fixed_height = Some(40);
    let context =
        StructureGenerationContext::new(seed, chunk, 0, catalog).with_level(&height_source);
    let start = definition
        .generate_start(&context)
        .expect("shipwreck start");
    assert!(start.is_valid());
    assert_eq!(start.pieces().len(), 0);
    let shipwreck = start.shipwreck().expect("shipwreck payload");
    assert!(!shipwreck.is_beached());
    let template = shipwreck.template();
    assert_eq!(template.template_name, ShipwreckStart::TEMPLATE);
    assert_eq!(template.place_settings.rotation(), Rotation::None);
    assert_eq!(
        template.place_settings.rotation_pivot(),
        BlockPos::new(4, 0, 15)
    );
    assert_eq!(
        start.bounding_box(),
        Some(BoundingBox::new(1200, 90, -704, 1208, 98, -677))
    );
    assert_eq!(shipwreck.bounding_box(), start.bounding_box().unwrap());
    assert_eq!(
        definition
            .generate_start(&context)
            .expect("second shipwreck start"),
        start
    );

    let mut level = MockLevel::new(-64, 384);
    level.fixed_height = Some(100);
    let first = start
        .place_in_chunk_with_height_source(
            &mut level,
            &height_source,
            ChunkPos::new(chunk.x, chunk.z),
        )
        .expect("first shipwreck chunk");
    assert_eq!(first.placed_pieces, 1);
    assert_eq!(first.skipped_pieces, 0);
    assert_eq!(first.rejected, 0);
    assert_eq!(first.written, first.written_blocks.len());
    assert!(first.written > 0);
    assert!(first.written_blocks.iter().all(|pos| {
        (1200..=1215).contains(&pos.x())
            && (-704..=-689).contains(&pos.z())
            && (-64..=319).contains(&pos.y())
    }));
    assert_eq!(
        first
            .data_markers
            .iter()
            .map(|marker| marker.name.as_str())
            .collect::<Vec<_>>(),
        vec!["supply_chest"]
    );
    assert_eq!(
        level
            .get_block(BlockPos::new(1203, 41, -695))
            .expect("sampled oak planks")
            .name,
        "minecraft:oak_planks"
    );
    assert!(
        level
            .blocks
            .values()
            .any(|block| block.name == "minecraft:spruce_stairs")
    );
    assert!(
        !level
            .blocks
            .values()
            .any(|block| block.name == "minecraft:structure_block")
    );
    let chest = first
        .block_entities
        .iter()
        .find(|block| block.state.name == "minecraft:chest")
        .expect("supply chest");
    assert_eq!(chest.pos, BlockPos::new(1204, 42, -696));
    assert!(chest.nbt.as_ref().is_some_and(|nbt| {
        nbt.get("id").and_then(Tag::as_str) == Some("minecraft:chest")
            && nbt.get("LootTable").and_then(Tag::as_str)
                == Some("minecraft:chests/shipwreck_supply")
            && nbt.get("LootTableSeed").and_then(Tag::as_long).is_some()
    }));

    let second = start
        .place_in_chunk_with_height_source(
            &mut level,
            &height_source,
            ChunkPos::new(chunk.x, chunk.z + 1),
        )
        .expect("second shipwreck chunk");
    assert_eq!(second.placed_pieces, 1);
    assert!(second.written > 0);
    let mut marker_names: Vec<_> = second
        .data_markers
        .iter()
        .map(|marker| marker.name.as_str())
        .collect();
    marker_names.sort_unstable();
    assert_eq!(marker_names, vec!["map_chest", "treasure_chest"]);
    let mut chests: Vec<_> = second
        .block_entities
        .iter()
        .filter(|block| block.state.name == "minecraft:chest")
        .map(|block| block.pos)
        .collect();
    chests.sort_by_key(|pos| (pos.x(), pos.y(), pos.z()));
    assert_eq!(
        chests,
        vec![BlockPos::new(1205, 42, -686), BlockPos::new(1206, 44, -680),]
    );
    assert!(second.written_blocks.iter().all(|pos| {
        (1200..=1215).contains(&pos.x())
            && (-688..=-673).contains(&pos.z())
            && (-64..=319).contains(&pos.y())
    }));
}

#[test]
fn shipwreck_structure_pass_uses_ocean_floor_height_source() {
    let seed = 12_345;
    let chunk = ChunkPos::new(75, -44);
    let structures = StructureGenerator::from_catalog(StructureTemplateCatalog::shipped().clone())
        .expect("shipwreck generator")
        .with_enabled_structures(["minecraft:shipwreck"]);
    let ocean = StaticBiomes("minecraft:ocean");
    let mut height_source = MockLevel::new(-64, 384);
    height_source.fixed_height = Some(40);
    let mut level = MockLevel::new(-64, 384);
    level.fixed_height = Some(100);
    let placement = structures
        .place_for_chunk_with_height_source(
            &mut level,
            &height_source,
            chunk,
            seed,
            StructureDimension::Overworld,
            63,
            &ocean,
            0,
            Decoration::SurfaceStructures,
        )
        .expect("shipwreck structure pass");
    assert_eq!(placement.generated_starts, 1);
    assert_eq!(placement.placed_pieces, 1);
    assert_eq!(placement.skipped_pieces, 0);
    assert_eq!(placement.rejected, 0);
    assert_eq!(placement.written, placement.written_blocks.len());
    assert_eq!(placement.data_markers.len(), 1);
    assert_eq!(placement.data_markers[0].name, "supply_chest");
    assert_eq!(
        level
            .get_block(BlockPos::new(1204, 42, -696))
            .expect("chest at ocean-floor height")
            .name,
        "minecraft:chest"
    );
    assert!(!level.blocks.contains_key(&BlockPos::new(1204, 102, -696)));

    let missing = MockLevel::new(-64, 384);
    let mut skipped_level = MockLevel::new(-64, 384);
    let skipped = start_for_shipwreck(seed, chunk, &missing)
        .place_in_chunk_with_height_source(&mut skipped_level, &missing, chunk)
        .expect("missing height source skips placement");
    assert_eq!(skipped.placed_pieces, 0);
    assert_eq!(skipped.written, 0);
}

fn start_for_shipwreck(
    seed: i64,
    chunk: ChunkPos,
    level: &dyn FeatureLevel,
) -> folia_core::worldgen::structure::StructureStart {
    let definition = StructureDefinition::from_json("minecraft:shipwreck", SHIPWRECK_JSON)
        .expect("shipwreck definition");
    let catalog = StructureTemplateCatalog::shipped();
    let context = StructureGenerationContext::new(seed, chunk, 0, catalog).with_level(level);
    definition
        .generate_start(&context)
        .expect("shipwreck start")
}

#[test]
fn ruined_portal_definitions_cover_all_shipped_variants() {
    let catalog = StructureTemplateCatalog::shipped();
    let structures =
        StructureGenerator::from_catalog(catalog.clone()).expect("ruined portal generator");
    for (name, variant, biomes, heightmap, placements) in [
        (
            "minecraft:ruined_portal",
            RuinedPortalVariant::Standard,
            "#minecraft:has_structure/ruined_portal_standard",
            Heightmap::WorldSurfaceWg,
            vec![
                RuinedPortalPlacement::Underground,
                RuinedPortalPlacement::OnLandSurface,
            ],
        ),
        (
            "minecraft:ruined_portal_desert",
            RuinedPortalVariant::Desert,
            "#minecraft:has_structure/ruined_portal_desert",
            Heightmap::WorldSurfaceWg,
            vec![RuinedPortalPlacement::PartlyBuried],
        ),
        (
            "minecraft:ruined_portal_jungle",
            RuinedPortalVariant::Jungle,
            "#minecraft:has_structure/ruined_portal_jungle",
            Heightmap::WorldSurfaceWg,
            vec![RuinedPortalPlacement::OnLandSurface],
        ),
        (
            "minecraft:ruined_portal_swamp",
            RuinedPortalVariant::Swamp,
            "#minecraft:has_structure/ruined_portal_swamp",
            Heightmap::OceanFloorWg,
            vec![RuinedPortalPlacement::OnOceanFloor],
        ),
        (
            "minecraft:ruined_portal_mountain",
            RuinedPortalVariant::Mountain,
            "#minecraft:has_structure/ruined_portal_mountain",
            Heightmap::WorldSurfaceWg,
            vec![
                RuinedPortalPlacement::InMountain,
                RuinedPortalPlacement::OnLandSurface,
            ],
        ),
        (
            "minecraft:ruined_portal_ocean",
            RuinedPortalVariant::Ocean,
            "#minecraft:has_structure/ruined_portal_ocean",
            Heightmap::OceanFloorWg,
            vec![RuinedPortalPlacement::OnOceanFloor],
        ),
        (
            "minecraft:ruined_portal_nether",
            RuinedPortalVariant::Nether,
            "#minecraft:has_structure/ruined_portal_nether",
            Heightmap::WorldSurfaceWg,
            vec![RuinedPortalPlacement::InNether],
        ),
    ] {
        let definition = structures
            .definitions()
            .get(name)
            .expect("shipped ruined portal definition");
        assert_eq!(
            definition.kind,
            StructureKind::RuinedPortal { variant },
            "definition {name}"
        );
        assert_eq!(definition.biomes, BiomeSet::Tag(biomes.to_owned()));
        assert_eq!(definition.step(), Decoration::SurfaceStructures);
        assert_eq!(variant.heightmap(), heightmap);
        assert_eq!(
            variant
                .setups()
                .iter()
                .map(|setup| setup.placement)
                .collect::<Vec<_>>(),
            placements
        );
    }
}

#[test]
fn ruined_portal_set_pipeline_selects_each_biome_and_rejects_other_dimensions() {
    let seed = 12_345;
    let structures = StructureGenerator::from_catalog(StructureTemplateCatalog::shipped().clone())
        .expect("ruined portal generator");
    let set = structures.sets().get("ruined_portals").expect("portal set");
    let StructurePlacement::RandomSpread(spread) = &set.placement else {
        panic!("ruined portals must use random spread");
    };
    assert_eq!(spread.spacing, 40);
    assert_eq!(spread.separation, 15);
    assert_eq!(spread.salt, 34_222_645);
    let find_chunk = |target: &str| {
        for x in -128..=128 {
            for z in -128..=128 {
                if spread.is_placement_chunk(seed, x, z)
                    && set
                        .select_structure(seed, x, z)
                        .expect("weighted portal selection")
                        .structure
                        == target
                {
                    return ChunkPos::new(x, z);
                }
            }
        }
        panic!("no placement chunk selecting {target}");
    };

    for (biome, dimension, expected) in [
        (
            "minecraft:plains",
            StructureDimension::Overworld,
            "minecraft:ruined_portal",
        ),
        (
            "minecraft:desert",
            StructureDimension::Overworld,
            "minecraft:ruined_portal_desert",
        ),
        (
            "minecraft:jungle",
            StructureDimension::Overworld,
            "minecraft:ruined_portal_jungle",
        ),
        (
            "minecraft:swamp",
            StructureDimension::Overworld,
            "minecraft:ruined_portal_swamp",
        ),
        (
            "minecraft:windswept_hills",
            StructureDimension::Overworld,
            "minecraft:ruined_portal_mountain",
        ),
        (
            "minecraft:ocean",
            StructureDimension::Overworld,
            "minecraft:ruined_portal_ocean",
        ),
        (
            "minecraft:nether_wastes",
            StructureDimension::Nether,
            "minecraft:ruined_portal_nether",
        ),
    ] {
        let chunk = find_chunk(expected);
        let selection = structures
            .structure_at_chunk(chunk.x, chunk.z, seed, dimension, 63, &StaticBiomes(biome))
            .expect("portal selection")
            .unwrap_or_else(|| panic!("matching portal biome {biome} in {dimension:?}"));
        assert_eq!(selection.structure, expected, "biome {biome}");
        assert_eq!(selection.set, "minecraft:ruined_portals");
        assert_eq!(selection.chunk, chunk);
    }

    let standard_chunk = find_chunk("minecraft:ruined_portal");
    assert_eq!(standard_chunk, ChunkPos::new(-96, 62));
    for (biome, dimension) in [
        ("minecraft:deep_dark", StructureDimension::Overworld),
        ("minecraft:desert", StructureDimension::Overworld),
        ("minecraft:nether_wastes", StructureDimension::Overworld),
        ("minecraft:plains", StructureDimension::Nether),
    ] {
        assert!(
            structures
                .structure_at_chunk(
                    standard_chunk.x,
                    standard_chunk.z,
                    seed,
                    dimension,
                    63,
                    &StaticBiomes(biome),
                )
                .expect("portal rejection")
                .is_none(),
            "unexpected portal in {biome} ({dimension:?})"
        );
    }
}

#[test]
fn ruined_portal_standard_fixed_seed_selects_template_bounds_and_portal_blocks() {
    let seed = 12_345;
    let chunk = ChunkPos::new(0, 0);
    let catalog = StructureTemplateCatalog::shipped();
    let structures = StructureGenerator::from_catalog(catalog.clone())
        .expect("ruined portal generator")
        .with_enabled_structures(["minecraft:ruined_portal"]);
    let definition = structures
        .definitions()
        .get("minecraft:ruined_portal")
        .expect("standard definition");
    let mut height_source = MockLevel::new(-64, 384);
    height_source.fixed_height = Some(64);
    let context =
        StructureGenerationContext::new(seed, chunk, 0, catalog).with_level(&height_source);
    let start = definition
        .generate_start(&context)
        .expect("standard ruined portal start");
    assert!(start.is_valid());
    assert!(start.pieces().is_empty());
    let portal = start.ruined_portal().expect("ruined portal payload");
    let template = portal.template();
    assert_eq!(template.template_name, "minecraft:ruined_portal/portal_2");
    assert_eq!(
        template.place_settings.rotation(),
        Rotation::CounterClockwise90
    );
    assert_eq!(template.place_settings.mirror(), Mirror::FrontBack);
    assert_eq!(template.template_position, BlockPos::new(0, 25, 0));
    assert_eq!(portal.bounding_box(), BoundingBox::new(0, 25, 8, 8, 36, 16));
    assert_eq!(portal.placement(), RuinedPortalPlacement::Underground);
    assert!(portal.air_pocket());
    assert!(matches!(
        RuinedPortalVariant::from_structure_id("minecraft:ruined_portal"),
        Some(RuinedPortalVariant::Standard)
    ));
    assert!(matches!(
        template.template_name.as_str(),
        "minecraft:ruined_portal/portal_1"
            | "minecraft:ruined_portal/portal_2"
            | "minecraft:ruined_portal/portal_3"
            | "minecraft:ruined_portal/portal_4"
            | "minecraft:ruined_portal/portal_5"
            | "minecraft:ruined_portal/portal_6"
            | "minecraft:ruined_portal/portal_7"
            | "minecraft:ruined_portal/portal_8"
            | "minecraft:ruined_portal/portal_9"
            | "minecraft:ruined_portal/portal_10"
            | "minecraft:ruined_portal/giant_portal_1"
            | "minecraft:ruined_portal/giant_portal_2"
            | "minecraft:ruined_portal/giant_portal_3"
    ));
    assert_eq!(
        template.place_settings.rotation_pivot(),
        BlockPos::new(
            template.template.size_x() / 2,
            0,
            template.template.size_z() / 2
        )
    );
    assert_eq!(portal.bounding_box(), start.bounding_box().unwrap());
    assert_eq!(
        definition
            .generate_start(&context)
            .expect("second ruined portal start"),
        start
    );

    let center = portal.bounding_box().center();
    let center_chunk = ChunkPos::new(center.x() >> 4, center.z() >> 4);
    let mut placed = 0;
    let mut obsidian = Vec::new();
    let mut lava = Vec::new();
    for dx in -2..=2 {
        for dz in -2..=2 {
            let candidate = ChunkPos::new(chunk.x + dx, chunk.z + dz);
            let mut level = MockLevel::new(-64, 384);
            let placement = start
                .place_in_chunk_with_height_source(&mut level, &height_source, candidate)
                .expect("ruined portal chunk placement");
            assert_eq!(
                usize::from(candidate == center_chunk),
                placement.placed_pieces
            );
            assert_eq!(
                placement.skipped_pieces,
                usize::from(candidate != center_chunk)
            );
            placed += placement.placed_pieces;
            assert!(placement.written_blocks.iter().all(|pos| {
                (candidate.x << 4) <= pos.x()
                    && pos.x() < (candidate.x << 4) + 16
                    && (candidate.z << 4) <= pos.z()
                    && pos.z() < (candidate.z << 4) + 16
                    && (-64..320).contains(&pos.y())
            }));
            for (pos, block) in &level.blocks {
                match block.name.as_str() {
                    "minecraft:obsidian" => obsidian.push(*pos),
                    "minecraft:lava" => lava.push(*pos),
                    "minecraft:structure_block" => panic!("structure block leaked into the world"),
                    _ => {}
                }
            }
        }
    }
    obsidian.sort_by_key(|pos| (pos.x(), pos.y(), pos.z()));
    lava.sort_by_key(|pos| (pos.x(), pos.y(), pos.z()));
    assert_eq!(placed, 1);
    assert_eq!(
        obsidian,
        vec![
            BlockPos::new(2, 29, 13),
            BlockPos::new(2, 30, 13),
            BlockPos::new(2, 31, 13),
            BlockPos::new(2, 32, 13),
            BlockPos::new(2, 33, 13),
            BlockPos::new(3, 33, 13),
            BlockPos::new(4, 33, 13),
            BlockPos::new(5, 32, 13),
            BlockPos::new(5, 33, 13),
            BlockPos::new(6, 26, 11),
            BlockPos::new(6, 27, 11),
        ]
    );
    assert!(lava.contains(&BlockPos::new(1, 26, 14)));
    assert!(lava.contains(&BlockPos::new(7, 26, 10)));
}

#[test]
fn ruined_portal_ocean_uses_floor_height_and_missing_inputs_fail_explicitly() {
    let seed = 12_345;
    let chunk = ChunkPos::new(0, 0);
    let catalog = StructureTemplateCatalog::shipped();
    let structures =
        StructureGenerator::from_catalog(catalog.clone()).expect("ruined portal generator");
    let definition = structures
        .definitions()
        .get("minecraft:ruined_portal_ocean")
        .expect("ocean definition");
    let mut height_source = MockLevel::new(-64, 384);
    height_source.fixed_height = Some(90);
    height_source.ocean_floor_height = Some(40);
    let context =
        StructureGenerationContext::new(seed, chunk, 0, catalog).with_level(&height_source);
    let start = definition
        .generate_start(&context)
        .expect("ocean ruined portal start");
    let portal = start.ruined_portal().expect("ocean ruined portal piece");
    assert_eq!(portal.variant(), RuinedPortalVariant::Ocean);
    assert_eq!(portal.placement(), RuinedPortalPlacement::OnOceanFloor);
    assert_eq!(portal.bounding_box().min_y(), 39);

    let mut level = MockLevel::new(-64, 384);
    let placement = start
        .place_in_chunk(&mut level, chunk)
        .expect("ocean ruined portal placement");
    assert!(placement.written > 0);
    assert!(
        placement
            .written_blocks
            .iter()
            .all(|pos| (0..16).contains(&pos.x()) && (0..16).contains(&pos.z()) && pos.y() < 60)
    );

    let missing_height = MockLevel::new(-64, 384);
    let error = definition
        .generate_start(
            &StructureGenerationContext::new(seed, chunk, 0, StructureTemplateCatalog::shipped())
                .with_level(&missing_height),
        )
        .expect_err("missing heights must fail explicitly");
    assert!(matches!(error, StructureError::MissingHeightSource { .. }));

    let no_templates = HashMap::new();
    let error = definition
        .generate_start(
            &StructureGenerationContext::new(seed, chunk, 0, &no_templates)
                .with_level(&height_source),
        )
        .expect_err("missing templates must fail explicitly");
    assert!(matches!(
        error,
        StructureError::UnknownTemplate { ref template, .. }
            if template.starts_with("minecraft:ruined_portal/")
    ));
}

#[test]
fn ruined_portal_all_variants_generate_valid_template_backed_starts() {
    let seed = 12_345;
    let chunk = ChunkPos::new(3, -5);
    let catalog = StructureTemplateCatalog::shipped();
    let structures =
        StructureGenerator::from_catalog(catalog.clone()).expect("ruined portal generator");
    let mut height_source = MockLevel::new(-64, 384);
    height_source.fixed_height = Some(64);
    for (name, variant) in [
        ("minecraft:ruined_portal", RuinedPortalVariant::Standard),
        (
            "minecraft:ruined_portal_desert",
            RuinedPortalVariant::Desert,
        ),
        (
            "minecraft:ruined_portal_jungle",
            RuinedPortalVariant::Jungle,
        ),
        ("minecraft:ruined_portal_swamp", RuinedPortalVariant::Swamp),
        (
            "minecraft:ruined_portal_mountain",
            RuinedPortalVariant::Mountain,
        ),
        ("minecraft:ruined_portal_ocean", RuinedPortalVariant::Ocean),
        (
            "minecraft:ruined_portal_nether",
            RuinedPortalVariant::Nether,
        ),
    ] {
        let definition = structures
            .definitions()
            .get(name)
            .expect("ruined portal definition");
        let context =
            StructureGenerationContext::new(seed, chunk, 0, catalog).with_level(&height_source);
        let start = definition
            .generate_start(&context)
            .expect("ruined portal start");
        assert!(start.is_valid(), "variant {variant:?}");
        let portal = start.ruined_portal().expect("ruined portal piece");
        assert_eq!(portal.variant(), variant);
        assert!(
            portal
                .template()
                .template_name
                .starts_with("minecraft:ruined_portal/")
        );
        let center = portal.bounding_box().center();
        let center_chunk = ChunkPos::new(center.x() >> 4, center.z() >> 4);
        let mut level = MockLevel::new(-64, 384);
        let placement = start
            .place_in_chunk(&mut level, center_chunk)
            .expect("ruined portal placement");
        assert_eq!(placement.placed_pieces, 1, "variant {variant:?}");
        assert!(placement.written > 0, "variant {variant:?}");
    }
}

#[test]
fn ruined_portal_common_and_giant_templates_are_selected_deterministically() {
    let seed = 12_345;
    let catalog = StructureTemplateCatalog::shipped();
    let structures = StructureGenerator::from_catalog(catalog.clone())
        .expect("ruined portal generator")
        .with_enabled_structures(["minecraft:ruined_portal"]);
    let definition = structures
        .definitions()
        .get("minecraft:ruined_portal")
        .expect("standard definition");
    let mut height_source = MockLevel::new(-64, 384);
    height_source.fixed_height = Some(64);
    let mut names = std::collections::BTreeSet::new();
    for x in -8..=8 {
        for z in -8..=8 {
            let context = StructureGenerationContext::new(seed, ChunkPos::new(x, z), 0, catalog)
                .with_level(&height_source);
            let start = definition.generate_start(&context).expect("portal start");
            names.insert(
                start
                    .ruined_portal()
                    .expect("portal piece")
                    .template()
                    .template_name
                    .clone(),
            );
        }
    }
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("minecraft:ruined_portal/portal_"))
    );
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("minecraft:ruined_portal/giant_portal_"))
    );
    assert_eq!(RuinedPortalStart::COMMON_TEMPLATES.len(), 10);
    assert_eq!(RuinedPortalStart::GIANT_TEMPLATES.len(), 3);
}

// Keep the TemplateProvider trait in scope for the HashMap implementation even
// though tests only pass it as &dyn TemplateProvider.
#[allow(dead_code)]
fn assert_template_provider<T: TemplateProvider>() {}
