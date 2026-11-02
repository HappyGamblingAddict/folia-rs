//! Known-answer tests for the Java/Folia random sources.
//!
//! Expected values were produced by running the decompiled Java algorithms
//! (`LegacyRandomSource`, `BitRandomSource`, `XoroshiroRandomSource`,
//! `Xoroshiro128PlusPlus`, and `RandomSupport`) directly. The first legacy
//! values are also hand-checkable from the state transitions documented
//! below.

use folia_random::{
    GOLDEN_RATIO_64, LegacyRandomSource, RandomSource, SILVER_RATIO_64, Seed128bit,
    Xoroshiro128PlusPlus, XoroshiroRandomSource, mix_stafford13, upgrade_seed_to_128bit,
};

// ---------------------------------------------------------------------------
// Known sequences
// ---------------------------------------------------------------------------

#[test]
fn legacy_seed_zero_matches_java() {
    // Hand check: setSeed(0) starts at (0 ^ 0x5DEECE66D) & (2^48 - 1).
    // First LCG step:
    //   0x5DEECE66D * 0x5DEECE66D + 11 = 0xBB20B4600A74 (mod 2^48)
    // next(32) = 0xBB20B4600A74 >> 16 = 0xBB20B460 = -1155484576 as i32.
    let mut rng = LegacyRandomSource::new(0);
    assert_eq!(
        [
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
        ],
        [
            -1155484576,
            -723955400,
            1033096058,
            -1690734402,
            -1557280266,
            1327362106,
            -1930858313,
            502539523,
        ]
    );

    let mut rng = LegacyRandomSource::new(0);
    assert_eq!(
        [
            rng.next_long(),
            rng.next_long(),
            rng.next_long(),
            rng.next_long(),
            rng.next_long(),
            rng.next_long(),
        ],
        [
            -4962768465676381896,
            4437113781045784766,
            -6688467811848818630,
            -8292973307042192125,
            -7423979211207825555,
            6146794652083548235,
        ]
    );

    let mut rng = LegacyRandomSource::new(0);
    assert_eq!(
        [
            rng.next_double().to_bits(),
            rng.next_double().to_bits(),
            rng.next_double().to_bits(),
            rng.next_double().to_bits(),
        ],
        [
            4604759191809163264,
            4597834258197774336,
            4603916565396586496,
            4603133115580284928,
        ]
    );

    let mut rng = LegacyRandomSource::new(0);
    assert_eq!(
        [
            rng.next_float().to_bits(),
            rng.next_float().to_bits(),
            rng.next_float().to_bits(),
            rng.next_float().to_bits(),
            rng.next_float().to_bits(),
            rng.next_float().to_bits(),
        ],
        [
            1060839604, 1062525265, 1047940908, 1058748784, 1059270089, 1050557408
        ]
    );

    let mut rng = LegacyRandomSource::new(0);
    assert_eq!(
        [
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
        ],
        [
            true, true, false, true, true, false, true, false, true, true, false, false,
        ]
    );
}

#[test]
fn legacy_seed_42_matches_java() {
    // Hand check: setSeed(42) starts at (42 ^ 0x5DEECE66D) = 0x5DEECE647.
    // First LCG step:
    //   0x5DEECE647 * 0x5DEECE66D + 11 = 0xBA419D35D646 (mod 2^48)
    // next(32) = 0xBA419D35D646 >> 16 = 0xBA419D35 = -1170105035 as i32.
    let mut rng = LegacyRandomSource::new(42);
    assert_eq!(
        [
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
        ],
        [
            -1170105035,
            234785527,
            -1360544799,
            205897768,
            1325939940,
            -248792245,
            1190043011,
            -1255373459,
        ]
    );

    let mut rng = LegacyRandomSource::new(42);
    assert_eq!(
        [
            rng.next_long(),
            rng.next_long(),
            rng.next_long(),
            rng.next_long(),
            rng.next_long(),
            rng.next_long(),
        ],
        [
            -5025562857975149833,
            -5843495416241995736,
            5694868678511409995,
            5111195811822994797,
            -6169532649852302182,
            -1782466964123969572,
        ]
    );

    let mut rng = LegacyRandomSource::new(42);
    assert_eq!(
        [
            rng.next_double().to_bits(),
            rng.next_double().to_bits(),
            rng.next_double().to_bits(),
            rng.next_double().to_bits(),
        ],
        [
            4604728530574508032,
            4604329149618716672,
            4599233015229448192,
            4598663022066532352,
        ]
    );

    let mut rng = LegacyRandomSource::new(42);
    assert_eq!(
        [
            rng.next_float().to_bits(),
            rng.next_float().to_bits(),
            rng.next_float().to_bits(),
            rng.next_float().to_bits(),
            rng.next_float().to_bits(),
            rng.next_float().to_bits(),
        ],
        [
            1060782493, 1029695648, 1060038587, 1027890176, 1050546296, 1064381371
        ]
    );

    let mut rng = LegacyRandomSource::new(42);
    assert_eq!(
        [
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
        ],
        [
            true, false, true, false, false, true, false, true, true, false, true, false,
        ]
    );
}

