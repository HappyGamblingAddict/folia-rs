//! Exploratory dump while wiring the 3x3 decoration biome region.

use folia_core::worldgen::biome::BiomeSampler;
use folia_core::worldgen::feature::{Decoration, FeatureBiomeRegion, FeatureData};

const SEED: i64 = 12345;

fn center_only(
    sampler: &BiomeSampler,
    data: &FeatureData,
    region: &FeatureBiomeRegion,
    step: Decoration,
    order: &[String],
) -> Vec<String> {
    let center_biomes = region.chunk_biomes(sampler, 0, 0).unwrap();
    let mut listed = std::collections::BTreeSet::new();
    for biome in &center_biomes {
        if let Some(steps) = data.biome_features(biome) {
            for id in &steps[step.index()] {
                listed.insert(id.as_str());
            }
        }
    }
    order
        .iter()
        .filter(|id| listed.contains(id.as_str()))
        .cloned()
        .collect()
}

#[test]
fn find_differences() {
    let sampler = BiomeSampler::overworld(SEED).unwrap();
    let data = FeatureData::vanilla();
    let order = data.step_feature_order();
    let steps = [
        Decoration::UndergroundOres,
        Decoration::UndergroundDecoration,
        Decoration::VegetalDecoration,
        Decoration::TopLayerModification,
    ];
    let mut shown = 0;
    'outer: for cz in -3..=1 {
        for cx in -2..=2 {
            let region = FeatureBiomeRegion::sample(&sampler, cx, cz, -64, 384);
            for step in steps {
                let region_features = region.active_features(data, step, &order[step.index()]);
                let base =
                    center_only(&sampler, data, &region, step, &order[step.index()]);
                let region_owned: Vec<String> =
                    region_features.iter().map(|id| (*id).to_owned()).collect();
                if region_owned != base {
                    println!(
                        "chunk ({cx},{cz}) step {} active={:?}",
                        step.name(),
                        region.active_biomes()
                    );
                    println!("  region-only: {:?}", diff(&region_owned, &base));
                    println!("  center-only: {:?}", diff(&base, &region_owned));
                    shown += 1;
                    if shown >= 5 {
                        break 'outer;
                    }
                }
            }
        }
    }
    assert!(shown > 0, "expected a chunk where the 3x3 window changes selection");
}

fn diff(a: &[String], b: &[String]) -> Vec<String> {
    a.iter().filter(|id| !b.contains(id)).cloned().collect()
}

#[test]
fn dump_ranges() {
    let sampler = BiomeSampler::overworld(SEED).unwrap();
    for cz in -1..=0 {
        for cx in 0..=1 {
            let region = FeatureBiomeRegion::sample(&sampler, cx, cz, -64, 384);
            println!(
                "({cx},{cz}) active={:?} center={:?} cells={}",
                region.active_biomes(),
                region.chunk_biomes(&sampler, 0, 0).unwrap(),
                region.center_cells().len()
            );
        }
    }
}
