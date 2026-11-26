//! Initial datapack loading for world generation JSON.
//!
//! # Local root convention
//!
//! The configurable root points at a server/world directory and contains:
//!
//! ```text
//! <root>/datapacks/<pack>/data/<namespace>/worldgen/
//!     configured_feature/<path>.json
//!     placed_feature/<path>.json
//!     biome/<path>.json
//!     tags/block/<path>.json
//! ```
//!
//! A JSON file's id is `<namespace>:<path>` where `<path>` is the relative
//! file path without `.json` (`/` separators). Pack directories are visited in
//! sorted name order. `FOLIA_DATAPACK_DIR` overrides the directory (absolute,
//! or relative to `<root>`); without it the default is `<root>/datapacks`.
//! A missing directory is not an error and keeps the embedded vanilla data.
//!
//! # Supported subset
//!
//! - `configured_feature`: the same normalized shape as the `configured`
//!   section of `assets/feature_data.json`:
//!   `{"type": "<id>", "config": { ... }}`. Only configured-feature types with
//!   a Rust port execute; unported types are validated and stored as
//!   `Unsupported` and fail loudly if generation tries to place them.
//! - `placed_feature`: `{"feature": "<id>", "placement": [ ... ]}` with the
//!   same placement modifier JSON as the embedded catalog.
//! - `biome`: `{"features": {"steps": [[...], ...]}}` (the vanilla
//!   `Biome.features.steps` shape), or `{"steps": [...]}` / a bare list of at
//!   most 11 step lists. Each step is a list of placed-feature ids. An
//!   optional `climate` object with quantized integer ranges for
//!   `temperature`, `humidity`, `continentalness`, `erosion`, `depth`,
//!   `weirdness` plus an `offset` adds the biome to the multi-noise climate
//!   parameter list, so it can actually be sampled during generation.
//! - `tags/block`: `{"values": ["<id>", ...]}`; entries are appended to the
//!   two embedded worldgen ore tags.
//!
//! # Merge rules and validation
//!
//! Datapack JSON is parsed and shape-checked while loading, then merged into a
//! copy of the embedded vanilla catalog and fully re-validated by
//! [`FeatureData::parse`](crate::worldgen::feature::FeatureData::parse).
//! Vanilla configured/placed ids cannot be redefined (a clear error names the
//! collision); biome steps and tag values are appended, so every vanilla entry
//! survives. Biome ids must be new when `climate` is supplied. Every error
//! includes the offending file or id. There is no `replace` support for tags
//! or existing entries in this slice, and no network access is performed.

use crate::worldgen::biome::CustomBiomeClimate;
use crate::worldgen::feature::{DECORATION_STEPS, FeatureData};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Default datapack directory name under a server/world root.
pub const DEFAULT_DATAPACK_ROOT: &str = "datapacks";

/// Validated worldgen JSON collected from one or more datapacks.
#[derive(Debug, Default, Clone)]
pub struct DatapackOverlay {
    pub(crate) configured: Map<String, Value>,
    pub(crate) placed: Map<String, Value>,
    pub(crate) biomes: Map<String, Value>,
    pub(crate) tags: Map<String, Value>,
    pub(crate) climates: BTreeMap<String, CustomBiomeClimate>,
    packs: Vec<String>,
}

impl DatapackOverlay {
    /// Loads datapacks from `<root>/datapacks`.
    pub fn load_root(root: impl AsRef<Path>) -> io::Result<Self> {
        Self::load_datapacks_dir(root.as_ref().join(DEFAULT_DATAPACK_ROOT))
    }

