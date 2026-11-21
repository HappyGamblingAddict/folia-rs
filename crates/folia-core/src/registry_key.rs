//! Typed registry keys and id/name maps.
//!
//! A [`RegistryKey`] carries a numeric id and a namespaced name together with
//! a zero-sized marker `M` that identifies the registry it belongs to. The
//! marker makes keys from different registries different Rust types, so a menu
//! key can never be passed where a block key is expected. [`IdMap`] stores a
//! payload indexed by both id and name and keeps the two indexes consistent.

use std::collections::HashMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

/// An id/name pair tagged with the registry it belongs to.
///
/// `T` is only a compile-time marker; no value of it is ever stored. Because
/// the marker is a distinct type per registry, `RegistryKey<MenuType>` and
/// `RegistryKey<BlockType>` cannot be mixed up by the compiler.
pub struct RegistryKey<T> {
    id: i32,
    name: String,
    marker: PhantomData<fn() -> T>,
}

impl<T> Clone for RegistryKey<T> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            name: self.name.clone(),
            marker: PhantomData,
        }
    }
}

impl<T> fmt::Debug for RegistryKey<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegistryKey")
            .field("id", &self.id)
            .field("name", &self.name)
            .finish()
    }
}

impl<T> PartialEq for RegistryKey<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.name == other.name
    }
}

impl<T> Eq for RegistryKey<T> {}

impl<T> Hash for RegistryKey<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.name.hash(state);
    }
}

impl<T> RegistryKey<T> {
    /// Builds a key for `id` and `name`. No validation happens here; the
    /// [`IdMap`] insert path is what rejects invalid or conflicting pairs.
    pub fn new(id: i32, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            marker: PhantomData,
        }
    }

    /// Returns the numeric id of this key.
    pub fn id(self) -> i32 {
        self.id
    }

    /// Returns the namespaced name of this key.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Errors produced while inserting into an [`IdMap`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryKeyError {
    /// An id below zero was supplied; registry ids are non-negative.
    NegativeId(i32),
    /// An id already maps to a different name.
    DuplicateId {
        id: i32,
        existing_name: String,
        new_name: String,
    },
    /// A name already maps to a different id.
    DuplicateName {
        name: String,
        existing_id: i32,
        new_id: i32,
    },
}

impl fmt::Display for RegistryKeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NegativeId(id) => write!(f, "registry id must be non-negative, got {id}"),
            Self::DuplicateId {
                id,
                existing_name,
                new_name,
            } => write!(
                f,
                "registry id {id} is already registered as \"{existing_name}\", cannot also register \"{new_name}\""
            ),
            Self::DuplicateName {
                name,
                existing_id,
                new_id,
            } => write!(
                f,
                "registry name \"{name}\" is already registered with id {existing_id}, cannot also use id {new_id}"
            ),
        }
    }
}

impl std::error::Error for RegistryKeyError {}

/// A payload stored under both a numeric id and a namespaced name.
///
/// Ids must be non-negative. The map keeps ids and names in a one-to-one
/// relationship: inserting an id with a different name, or a name with a
/// different id, fails with a [`RegistryKeyError`] and leaves the map
/// untouched. Re-inserting the same id/name pair succeeds.
pub struct IdMap<T: Clone> {
    by_id: Vec<Option<T>>,
    by_name: HashMap<String, T>,
    len: usize,
}

impl<T: Clone> Default for IdMap<T> {
    fn default() -> Self {
        Self {
            by_id: Vec::new(),
            by_name: HashMap::new(),
            len: 0,
        }
    }
}

impl<T: Clone> IdMap<T> {
    /// Creates an empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Looks up a payload by its numeric id. Negative and out-of-range ids
    /// return `None`.
    pub fn by_id(&self, id: i32) -> Option<&T> {
        if id < 0 {
            return None;
        }
        self.by_id.get(id as usize).and_then(Option::as_ref)
    }

    /// Looks up a payload by its namespaced name.
    pub fn by_name(&self, name: &str) -> Option<&T> {
        self.by_name.get(name)
    }

