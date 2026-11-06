//! Vanilla 1.21.11 tag registries, flattened at build time.
//!
//! `scripts/extract_tags.ps1` resolves every nested `#tag` reference and emits
//! one sorted `tag<TAB>member` line per member into `assets/tags/*.tsv`. Empty
//! tags emit no line, so an empty tag and an unknown tag are both represented
//! by "no members"; the two empty block tags in 1.21.11
//! (`incorrect_for_diamond_tool` and `incorrect_for_netherite_tool`) are
//! therefore not counted by [`tag_count`].
//!
//! All keys in the assets are namespaced. Lookups accept either the fully
//! qualified `minecraft:logs` form or the bare `logs` form, and unknown keys
//! return an empty result instead of panicking.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;

const BLOCK: &str = include_str!("../assets/tags/block.tsv");
const ITEM: &str = include_str!("../assets/tags/item.tsv");
const ENTITY_TYPE: &str = include_str!("../assets/tags/entity_type.tsv");
const FLUID: &str = include_str!("../assets/tags/fluid.tsv");
const GAME_EVENT: &str = include_str!("../assets/tags/game_event.tsv");
const DAMAGE_TYPE: &str = include_str!("../assets/tags/damage_type.tsv");
const ENCHANTMENT: &str = include_str!("../assets/tags/enchantment.tsv");
const POINT_OF_INTEREST_TYPE: &str = include_str!("../assets/tags/point_of_interest_type.tsv");
const WORLDGEN: &str = include_str!("../assets/tags/worldgen.tsv");

/// A vanilla tag registry that ships flattened tables.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TagRegistryKind {
    Block,
    Item,
    EntityType,
    Fluid,
    GameEvent,
    DamageType,
    Enchantment,
    PointOfInterestType,
    Worldgen,
}

impl TagRegistryKind {
    const COUNT: usize = 9;

    /// The registry directory name used by the extractor and the Java data.
    pub fn name(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Item => "item",
            Self::EntityType => "entity_type",
            Self::Fluid => "fluid",
            Self::GameEvent => "game_event",
            Self::DamageType => "damage_type",
            Self::Enchantment => "enchantment",
            Self::PointOfInterestType => "point_of_interest_type",
            Self::Worldgen => "worldgen",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

const ASSETS: [&str; TagRegistryKind::COUNT] = [
    BLOCK,
    ITEM,
    ENTITY_TYPE,
    FLUID,
    GAME_EVENT,
    DAMAGE_TYPE,
    ENCHANTMENT,
    POINT_OF_INTEREST_TYPE,
    WORLDGEN,
];

struct TagBook {
    tags: HashMap<String, Vec<String>>,
    member_count: usize,
}

impl TagBook {
    fn parse(asset: &str) -> Self {
        let mut tags: HashMap<String, Vec<String>> = HashMap::new();
        let mut member_count = 0usize;
        for line in asset.lines() {
            let Some((tag, member)) = line.split_once('\t') else {
                continue;
            };
            if tag.is_empty() || member.is_empty() {
                continue;
            }
            tags.entry(tag.to_owned())
                .or_default()
                .push(member.to_owned());
            member_count += 1;
        }
        for members in tags.values() {
            debug_assert!(
                members.windows(2).all(|pair| pair[0] <= pair[1]),
                "tag asset members must be sorted"
            );
        }
        Self { tags, member_count }
    }
}

/// The embedded vanilla tag tables, loaded once on first use.
pub struct TagStore {
    registries: [TagBook; TagRegistryKind::COUNT],
}

static TAGS: OnceLock<TagStore> = OnceLock::new();

impl TagStore {
    /// Returns the process-wide tag store, loading every embedded table once.
    pub fn global() -> &'static Self {
        TAGS.get_or_init(|| Self {
            registries: std::array::from_fn(|index| TagBook::parse(ASSETS[index])),
        })
    }

    fn book(&self, registry: TagRegistryKind) -> &TagBook {
        &self.registries[registry.index()]
    }

    /// Returns whether `member` belongs to `tag` in `registry`.
    ///
    /// Both arguments accept a bare or namespaced key. Unknown tags and members
    /// return `false`.
    pub fn has(&self, registry: TagRegistryKind, tag: &str, member: &str) -> bool {
        let tag = normalize(tag);
        let member = normalize(member);
        self.book(registry)
            .tags
            .get(tag.as_ref())
            .is_some_and(|members| {
                members
                    .binary_search_by(|candidate| candidate.as_str().cmp(member.as_ref()))
                    .is_ok()
            })
    }

    /// Returns the flattened, sorted members of `tag` in `registry`.
    ///
    /// Unknown tags return an empty slice. Both bare and namespaced tag keys
    /// work.
    pub fn members(&self, registry: TagRegistryKind, tag: &str) -> &[String] {
        let tag = normalize(tag);
        self.book(registry)
            .tags
            .get(tag.as_ref())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Returns the number of non-empty tags loaded for `registry`.
    pub fn tag_count(&self, registry: TagRegistryKind) -> usize {
        self.book(registry).tags.len()
    }

    /// Returns the number of flattened tag/member rows loaded for `registry`.
    pub fn member_count(&self, registry: TagRegistryKind) -> usize {
        self.book(registry).member_count
    }
}

fn normalize(key: &str) -> Cow<'_, str> {
    if key.contains(':') {
        Cow::Borrowed(key)
    } else {
        Cow::Owned(format!("minecraft:{key}"))
    }
}

/// Returns whether `member` belongs to `tag` in `registry`.
///
/// Both arguments accept a bare or namespaced key. Unknown tags and members
/// return `false`.
pub fn has(registry: TagRegistryKind, tag: &str, member: &str) -> bool {
    TagStore::global().has(registry, tag, member)
}

/// Returns the flattened, sorted members of `tag` in `registry`.
///
/// Unknown tags return an empty slice. Both bare and namespaced tag keys work.
pub fn members(registry: TagRegistryKind, tag: &str) -> &'static [String] {
    TagStore::global().members(registry, tag)
}

/// Returns whether a block belongs to a block tag.
pub fn block_has(tag: &str, block: &str) -> bool {
    has(TagRegistryKind::Block, tag, block)
}

/// Returns whether an item belongs to an item tag.
pub fn item_has(tag: &str, item: &str) -> bool {
    has(TagRegistryKind::Item, tag, item)
}

/// Returns the number of non-empty tags loaded for `registry`.
pub fn tag_count(registry: TagRegistryKind) -> usize {
    TagStore::global().tag_count(registry)
}

/// Returns the number of flattened tag/member rows loaded for `registry`.
pub fn member_count(registry: TagRegistryKind) -> usize {
    TagStore::global().member_count(registry)
}
