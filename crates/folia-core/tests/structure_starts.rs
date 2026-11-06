use folia_core::chunk::BlockState;
use folia_core::math::BlockPos;
use folia_core::region::ChunkPos;
use folia_core::worldgen::feature::{FeatureLevel, Heightmap};
use folia_core::worldgen::structure::StructureError;
use folia_core::worldgen::structure::piece::{BoundingBox, StructurePiece, TemplateStructurePiece};
use folia_core::worldgen::structure::set::{
    PlacementError, RandomSpreadType, StructurePlacement, StructureSetError, StructureSetRegistry,
};
use folia_core::worldgen::structure::start::{
    IglooStructure, StructureDefinition, StructureGenerationContext, TemplateProvider,
};
use folia_core::worldgen::structure::template::{
    Mirror, Rotation, StructurePlaceSettings, StructureTemplate,
};
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

const VILLAGE_PLAINS_JSON: &str = r##"{ "type": "minecraft:jigsaw", "biomes": "#minecraft:has_structure/village_plains", "max_distance_from_center": 80, "project_start_to_heightmap": "WORLD_SURFACE_WG", "size": 6, "spawn_overrides": {}, "start_height": { "absolute": 0 }, "start_pool": "minecraft:village/plains/town_centers", "step": "surface_structures", "terrain_adaptation": "beard_thin", "use_expansion_hack": true }"##;

#[derive(Default)]
struct MockLevel {
    min_y: i32,
    height: i32,
    fixed_height: Option<i32>,
    blocks: HashMap<BlockPos, BlockState>,
}

impl MockLevel {
    fn new(min_y: i32, height: i32) -> Self {
        Self {
            min_y,
            height,
            fixed_height: None,
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

    fn heightmap(&self, _kind: Heightmap, _x: i32, _z: i32) -> Option<i32> {
        self.fixed_height
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

// Keep the TemplateProvider trait in scope for the HashMap implementation even
// though tests only pass it as `&dyn TemplateProvider`.
#[allow(dead_code)]
fn assert_template_provider<T: TemplateProvider>() {}