#[test]
fn xoroshiro_seed_zero_matches_java() {
    // Hand check: upgradeSeedTo128bit(0) mixes the pair
    //   (0 ^ SILVER, (0 ^ SILVER) + GOLDEN)
    // into [3847398142028685078, 7192185014346937746].
    // The first xoroshiro128++ nextLong() is 3038984756725240190.
    let seed = upgrade_seed_to_128bit(0);
    assert_eq!(
        seed,
        Seed128bit::new(3847398142028685078, 7192185014346937746)
    );

    let mut rng = XoroshiroRandomSource::new(0);
    assert_eq!(
        [
            rng.next_long(),
            rng.next_long(),
            rng.next_long(),
            rng.next_long(),
            rng.next_long(),
            rng.next_long(),
        ],
        [
            3038984756725240190,
            -3694039286755638414,
            4633751808701151732,
            2160572957309072155,
            1839370574944072389,
            -4488466507718817201,
        ]
    );

    let mut rng = XoroshiroRandomSource::new(0);
    assert_eq!(
        [
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
        ],
        [
            -160476802, 781697906, 653572596, 1337520923, -505875771, -47281585, 342195906,
            1417498593,
        ]
    );

    let mut rng = XoroshiroRandomSource::new(0);
    assert_eq!(
        [
            rng.next_double().to_bits(),
            rng.next_double().to_bits(),
            rng.next_double().to_bits(),
            rng.next_double().to_bits(),
        ],
        [
            4595103537318330368,
            4605378688765132800,
            4598196767933071360,
            4593104158638211072,
        ]
    );

    let mut rng = XoroshiroRandomSource::new(0);
    assert_eq!(
        [
            rng.next_float().to_bits(),
            rng.next_float().to_bits(),
            rng.next_float().to_bits(),
            rng.next_float().to_bits(),
            rng.next_float().to_bits(),
            rng.next_float().to_bits(),
        ],
        [
            1042854544, 1061993506, 1048616136, 1039130408, 1036793352, 1061270979
        ]
    );

    let mut rng = XoroshiroRandomSource::new(0);
    assert_eq!(
        [
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
        ],
        [
            false, false, false, true, true, true, false, true, true, false, true, false,
        ]
    );
}

#[test]
fn xoroshiro_seed_42_matches_java() {
    let seed = upgrade_seed_to_128bit(42);
    assert_eq!(
        seed,
        Seed128bit::new(6720814022939733433, -2851323883594622011)
    );

    let mut rng = XoroshiroRandomSource::new(42);
    assert_eq!(
        [
            rng.next_long(),
            rng.next_long(),
            rng.next_long(),
            rng.next_long(),
            rng.next_long(),
            rng.next_long(),
        ],
        [
            -4695948378737616609,
            7341713790291473579,
            -7542733514721318211,
            4888889476139319686,
            8419651034331256779,
            -6491934549477179079,
        ]
    );

    let mut rng = XoroshiroRandomSource::new(42);
    assert_eq!(
        [
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
            rng.next_int(),
        ],
        [
            1774573855,
            1357444267,
            -610653507,
            2070806918,
            -1441753141,
            1230225721,
            -290502145,
            1752826986,
        ]
    );

    let mut rng = XoroshiroRandomSource::new(42);
    assert_eq!(
        [
            rng.next_double().to_bits(),
            rng.next_double().to_bits(),
            rng.next_double().to_bits(),
            rng.next_double().to_bits(),
        ],
        [
            4604889475347120128,
            4600841262512209920,
            4603499443541508096,
            4598445925965234176,
        ]
    );

    let mut rng = XoroshiroRandomSource::new(42);
    assert_eq!(
        [
            rng.next_float().to_bits(),
            rng.next_float().to_bits(),
            rng.next_float().to_bits(),
            rng.next_float().to_bits(),
            rng.next_float().to_bits(),
            rng.next_float().to_bits(),
        ],
        [
            1061082275, 1053541890, 1058493139, 1049080230, 1055502648, 1059448835
        ]
    );

    let mut rng = XoroshiroRandomSource::new(42);
    assert_eq!(
        [
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
            rng.next_boolean(),
        ],
        [
            true, true, true, false, true, true, true, false, true, true, true, true,
        ]
    );
}