    /// Loads every `<dir>/<pack>/data/<namespace>/worldgen/...` document.
    pub fn load_datapacks_dir(dir: impl AsRef<Path>) -> io::Result<Self> {
        let dir = dir.as_ref();
        let mut overlay = Self::default();
        if !dir.is_dir() {
            return Ok(overlay);
        }
        let mut packs = fs::read_dir(dir)
            .map_err(|error| {
                invalid(format!(
                    "cannot read datapack directory {}: {error}",
                    dir.display()
                ))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| invalid(format!("cannot read datapack entry: {error}")))?;
        packs.sort_by_key(|entry| entry.file_name());
        for entry in packs {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            overlay.load_pack_into(&path, &name)?;
        }
        Ok(overlay)
    }

    /// Loads a single datapack directory.
    pub fn load_pack(pack: &Path, name: &str) -> io::Result<Self> {
        let mut overlay = Self::default();
        overlay.load_pack_into(pack, name)?;
        Ok(overlay)
    }

    fn load_pack_into(&mut self, pack: &Path, name: &str) -> io::Result<()> {
        validate_component(name)
            .map_err(|error| invalid(format!("invalid datapack name `{name}`: {error}")))?;
        let data = pack.join("data");
        if !data.is_dir() {
            return Ok(());
        }
        let mut namespaces = fs::read_dir(&data)
            .map_err(|error| invalid(format!("cannot read {}: {error}", data.display())))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| invalid(format!("cannot read datapack namespace: {error}")))?;
        namespaces.sort_by_key(|entry| entry.file_name());
        let mut loaded = false;
        for namespace in namespaces {
            let path = namespace.path();
            if !path.is_dir() {
                continue;
            }
            let namespace_name = namespace.file_name().to_string_lossy().into_owned();
            validate_component(&namespace_name).map_err(|error| {
                invalid(format!(
                    "invalid namespace `{namespace_name}` in pack `{name}`: {error}"
                ))
            })?;
            loaded |= self.load_namespace(&path, &namespace_name)?;
        }
        if loaded {
            self.packs.push(name.to_owned());
        }
        Ok(())
    }

    fn load_namespace(&mut self, worldgen_namespace: &Path, namespace: &str) -> io::Result<bool> {
        let worldgen = worldgen_namespace.join("worldgen");
        if !worldgen.is_dir() {
            return Ok(false);
        }
        let mut loaded = false;
        for (folder, kind) in [
            ("configured_feature", RegistryKind::Configured),
            ("placed_feature", RegistryKind::Placed),
            ("biome", RegistryKind::Biome),
            ("tags/block", RegistryKind::Tag),
        ] {
            let base = worldgen.join(folder);
            if !base.is_dir() {
                continue;
            }
            let mut files = Vec::new();
            collect_json_files(&base, &mut Vec::new(), &mut files)?;
            for (relative, path) in files {
                let id = format!("{namespace}:{relative}");
                let text = fs::read_to_string(&path)
                    .map_err(|error| invalid(format!("cannot read {}: {error}", path.display())))?;
                let value: Value = serde_json::from_str(&text).map_err(|error| {
                    invalid(format!("{} is not valid JSON: {error}", path.display()))
                })?;
                match kind {
                    RegistryKind::Configured => {
                        validate_configured(&value, &id)
                            .map_err(|error| with_path(error, &path))?;
                        insert_unique(&mut self.configured, id, value, &path)?;
                    }
                    RegistryKind::Placed => {
                        validate_placed(&value, &id).map_err(|error| with_path(error, &path))?;
                        insert_unique(&mut self.placed, id, value, &path)?;
                    }
                    RegistryKind::Biome => {
                        let steps =
                            biome_steps(&value, &id).map_err(|error| with_path(error, &path))?;
                        insert_unique(&mut self.biomes, id.clone(), steps, &path)?;
                        if let Some(object) = value.as_object()
                            && let Some(climate) = object.get("climate")
                        {
                            let parsed = CustomBiomeClimate::parse(climate)
                                .map_err(|error| with_path(error, &path))?;
                            if self.climates.insert(id.clone(), parsed).is_some() {
                                return Err(invalid(format!("duplicate custom biome `{id}`")));
                            }
                        }
                    }
                    RegistryKind::Tag => {
                        validate_tag(&value, &id).map_err(|error| with_path(error, &path))?;
                        insert_unique(&mut self.tags, id, value, &path)?;
                    }
                }
                loaded = true;
            }
        }
        Ok(loaded)
    }

    /// Packs that contributed at least one worldgen document.
    pub fn packs(&self) -> &[String] {
        &self.packs
    }

    pub fn is_empty(&self) -> bool {
        self.configured.is_empty()
            && self.placed.is_empty()
            && self.biomes.is_empty()
            && self.tags.is_empty()
    }

    pub fn configured_count(&self) -> usize {
        self.configured.len()
    }

    pub fn placed_count(&self) -> usize {
        self.placed.len()
    }

    pub fn biome_count(&self) -> usize {
        self.biomes.len()
    }

    pub fn tag_count(&self) -> usize {
        self.tags.len()
    }

    pub fn climates(&self) -> &BTreeMap<String, CustomBiomeClimate> {
        &self.climates
    }
}

