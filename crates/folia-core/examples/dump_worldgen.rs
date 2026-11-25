use folia_core::worldgen::diagnostics::{DEFAULT_SAMPLE_STRIDE, WorldgenDump};
use folia_core::worldgen::generator::WorldGenerator;
use folia_core::worldgen::structure::StructureGenerator;
use std::io;

fn main() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let seed = args
        .next()
        .ok_or_else(|| usage("missing seed"))?
        .parse()
        .map_err(|_| usage("seed must be an i64"))?;
    let chunk_x = args
        .next()
        .ok_or_else(|| usage("missing chunk x"))?
        .parse()
        .map_err(|_| usage("chunk x must be an i32"))?;
    let chunk_z = args
        .next()
        .ok_or_else(|| usage("missing chunk z"))?
        .parse()
        .map_err(|_| usage("chunk z must be an i32"))?;
    let sample_stride = args.next().map_or(Ok(DEFAULT_SAMPLE_STRIDE), |value| {
        value
            .parse::<i32>()
            .map_err(|_| usage("sample stride must be an i32"))
    })?;
    if args.next().is_some() {
        return Err(usage("too many arguments"));
    }
    if sample_stride <= 0 {
        return Err(usage("sample stride must be positive"));
    }

    let generator = WorldGenerator::overworld(seed)?.with_structure_generator(
        StructureGenerator::shipped_catalog()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?,
    );
    let generated = generator.generate_chunk_with_diagnostics(chunk_x, chunk_z)?;
    let dump = WorldgenDump::from_chunk(seed, &generated, sample_stride);
    println!("{}", dump.to_json_string());
    Ok(())
}

fn usage(message: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "{message}\nusage: dump_worldgen <seed> <chunk-x> <chunk-z> [sample-stride={DEFAULT_SAMPLE_STRIDE}]"
        ),
    )
}