// ---------------------------------------------------------------------------
// Bounds
// ---------------------------------------------------------------------------

#[test]
fn legacy_bounds_match_java() {
    // Seed 42. Power-of-two bounds use the 64-bit multiply path; the other
    // bounds use rejection sampling.
    let mut rng = LegacyRandomSource::new(42);
    assert_eq!(
        [
            rng.next_int_bound(1),
            rng.next_int_bound(1),
            rng.next_int_bound(1),
            rng.next_int_bound(1),
        ],
        [0, 0, 0, 0]
    );

    let mut rng = LegacyRandomSource::new(42);
    assert_eq!(
        [
            rng.next_int_bound(2),
            rng.next_int_bound(2),
            rng.next_int_bound(2),
            rng.next_int_bound(2),
            rng.next_int_bound(2),
            rng.next_int_bound(2),
            rng.next_int_bound(2),
            rng.next_int_bound(2),
        ],
        [1, 0, 1, 0, 0, 1, 0, 1]
    );

    let mut rng = LegacyRandomSource::new(42);
    assert_eq!(
        [
            rng.next_int_bound(3),
            rng.next_int_bound(3),
            rng.next_int_bound(3),
            rng.next_int_bound(3),
            rng.next_int_bound(3),
            rng.next_int_bound(3),
            rng.next_int_bound(3),
            rng.next_int_bound(3),
        ],
        [2, 0, 0, 2, 0, 1, 2, 2]
    );

    let mut rng = LegacyRandomSource::new(42);
    assert_eq!(
        [
            rng.next_int_bound(4),
            rng.next_int_bound(4),
            rng.next_int_bound(4),
            rng.next_int_bound(4),
            rng.next_int_bound(4),
            rng.next_int_bound(4),
            rng.next_int_bound(4),
            rng.next_int_bound(4),
        ],
        [2, 0, 2, 0, 1, 3, 1, 2]
    );

    let mut rng = LegacyRandomSource::new(42);
    assert_eq!(
        [
            rng.next_int_bound(7),
            rng.next_int_bound(7),
            rng.next_int_bound(7),
            rng.next_int_bound(7),
            rng.next_int_bound(7),
            rng.next_int_bound(7),
            rng.next_int_bound(7),
            rng.next_int_bound(7),
        ],
        [1, 5, 6, 3, 5, 4, 1, 3]
    );

    let mut rng = LegacyRandomSource::new(42);
    assert_eq!(
        [
            rng.next_int_bound(8),
            rng.next_int_bound(8),
            rng.next_int_bound(8),
            rng.next_int_bound(8),
            rng.next_int_bound(8),
            rng.next_int_bound(8),
            rng.next_int_bound(8),
            rng.next_int_bound(8),
        ],
        [5, 0, 5, 0, 2, 7, 2, 5]
    );

    let mut rng = LegacyRandomSource::new(42);
    assert_eq!(
        [
            rng.next_int_bound(16),
            rng.next_int_bound(16),
            rng.next_int_bound(16),
            rng.next_int_bound(16),
            rng.next_int_bound(16),
            rng.next_int_bound(16),
            rng.next_int_bound(16),
            rng.next_int_bound(16),
        ],
        [11, 0, 10, 0, 4, 15, 4, 11]
    );

    let mut rng = LegacyRandomSource::new(42);
    assert_eq!(
        [
            rng.next_int_bound(100),
            rng.next_int_bound(100),
            rng.next_int_bound(100),
            rng.next_int_bound(100),
            rng.next_int_bound(100),
            rng.next_int_bound(100),
            rng.next_int_bound(100),
            rng.next_int_bound(100),
        ],
        [30, 63, 48, 84, 70, 25, 5, 18]
    );

    let mut rng = LegacyRandomSource::new(42);
    assert_eq!(
        [
            rng.next_int_bound(1000),
            rng.next_int_bound(1000),
            rng.next_int_bound(1000),
            rng.next_int_bound(1000),
            rng.next_int_bound(1000),
            rng.next_int_bound(1000),
            rng.next_int_bound(1000),
            rng.next_int_bound(1000),
        ],
        [130, 763, 248, 884, 970, 525, 505, 918]
    );

    let mut rng = LegacyRandomSource::new(42);
    assert_eq!(
        [
            rng.next_int_bound(1024),
            rng.next_int_bound(1024),
            rng.next_int_bound(1024),
            rng.next_int_bound(1024),
            rng.next_int_bound(1024),
            rng.next_int_bound(1024),
            rng.next_int_bound(1024),
            rng.next_int_bound(1024),
        ],
        [745, 55, 699, 49, 316, 964, 283, 724]
    );

    let mut rng = LegacyRandomSource::new(42);
    assert_eq!(
        [
            rng.next_int_bound(i32::MAX),
            rng.next_int_bound(i32::MAX),
            rng.next_int_bound(i32::MAX),
            rng.next_int_bound(i32::MAX),
            rng.next_int_bound(i32::MAX),
            rng.next_int_bound(i32::MAX),
            rng.next_int_bound(i32::MAX),
            rng.next_int_bound(i32::MAX),
        ],
        [
            1562431130, 117392763, 1467211248, 102948884, 662969970, 2023087525, 595021505,
            1519796918,
        ]
    );
}