#[derive(Debug, Clone, Copy)]
enum RegistryKind {
    Configured,
    Placed,
    Biome,
    Tag,
}

fn insert_unique(
    map: &mut Map<String, Value>,
    id: String,
    value: Value,
    path: &Path,
) -> io::Result<()> {
    if map.contains_key(&id) {
        return Err(invalid(format!(
            "duplicate datapack id `{id}` (second definition in {})",
            path.display()
        )));
    }
    map.insert(id, value);
    Ok(())
}

fn collect_json_files(
    base: &Path,
    relative: &mut Vec<String>,
    out: &mut Vec<(String, PathBuf)>,
) -> io::Result<()> {
    let mut entries = fs::read_dir(base)
        .map_err(|error| invalid(format!("cannot read {}: {error}", base.display())))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| invalid(format!("cannot read datapack file: {error}")))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            validate_component(&name).map_err(|error| with_path(error, &path))?;
            relative.push(name);
            collect_json_files(&path, relative, out)?;
            relative.pop();
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or_else(|| invalid(format!("invalid JSON file name in {}", path.display())))?;
            validate_component(stem).map_err(|error| with_path(error, &path))?;
            relative.push(stem.to_owned());
            out.push((relative.join("/"), path));
            relative.pop();
        }
    }
    Ok(())
}

fn validate_component(name: &str) -> io::Result<()> {
    let valid = !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('\\')
        && name.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' || c == '.'
        });
    if valid {
        Ok(())
    } else {
        Err(invalid(format!(
            "invalid resource-location component `{name}`"
        )))
    }
}

fn validate_configured(value: &Value, id: &str) -> io::Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid(format!("configured feature `{id}` must be an object")))?;
    let feature_type = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("configured feature `{id}` is missing `type`")))?;
    if feature_type.is_empty() {
        return Err(invalid(format!(
            "configured feature `{id}` has an empty `type`"
        )));
    }
    if let Some(config) = object.get("config")
        && !config.is_object()
    {
        return Err(invalid(format!(
            "configured feature `{id}` has a non-object `config`"
        )));
    }
    Ok(())
}

fn validate_placed(value: &Value, id: &str) -> io::Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid(format!("placed feature `{id}` must be an object")))?;
    let feature = object
        .get("feature")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid(format!("placed feature `{id}` is missing `feature`")))?;
    if feature.is_empty() {
        return Err(invalid(format!(
            "placed feature `{id}` has an empty `feature`"
        )));
    }
    let placement = object
        .get("placement")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(format!("placed feature `{id}` is missing `placement`")))?;
    for (index, modifier) in placement.iter().enumerate() {
        let modifier = modifier.as_object().ok_or_else(|| {
            invalid(format!(
                "placed feature `{id}` modifier {index} must be an object"
            ))
        })?;
        if modifier.get("type").and_then(Value::as_str).is_none() {
            return Err(invalid(format!(
                "placed feature `{id}` modifier {index} is missing `type`"
            )));
        }
    }
    Ok(())
}

fn validate_tag(value: &Value, id: &str) -> io::Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid(format!("tag `{id}` must be an object")))?;
    let values = object
        .get("values")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid(format!("tag `{id}` is missing `values`")))?;
    for entry in values {
        if !entry.is_string() {
            return Err(invalid(format!("tag `{id}` contains a non-string value")));
        }
    }
    Ok(())
}

fn biome_steps(value: &Value, id: &str) -> io::Result<Value> {
    let steps = if let Some(object) = value.as_object() {
        if let Some(features) = object.get("features") {
            let features = features
                .as_object()
                .ok_or_else(|| invalid(format!("biome `{id}` has a non-object `features`")))?;
            features
                .get("steps")
                .ok_or_else(|| invalid(format!("biome `{id}` is missing `features.steps`")))?
        } else {
            object
                .get("steps")
                .ok_or_else(|| invalid(format!("biome `{id}` is missing `steps`")))?
        }
    } else {
        value
    };
    let steps = steps
        .as_array()
        .ok_or_else(|| invalid(format!("biome `{id}` steps must be a list")))?;
    if steps.len() > DECORATION_STEPS {
        return Err(invalid(format!(
            "biome `{id}` has {} feature steps, more than {DECORATION_STEPS}",
            steps.len()
        )));
    }
    let mut parsed = Vec::with_capacity(DECORATION_STEPS);
    for (index, step) in steps.iter().enumerate() {
        let list = step
            .as_array()
            .ok_or_else(|| invalid(format!("biome `{id}` step {index} must be a list")))?;
        if let Some(entry) = list.iter().find(|entry| !entry.is_string()) {
            return Err(invalid(format!(
                "biome `{id}` step {index} contains a non-string entry: {entry}"
            )));
        }
        parsed.push(step.clone());
    }
    parsed.resize(DECORATION_STEPS, Value::Array(Vec::new()));
    Ok(Value::Array(parsed))
}

