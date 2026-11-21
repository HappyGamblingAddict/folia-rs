//! Records the target Minecraft version beside the generated assets and
//! verifies, at startup and in tests, that every embedded version marker
//! agrees. Mixing assets from two versions produces subtle, hard-to-find bugs.

use std::io;

/// The Minecraft/Folia version the shipped assets were generated from.
pub const MINECRAFT_VERSION: &str = "1.21.11";
/// The vanilla DataVersion the shipped assets were generated from.
pub const DATA_VERSION: i32 = 4788;

const MANIFEST: &str = include_str!("../assets/VERSION.txt");

/// JSON catalogs whose extractors stamp a top-level `data_version` field.
/// The field is scanned as a token (no full JSON tree) so startup stays cheap.
const VERSIONED_JSON_ASSETS: &[(&str, &str)] = &[
    (
        "item_prototypes.json",
        include_str!("../assets/item_prototypes.json"),
    ),
    (
        "loot_tables.json",
        include_str!("../assets/loot_tables.json"),
    ),
    ("recipes.json", include_str!("../assets/recipes.json")),
    (
        "structure_processors.json",
        include_str!("../assets/structure_processors.json"),
    ),
    (
        "structure_templates.json",
        include_str!("../assets/structure_templates.json"),
    ),
    (
        "structure_templates_more.json",
        include_str!("../assets/structure_templates_more.json"),
    ),
    (
        "structure_templates_trial.json",
        include_str!("../assets/structure_templates_trial.json"),
    ),
    (
        "template_pools.json",
        include_str!("../assets/template_pools.json"),
    ),
];

fn parse_manifest(text: &str) -> io::Result<(String, i32)> {
    let mut minecraft_version = None;
    let mut data_version = None;

    for (index, raw_line) in text.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "asset version manifest line {} is malformed: {line:?}",
                    index + 1
                ),
            ));
        };
        match key.trim() {
            "minecraft_version" => minecraft_version = Some(value.trim().to_string()),
            "data_version" => {
                let parsed = value.trim().parse::<i32>().map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("asset version manifest data_version is invalid: {error}"),
                    )
                })?;
                data_version = Some(parsed);
            }
            _ => {}
        }
    }

    let minecraft_version = minecraft_version.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "asset version manifest is missing minecraft_version",
        )
    })?;
    let data_version = data_version.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "asset version manifest is missing data_version",
        )
    })?;
    Ok((minecraft_version, data_version))
}

fn validate_parsed(minecraft_version: &str, data_version: i32) -> io::Result<()> {
    if minecraft_version != MINECRAFT_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "asset version manifest declares Minecraft {minecraft_version}, expected {MINECRAFT_VERSION}"
            ),
        ));
    }
    if data_version != DATA_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "asset version manifest declares DataVersion {data_version}, expected {DATA_VERSION}"
            ),
        ));
    }
    Ok(())
}

fn embedded_data_version(name: &str, json: &str) -> io::Result<i32> {
    const NEEDLE: &str = "\"data_version\":";
    let offset = json.find(NEEDLE).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("generated asset {name} is missing a `{NEEDLE}` field"),
        )
    })? + NEEDLE.len();
    let digits: String = json[offset..]
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect();
    digits.parse::<i32>().map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("generated asset {name} has an invalid data_version: {error}"),
        )
    })
}

/// Verifies the embedded manifest and every versioned JSON catalog against the
/// target version. Returns a descriptive error on any missing or mismatched
/// marker; a mismatch is never silently accepted.
pub fn validate() -> io::Result<()> {
    let (minecraft_version, data_version) = parse_manifest(MANIFEST)?;
    validate_parsed(&minecraft_version, data_version)?;
    for (name, json) in VERSIONED_JSON_ASSETS {
        let embedded = embedded_data_version(name, json)?;
        if embedded != DATA_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "generated asset {name} has DataVersion {embedded}, expected {DATA_VERSION}"
                ),
            ));
        }
    }
    Ok(())
}

/// Human-readable target version for startup logging.
pub fn describe() -> String {
    format!("Minecraft {MINECRAFT_VERSION} / DataVersion {DATA_VERSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_matches_target_version() {
        assert_eq!(MINECRAFT_VERSION, "1.21.11");
        assert_eq!(DATA_VERSION, 4788);
        let (minecraft_version, data_version) =
            parse_manifest(MANIFEST).expect("embedded manifest parses");
        assert_eq!(minecraft_version, MINECRAFT_VERSION);
        assert_eq!(data_version, DATA_VERSION);
    }

    #[test]
    fn validate_accepts_the_embedded_manifest() {
        validate().expect("embedded manifest and assets must validate");
        assert_eq!(describe(), "Minecraft 1.21.11 / DataVersion 4788");
    }

    #[test]
    fn parser_rejects_wrong_data_version() {
        let wrong = "minecraft_version=1.21.11\ndata_version=9999\n";
        let (minecraft_version, data_version) = parse_manifest(wrong).expect("manifest parses");
        assert_eq!(data_version, 9999);
        assert!(
            validate_parsed(&minecraft_version, data_version).is_err(),
            "a manifest with the wrong DataVersion must be rejected"
        );

        let malformed = "minecraft_version=1.21.11\ndata_version=not-a-number\n";
        assert!(
            parse_manifest(malformed).is_err(),
            "a non-numeric DataVersion must fail to parse"
        );
    }

    #[test]
    fn generated_json_catalogs_carry_the_target_data_version() {
        for (name, json) in VERSIONED_JSON_ASSETS {
            assert_eq!(
                embedded_data_version(name, json).expect("asset has a valid data_version"),
                DATA_VERSION,
                "{name} must carry the target DataVersion"
            );
        }
    }
}