#[test]
fn xoroshiro_bounds_match_java() {
    // Seed 0. xoroshiro uses the Lemire multiply-and-reject algorithm.
    let mut rng = XoroshiroRandomSource::new(0);
    assert_eq!(
        [
            rng.next_int_bound(1),
            rng.next_int_bound(1),
            rng.next_int_bound(1),
            rng.next_int_bound(1),
        ],
        [0, 0, 0, 0]
    );

    let mut rng = XoroshiroRandomSource::new(0);
    assert_eq!(
        [
            rng.next_int_bound(2),
            rng.next_int_bound(2),
            rng.next_int_bound(2),
            rng.next_int_bound(2),
            rng.next_int_bound(2),
            rng.next_int_bound(2),
            rng.next_int_bound(2),
            rng.next_int_bound(2),
        ],
        [1, 0, 0, 0, 1, 1, 0, 0]
    );

    let mut rng = XoroshiroRandomSource::new(0);
    assert_eq!(
        [
            rng.next_int_bound(3),
            rng.next_int_bound(3),
            rng.next_int_bound(3),
            rng.next_int_bound(3),
            rng.next_int_bound(3),
            rng.next_int_bound(3),
            rng.next_int_bound(3),
            rng.next_int_bound(3),
        ],
        [2, 0, 0, 0, 2, 2, 0, 0]
    );

    let mut rng = XoroshiroRandomSource::new(0);
    assert_eq!(
        [
            rng.next_int_bound(4),
            rng.next_int_bound(4),
            rng.next_int_bound(4),
            rng.next_int_bound(4),
            rng.next_int_bound(4),
            rng.next_int_bound(4),
            rng.next_int_bound(4),
            rng.next_int_bound(4),
        ],
        [3, 0, 0, 1, 3, 3, 0, 1]
    );

    let mut rng = XoroshiroRandomSource::new(0);
    assert_eq!(
        [
            rng.next_int_bound(7),
            rng.next_int_bound(7),
            rng.next_int_bound(7),
            rng.next_int_bound(7),
            rng.next_int_bound(7),
            rng.next_int_bound(7),
            rng.next_int_bound(7),
            rng.next_int_bound(7),
        ],
        [6, 1, 1, 2, 6, 6, 0, 2]
    );

    let mut rng = XoroshiroRandomSource::new(0);
    assert_eq!(
        [
            rng.next_int_bound(8),
            rng.next_int_bound(8),
            rng.next_int_bound(8),
            rng.next_int_bound(8),
            rng.next_int_bound(8),
            rng.next_int_bound(8),
            rng.next_int_bound(8),
            rng.next_int_bound(8),
        ],
        [7, 1, 1, 2, 7, 7, 0, 2]
    );

    let mut rng = XoroshiroRandomSource::new(0);
    assert_eq!(
        [
            rng.next_int_bound(16),
            rng.next_int_bound(16),
            rng.next_int_bound(16),
            rng.next_int_bound(16),
            rng.next_int_bound(16),
            rng.next_int_bound(16),
            rng.next_int_bound(16),
            rng.next_int_bound(16),
        ],
        [15, 2, 2, 4, 14, 15, 1, 5]
    );

    let mut rng = XoroshiroRandomSource::new(0);
    assert_eq!(
        [
            rng.next_int_bound(100),
            rng.next_int_bound(100),
            rng.next_int_bound(100),
            rng.next_int_bound(100),
            rng.next_int_bound(100),
            rng.next_int_bound(100),
            rng.next_int_bound(100),
            rng.next_int_bound(100),
        ],
        [96, 18, 15, 31, 88, 98, 7, 33]
    );

    let mut rng = XoroshiroRandomSource::new(0);
    assert_eq!(
        [
            rng.next_int_bound(1000),
            rng.next_int_bound(1000),
            rng.next_int_bound(1000),
            rng.next_int_bound(1000),
            rng.next_int_bound(1000),
            rng.next_int_bound(1000),
            rng.next_int_bound(1000),
            rng.next_int_bound(1000),
        ],
        [962, 182, 152, 311, 882, 988, 79, 330]
    );

    let mut rng = XoroshiroRandomSource::new(0);
    assert_eq!(
        [
            rng.next_int_bound(1024),
            rng.next_int_bound(1024),
            rng.next_int_bound(1024),
            rng.next_int_bound(1024),
            rng.next_int_bound(1024),
            rng.next_int_bound(1024),
            rng.next_int_bound(1024),
            rng.next_int_bound(1024),
        ],
        [985, 186, 155, 318, 903, 1012, 81, 337]
    );

    let mut rng = XoroshiroRandomSource::new(0);
    assert_eq!(
        [
            rng.next_int_bound(i32::MAX),
            rng.next_int_bound(i32::MAX),
            rng.next_int_bound(i32::MAX),
            rng.next_int_bound(i32::MAX),
            rng.next_int_bound(i32::MAX),
            rng.next_int_bound(i32::MAX),
            rng.next_int_bound(i32::MAX),
            rng.next_int_bound(i32::MAX),
        ],
        [
            2067245246, 390848952, 326786297, 668760461, 1894545761, 2123842854, 171097952,
            708749296,
        ]
    );
}

