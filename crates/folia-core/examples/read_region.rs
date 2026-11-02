use folia_core::anvil::RegionFile;
use folia_core::chunk::decode_chunk;
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: read_region <r.x.z.mca>");
    let region = RegionFile::open(&path)?;

    let mut chunks = 0usize;
    let mut sections = 0usize;
    let mut read_errors = 0usize;
    let mut decode_errors = 0usize;
    let mut first_error: Option<String> = None;

    for z in 0..32u8 {
        for x in 0..32u8 {
            let Some(tag) = (match region.read_chunk(x, z) {
                Ok(value) => value,
                Err(error) => {
                    read_errors += 1;
                    if first_error.is_none() {
                        first_error = Some(format!("read ({x},{z}): {error}"));
                    }
                    continue;
                }
            }) else {
                continue;
            };

            chunks += 1;
            match decode_chunk(&tag) {
                Ok(chunk) => {
                    sections += chunk.sections.len();
                    if chunks == 1 {
                        let block = chunk.block_state(0, 64, 0).map(|state| state.name.clone());
                        println!(
                            "first chunk: x={} z={} sections={} block(0,64,0)={:?}",
                            chunk.x,
                            chunk.z,
                            chunk.sections.len(),
                            block
                        );
                    }
                }
                Err(error) => {
                    decode_errors += 1;
                    if first_error.is_none() {
                        first_error = Some(format!("decode ({x},{z}): {error}"));
                    }
                }
            }
        }
    }

    println!(
        "chunks={chunks} sections={sections} read_errors={read_errors} decode_errors={decode_errors}"
    );
    if let Some(error) = first_error {
        println!("first_error={error}");
    }
    Ok(())
}