    /// Returns the number of inserted entries.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` when no entries have been inserted.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Inserts `key` under `id` and `name`.
    ///
    /// `T: PartialEq` is used internally to recover the name associated with
    /// an occupied id (and vice versa) for the duplicate errors.
    pub fn insert(
        &mut self,
        key: T,
        id: i32,
        name: impl Into<String>,
    ) -> Result<(), RegistryKeyError>
    where
        T: PartialEq,
    {
        if id < 0 {
            return Err(RegistryKeyError::NegativeId(id));
        }
        let name = name.into();
        let index = id as usize;

        let existing_name = self
            .by_id
            .get(index)
            .and_then(Option::as_ref)
            .and_then(|existing| {
                self.by_name
                    .iter()
                    .find_map(|(name, value)| (value == existing).then(|| name.clone()))
            });

        if let Some(existing_name) = existing_name {
            if existing_name != name {
                return Err(RegistryKeyError::DuplicateId {
                    id,
                    existing_name,
                    new_name: name,
                });
            }
            self.by_id[index] = Some(key.clone());
            self.by_name.insert(name, key);
            return Ok(());
        }

        if let Some(existing_value) = self.by_name.get(&name) {
            let existing_id = self
                .by_id
                .iter()
                .position(|slot| slot.as_ref() == Some(existing_value))
                .expect("IdMap invariant: every name has a matching id slot")
                as i32;
            return Err(RegistryKeyError::DuplicateName {
                name,
                existing_id,
                new_id: id,
            });
        }

        if index >= self.by_id.len() {
            self.by_id.resize(index + 1, None);
        }
        self.by_id[index] = Some(key.clone());
        self.by_name.insert(name, key);
        self.len += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MenuType;
    type MenuTypeKey = RegistryKey<MenuType>;

    struct BlockType;
    type BlockTypeKey = RegistryKey<BlockType>;

    fn accept_menu_key(key: MenuTypeKey) -> (i32, String) {
        let name = key.name().to_owned();
        (key.id(), name)
    }

    fn accept_block_key(key: BlockTypeKey) -> (i32, String) {
        let name = key.name().to_owned();
        (key.id(), name)
    }

    #[test]
    fn typed_keys_keep_registries_distinct() {
        let menu = MenuTypeKey::new(14, "minecraft:chest");
        assert_eq!(
            accept_menu_key(menu.clone()),
            (14, "minecraft:chest".to_owned())
        );

        let block = BlockTypeKey::new(14, "minecraft:stone");
        assert_eq!(
            accept_block_key(block.clone()),
            (14, "minecraft:stone".to_owned())
        );

        assert_eq!(menu, menu.clone());
        assert_eq!(block, block.clone());
    }

    #[test]
    fn id_map_roundtrips_by_id_and_name() {
        let mut map: IdMap<MenuTypeKey> = IdMap::new();
        assert!(map.is_empty());

        let chest = MenuTypeKey::new(14, "minecraft:chest");
        let hopper = MenuTypeKey::new(22, "minecraft:hopper");
        map.insert(chest.clone(), 14, "minecraft:chest").unwrap();
        map.insert(hopper.clone(), 22, "minecraft:hopper").unwrap();

        assert_eq!(map.len(), 2);
        assert!(!map.is_empty());
        assert_eq!(map.by_id(14), Some(&chest));
        assert_eq!(map.by_name("minecraft:chest"), Some(&chest));
        assert_eq!(map.by_id(22), Some(&hopper));
        assert_eq!(map.by_name("minecraft:hopper"), Some(&hopper));
        assert_eq!(map.by_id(23), None);
        assert_eq!(map.by_name("minecraft:unknown"), None);

        assert_eq!(map.insert(chest, 14, "minecraft:chest"), Ok(()));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn id_map_rejects_negative_duplicate_id_and_duplicate_name() {
        let mut map: IdMap<MenuTypeKey> = IdMap::new();

        assert_eq!(
            map.insert(MenuTypeKey::new(-1, "minecraft:bad"), -1, "minecraft:bad"),
            Err(RegistryKeyError::NegativeId(-1))
        );
        assert!(map.is_empty());
        assert_eq!(map.by_id(-1), None);

        map.insert(MenuTypeKey::new(1, "minecraft:one"), 1, "minecraft:one")
            .unwrap();

        assert_eq!(
            map.insert(MenuTypeKey::new(1, "minecraft:two"), 1, "minecraft:two"),
            Err(RegistryKeyError::DuplicateId {
                id: 1,
                existing_name: "minecraft:one".to_owned(),
                new_name: "minecraft:two".to_owned(),
            })
        );

        assert_eq!(
            map.insert(MenuTypeKey::new(2, "minecraft:one"), 2, "minecraft:one"),
            Err(RegistryKeyError::DuplicateName {
                name: "minecraft:one".to_owned(),
                existing_id: 1,
                new_id: 2,
            })
        );

        assert_eq!(map.len(), 1);
        assert_eq!(map.by_id(2), None);
        assert_eq!(map.by_name("minecraft:two"), None);
    }

    #[test]
    fn id_map_handles_sparse_ids() {
        let mut map: IdMap<String> = IdMap::new();

        map.insert("zero".to_owned(), 0, "zero").unwrap();
        map.insert("five".to_owned(), 5, "five").unwrap();
        map.insert("hundred".to_owned(), 100, "hundred").unwrap();

        assert_eq!(map.len(), 3);
        assert_eq!(map.by_id(0).map(String::as_str), Some("zero"));
        assert_eq!(map.by_id(1), None);
        assert_eq!(map.by_id(4), None);
        assert_eq!(map.by_id(5).map(String::as_str), Some("five"));
        assert_eq!(map.by_id(99), None);
        assert_eq!(map.by_id(100).map(String::as_str), Some("hundred"));
        assert_eq!(map.by_id(101), None);
        assert_eq!(map.by_id(i32::MAX), None);
        assert_eq!(map.by_name("five").map(String::as_str), Some("five"));
    }
}