#[test]
fn legacy_bounds_with_rejection_match_java() {
    // These bounds make the `i - i1 + (bound - 1) < 0` rejection condition
    // fire frequently, unlike small bounds where the probability is tiny.
    let mut rng = LegacyRandomSource::new(42);
    assert_eq!(
        [
            rng.next_int_bound(1_073_741_825),
            rng.next_int_bound(1_073_741_825),
            rng.next_int_bound(1_073_741_825),
            rng.next_int_bound(1_073_741_825),
            rng.next_int_bound(1_073_741_825),
            rng.next_int_bound(1_073_741_825),
            rng.next_int_bound(1_073_741_825),
            rng.next_int_bound(1_073_741_825),
        ],
        [
            117392763, 102948884, 662969970, 595021505, 196118093, 969067502, 791955276, 819572292
        ]
    );

    let mut rng = LegacyRandomSource::new(42);
    assert_eq!(
        [
            rng.next_int_bound(1_000_000_007),
            rng.next_int_bound(1_000_000_007),
            rng.next_int_bound(1_000_000_007),
            rng.next_int_bound(1_000_000_007),
            rng.next_int_bound(1_000_000_007),
            rng.next_int_bound(1_000_000_007),
            rng.next_int_bound(1_000_000_007),
            rng.next_int_bound(1_000_000_007),
        ],
        [
            562431123, 117392763, 467211241, 102948884, 662969970, 595021505, 519796911, 429255512
        ]
    );

    let mut rng = LegacyRandomSource::new(42);
    assert_eq!(
        [
            rng.next_int_bound(2_000_000_000),
            rng.next_int_bound(2_000_000_000),
            rng.next_int_bound(2_000_000_000),
            rng.next_int_bound(2_000_000_000),
            rng.next_int_bound(2_000_000_000),
            rng.next_int_bound(2_000_000_000),
            rng.next_int_bound(2_000_000_000),
            rng.next_int_bound(2_000_000_000),
        ],
        [
            1562431130, 117392763, 1467211248, 102948884, 662969970, 595021505, 1519796918,
            1429255519,
        ]
    );
}