fn with_path(error: io::Error, path: &Path) -> io::Error {
    invalid(format!("{}: {error}", path.display()))
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// Outcome of [`install_from`], for startup logging.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatapackInstall {
    pub directory: PathBuf,
    pub packs: usize,
    pub configured: usize,
    pub placed: usize,
    pub biomes: usize,
    pub tags: usize,
}

impl fmt::Display for DatapackInstall {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{} pack(s) from {}: {} configured feature(s), {} placed feature(s), {} biome(s), {} tag(s)",
            self.packs,
            self.directory.display(),
            self.configured,
            self.placed,
            self.biomes,
            self.tags
        )
    }
}

fn datapack_dir(root: &Path) -> PathBuf {
    if let Some(override_dir) = std::env::var_os("FOLIA_DATAPACK_DIR") {
        let path = PathBuf::from(override_dir);
        if path.is_absolute() {
            return path;
        }
        return root.join(path);
    }
    root.join(DEFAULT_DATAPACK_ROOT)
}

/// Loads, validates and installs datapack worldgen data for the process.
///
/// Called once at startup before `generator::configure`. Vanilla data is the
/// fallback whenever the directory is absent or empty.
pub fn install_from(root: impl AsRef<Path>) -> io::Result<DatapackInstall> {
    let root = root.as_ref();
    let directory = datapack_dir(root);
    let overlay = DatapackOverlay::load_datapacks_dir(&directory)?;
    let summary = DatapackInstall {
        directory,
        packs: overlay.packs().len(),
        configured: overlay.configured_count(),
        placed: overlay.placed_count(),
        biomes: overlay.biome_count(),
        tags: overlay.tag_count(),
    };
    if overlay.is_empty() {
        return Ok(summary);
    }
    let merged = FeatureData::vanilla().merge(&overlay)?;
    FeatureData::install_active(merged)
        .map_err(|_| invalid("worldgen datapacks were already installed in this process"))?;
    let climates: Vec<_> = overlay
        .climates()
        .iter()
        .map(|(id, climate)| (id.clone(), *climate))
        .collect();
    if !climates.is_empty() {
        install_custom_biomes(climates).map_err(|_| {
            invalid("custom datapack biomes were already installed in this process")
        })?;
    }
    Ok(summary)
}

static CUSTOM_BIOMES: OnceLock<Vec<(String, CustomBiomeClimate)>> = OnceLock::new();

/// Installs custom climate entries once for the process.
pub fn install_custom_biomes(
    entries: Vec<(String, CustomBiomeClimate)>,
) -> Result<(), Vec<(String, CustomBiomeClimate)>> {
    CUSTOM_BIOMES.set(entries)
}

/// Custom climate entries installed for the process, if any.
pub fn installed_custom_biomes() -> &'static [(String, CustomBiomeClimate)] {
    CUSTOM_BIOMES.get().map_or(&[], Vec::as_slice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_components_reject_traversal_and_uppercase() {
        assert!(validate_component("testmod").is_ok());
        assert!(validate_component("a-b_c.1").is_ok());
        for bad in ["..", "Bad", "a/b", "a\\b", "", "a b"] {
            assert!(validate_component(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn biome_steps_accept_vanilla_and_normalized_shapes() {
        let vanilla = serde_json::json!({"features": {"steps": [["minecraft:lake_lava_surface"]]}});
        let steps = biome_steps(&vanilla, "testmod:b").unwrap();
        assert_eq!(steps.as_array().unwrap().len(), DECORATION_STEPS);
        assert_eq!(steps[0][0], "minecraft:lake_lava_surface");
        assert_eq!(steps[1].as_array().unwrap().len(), 0);
        assert!(
            biome_steps(
                &serde_json::json!({"steps": [[], [], [], [], [], [], [], [], [], [], [], []]}),
                "testmod:b"
            )
            .is_err()
        );
    }
}