#[test]
fn xoroshiro_bounds_with_rejection_match_java() {
    // lemire rejection threshold path for xoroshiro.
    let mut rng = XoroshiroRandomSource::new(0);
    assert_eq!(
        [
            rng.next_int_bound(1_073_741_825),
            rng.next_int_bound(1_073_741_825),
            rng.next_int_bound(1_073_741_825),
            rng.next_int_bound(1_073_741_825),
            rng.next_int_bound(1_073_741_825),
            rng.next_int_bound(1_073_741_825),
            rng.next_int_bound(1_073_741_825),
            rng.next_int_bound(1_073_741_825),
        ],
        [
            1033622624, 195424476, 1061921428, 85548976, 354374648, 704019963, 390020067,
            626227589,
        ]
    );

    let mut rng = XoroshiroRandomSource::new(0);
    assert_eq!(
        [
            rng.next_int_bound(1_000_000_007),
            rng.next_int_bound(1_000_000_007),
            rng.next_int_bound(1_000_000_007),
            rng.next_int_bound(1_000_000_007),
            rng.next_int_bound(1_000_000_007),
            rng.next_int_bound(1_000_000_007),
            rng.next_int_bound(1_000_000_007),
            rng.next_int_bound(1_000_000_007),
        ],
        [
            962636089, 182003227, 152171729, 311415859, 882216624, 79673693, 330037112, 655669689,
        ]
    );

    let mut rng = XoroshiroRandomSource::new(0);
    assert_eq!(
        [
            rng.next_int_bound(2_000_000_000),
            rng.next_int_bound(2_000_000_000),
            rng.next_int_bound(2_000_000_000),
            rng.next_int_bound(2_000_000_000),
            rng.next_int_bound(2_000_000_000),
            rng.next_int_bound(2_000_000_000),
            rng.next_int_bound(2_000_000_000),
            rng.next_int_bound(2_000_000_000),
        ],
        [
            1925272165, 364006453, 304343456, 622831714, 1764433237, 1977982796, 159347386,
            660074219,
        ]
    );
}

#[test]
fn bounds_are_always_in_range() {
    let bounds = [1, 2, 3, 4, 5, 7, 8, 16, 17, 100, 1000, i32::MAX];

    for &bound in &bounds {
        let mut legacy = LegacyRandomSource::new(0xC0FFEE);
        let mut xoroshiro = XoroshiroRandomSource::new(0xC0FFEE);

        for _ in 0..2_000 {
            let legacy_value = legacy.next_int_bound(bound);
            assert!(
                (0..bound).contains(&legacy_value),
                "legacy bound {bound} produced {legacy_value}"
            );

            let xoroshiro_value = xoroshiro.next_int_bound(bound);
            assert!(
                (0..bound).contains(&xoroshiro_value),
                "xoroshiro bound {bound} produced {xoroshiro_value}"
            );
        }
    }
}

#[test]
#[should_panic(expected = "Bound must be positive")]
fn legacy_next_int_bound_zero_panics() {
    let mut rng = LegacyRandomSource::new(0);
    rng.next_int_bound(0);
}

#[test]
#[should_panic(expected = "Bound must be positive")]
fn xoroshiro_next_int_bound_zero_panics() {
    let mut rng = XoroshiroRandomSource::new(0);
    rng.next_int_bound(0);
}

#[test]
#[should_panic(expected = "Bound must be positive")]
fn legacy_next_int_bound_negative_panics() {
    let mut rng = LegacyRandomSource::new(0);
    rng.next_int_bound(-1);
}

#[test]
#[should_panic(expected = "Bound must be positive")]
fn xoroshiro_next_int_bound_negative_panics() {
    let mut rng = XoroshiroRandomSource::new(0);
    rng.next_int_bound(i32::MIN);
}

// ---------------------------------------------------------------------------
// Repeatability and reseeding
// ---------------------------------------------------------------------------

#[test]
fn repeated_instances_are_deterministic() {
    let mut legacy_a = LegacyRandomSource::new(0x1234_5678_9ABC_DEF0);
    let mut legacy_b = LegacyRandomSource::new(0x1234_5678_9ABC_DEF0);
    let mut xoroshiro_a = XoroshiroRandomSource::new(0x1234_5678_9ABC_DEF0);
    let mut xoroshiro_b = XoroshiroRandomSource::new(0x1234_5678_9ABC_DEF0);

    for _ in 0..500 {
        assert_eq!(legacy_a.next_int(), legacy_b.next_int());
        assert_eq!(legacy_a.next_long(), legacy_b.next_long());
        assert_eq!(
            legacy_a.next_double().to_bits(),
            legacy_b.next_double().to_bits()
        );
        assert_eq!(
            legacy_a.next_float().to_bits(),
            legacy_b.next_float().to_bits()
        );
        assert_eq!(legacy_a.next_boolean(), legacy_b.next_boolean());
        assert_eq!(legacy_a.next_int_bound(1000), legacy_b.next_int_bound(1000));

        assert_eq!(xoroshiro_a.next_int(), xoroshiro_b.next_int());
        assert_eq!(xoroshiro_a.next_long(), xoroshiro_b.next_long());
        assert_eq!(
            xoroshiro_a.next_double().to_bits(),
            xoroshiro_b.next_double().to_bits()
        );
        assert_eq!(
            xoroshiro_a.next_float().to_bits(),
            xoroshiro_b.next_float().to_bits()
        );
        assert_eq!(xoroshiro_a.next_boolean(), xoroshiro_b.next_boolean());
        assert_eq!(
            xoroshiro_a.next_int_bound(1000),
            xoroshiro_b.next_int_bound(1000)
        );
    }
}

#[test]
fn legacy_set_seed_replays_sequence() {
    let mut rng = LegacyRandomSource::new(7);
    let first: Vec<(i32, i64, u64, u32, bool)> = (0..100)
        .map(|_| {
            (
                rng.next_int(),
                rng.next_long(),
                rng.next_double().to_bits(),
                rng.next_float().to_bits(),
                rng.next_boolean(),
            )
        })
        .collect();

    rng.set_seed(7);
    let second: Vec<(i32, i64, u64, u32, bool)> = (0..100)
        .map(|_| {
            (
                rng.next_int(),
                rng.next_long(),
                rng.next_double().to_bits(),
                rng.next_float().to_bits(),
                rng.next_boolean(),
            )
        })
        .collect();

    assert_eq!(first, second);
}

#[test]
fn xoroshiro_set_seed_replays_sequence() {
    let mut rng = XoroshiroRandomSource::new(7);
    let first: Vec<(i32, i64, u64, u32, bool)> = (0..100)
        .map(|_| {
            (
                rng.next_int(),
                rng.next_long(),
                rng.next_double().to_bits(),
                rng.next_float().to_bits(),
                rng.next_boolean(),
            )
        })
        .collect();

    rng.set_seed(7);
    let second: Vec<(i32, i64, u64, u32, bool)> = (0..100)
        .map(|_| {
            (
                rng.next_int(),
                rng.next_long(),
                rng.next_double().to_bits(),
                rng.next_float().to_bits(),
                rng.next_boolean(),
            )
        })
        .collect();

    assert_eq!(first, second);
}

// ---------------------------------------------------------------------------
// Seed mixing / raw generator
// ---------------------------------------------------------------------------

#[test]
fn seed_mixing_matches_java() {
    assert_eq!(mix_stafford13(0), 0);
    assert_eq!(mix_stafford13(42), -6387817139659442654);
    assert_eq!(mix_stafford13(-1), -5417735806833148549);
    assert_eq!(mix_stafford13(123_456_789), -1000775671988880032);

    assert_eq!(
        upgrade_seed_to_128bit(0),
        Seed128bit::new(3847398142028685078, 7192185014346937746)
    );
    assert_eq!(
        upgrade_seed_to_128bit(42),
        Seed128bit::new(6720814022939733433, -2851323883594622011)
    );
}

#[test]
fn xoroshiro_zero_state_uses_fallback_constants() {
    let mut zero = Xoroshiro128PlusPlus::new(0, 0);
    let mut fallback = Xoroshiro128PlusPlus::new(GOLDEN_RATIO_64, SILVER_RATIO_64);

    // Java: new Xoroshiro128PlusPlus(0, 0).nextLong() == 6807859099481836695.
    for _ in 0..16 {
        assert_eq!(zero.next_long(), fallback.next_long());
    }

    let mut zero = Xoroshiro128PlusPlus::new(0, 0);
    assert_eq!(zero.next_long(), 6807859099481836695);
}

#[test]
fn extra_seeds_match_java() {
    // Negative / arbitrary 64-bit seeds exercise the full seed word.
    let mut legacy = LegacyRandomSource::new(-1);
    assert_eq!(
        [
            legacy.next_int(),
            legacy.next_int(),
            legacy.next_int(),
            legacy.next_int(),
            legacy.next_int(),
        ],
        [1155099827, 1887904451, 52699159, -1941176418, -1451336087]
    );

    let mut legacy = LegacyRandomSource::new(-1);
    assert_eq!(
        [
            legacy.next_long(),
            legacy.next_long(),
            legacy.next_long(),
            legacy.next_long(),
            legacy.next_long(),
        ],
        [
            4961115982468162243,
            226341162490527646,
            -6233441030884181172,
            7681931065131779340,
            -3206673117535979274,
        ]
    );

    let mut xoroshiro = XoroshiroRandomSource::new(-1);
    assert_eq!(
        [
            xoroshiro.next_long(),
            xoroshiro.next_long(),
            xoroshiro.next_long(),
            xoroshiro.next_long(),
            xoroshiro.next_long(),
        ],
        [
            -8676505878415342125,
            -868585888688873692,
            -6331679347063163302,
            -2068491455652362927,
            -5626054917968568837,
        ]
    );

    let mut xoroshiro = XoroshiroRandomSource::new(123_456_789);
    assert_eq!(
        [
            xoroshiro.next_long(),
            xoroshiro.next_long(),
            xoroshiro.next_long(),
            xoroshiro.next_long(),
            xoroshiro.next_long(),
        ],
        [
            3219654894476264721,
            8777179534091651608,
            6434035011650043425,
            -8486945822236614382,
            3449493904201163115,
        ]
    );
}

#[test]
fn forks_are_deterministic() {
    // Legacy's fork uses the parent's nextLong() as the child's seed.
    let mut legacy = LegacyRandomSource::new(42);
    let mut legacy_fork = legacy.fork();
    let mut legacy_parent = LegacyRandomSource::new(42);
    let fork_seed = legacy_parent.next_long();
    let mut legacy_reference = LegacyRandomSource::new(fork_seed);
    assert_eq!(legacy_fork.next_long(), legacy_reference.next_long());

    // Xoroshiro's fork uses two raw nextLong() draws as the child state.
    let mut xoroshiro = XoroshiroRandomSource::new(42);
    let mut xoroshiro_fork = xoroshiro.fork();
    let mut xoroshiro_parent = XoroshiroRandomSource::new(42);
    let seed_lo = xoroshiro_parent.next_long();
    let seed_hi = xoroshiro_parent.next_long();
    let mut xoroshiro_reference = XoroshiroRandomSource::from_parts(seed_lo, seed_hi);
    assert_eq!(xoroshiro_fork.next_long(), xoroshiro_reference.next_long());
}

#[test]
fn consume_count_matches_java_semantics() {
    // Legacy consumes ints: after 3 ints, the next int must match.
    let mut consumed = LegacyRandomSource::new(42);
    consumed.consume_count(3);
    let mut expected = LegacyRandomSource::new(42);
    expected.next_int();
    expected.next_int();
    expected.next_int();
    assert_eq!(consumed.next_int(), expected.next_int());

    // Xoroshiro consumes whole longs.
    let mut consumed = XoroshiroRandomSource::new(42);
    consumed.consume_count(3);
    let mut expected = XoroshiroRandomSource::new(42);
    expected.next_long();
    expected.next_long();
    expected.next_long();
    assert_eq!(consumed.next_long(), expected.next_long());
}

#[test]
fn range_helpers_match_java_defaults() {
    let mut legacy = LegacyRandomSource::new(42);
    let mut reference = LegacyRandomSource::new(42);
    for _ in 0..100 {
        let min = -50;
        let max = 50;
        let value = legacy.next_int_between_inclusive(min, max);
        let expected = reference.next_int_bound(max - min + 1) + min;
        assert_eq!(value, expected);
        assert!((min..=max).contains(&value));
    }

    let mut xoroshiro = XoroshiroRandomSource::new(42);
    let mut reference = XoroshiroRandomSource::new(42);
    for _ in 0..100 {
        let origin = -25;
        let bound = 75;
        let value = xoroshiro.next_int_in_range(origin, bound);
        let expected = reference.next_int_bound(bound - origin) + origin;
        assert_eq!(value, expected);
        assert!((origin..bound).contains(&value));
    }
}
