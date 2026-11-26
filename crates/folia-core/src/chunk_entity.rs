//! Persistence model for the Anvil chunk `Entities` list.
//!
//! A chunk root stores its living entities in a top-level `Entities` list of
//! compounds. This port does not model every entity type's payload, so a
//! [`ChunkEntity`] keeps the fields it understands (`id`, `UUID`, `Pos`,
//! `Motion`, `Rotation`) as structured values and every other field verbatim in
//! [`payload`](ChunkEntity::payload). That means an entity type this build
//! cannot fully encode still survives a save/reload with its type-specific NBT
//! intact; a malformed core field is rejected instead of being silently
//! dropped.

use crate::nbt::Tag;
use std::io;

/// Modern Anvil key for a chunk's entity list.
pub const ENTITIES_KEY: &str = "Entities";
/// Lowercase key used by pre-1.17 region files, still accepted on read.
pub const LEGACY_ENTITIES_KEY: &str = "entities";

const CORE_FIELDS: [&str; 5] = ["id", "UUID", "Pos", "Motion", "Rotation"];

/// One entity stored in a chunk's `Entities` list.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkEntity {
    /// Resource location written to the `id` field, e.g. `minecraft:item`.
    pub id: String,
    /// 128-bit UUID in Java's big-endian byte order.
    pub uuid: [u8; 16],
    /// Absolute world position (`Pos`).
    pub position: [f64; 3],
    /// Yaw and pitch in degrees (`Rotation`).
    pub rotation: [f32; 2],
    /// Velocity vector (`Motion`).
    pub velocity: [f64; 3],
    /// Every field other than the five core fields, in its original order.
    /// Type-specific state (item stacks, mob armour, villagers, …) lives here
    /// so an entity type this port cannot encode is never silently discarded.
    pub payload: Vec<(String, Tag)>,
}

/// Entities belonging to one chunk, in vanilla save order.
pub type ChunkEntities = Vec<ChunkEntity>;

impl ChunkEntity {
    /// A bare entity with no type-specific payload. Callers that persist a type
    /// with extra state must put that state in [`payload`](Self::payload).
    pub fn new(id: impl Into<String>, uuid: [u8; 16], position: [f64; 3]) -> Self {
        Self {
            id: id.into(),
            uuid,
            position,
            rotation: [0.0; 2],
            velocity: [0.0; 3],
            payload: Vec::new(),
        }
    }

    /// Decode one `Entities` entry.
    pub fn from_nbt(tag: &Tag) -> io::Result<Self> {
        let fields = tag
            .as_compound()
            .ok_or_else(|| invalid("entity entry must be a compound tag"))?;
        let id = field(fields, "id")
            .and_then(Tag::as_str)
            .ok_or_else(|| invalid("entity is missing a string `id` field"))?
            .to_owned();
        let uuid = match field(fields, "UUID") {
            Some(tag) => decode_uuid(tag)?,
            None => [0u8; 16],
        };
        let position = match field(fields, "Pos") {
            Some(tag) => decode_vec3(tag, "Pos")?,
            None => [0.0; 3],
        };
        let velocity = match field(fields, "Motion") {
            Some(tag) => decode_vec3(tag, "Motion")?,
            None => [0.0; 3],
        };
        let rotation = match field(fields, "Rotation") {
            Some(tag) => decode_vec2(tag, "Rotation")?,
            None => [0.0; 2],
        };
        let payload = fields
            .iter()
            .filter(|(key, _)| !CORE_FIELDS.contains(&key.as_str()))
            .cloned()
            .collect();
        Ok(Self {
            id,
            uuid,
            position,
            rotation,
            velocity,
            payload,
        })
    }

    /// Encode this entity as an `Entities` entry. The five core fields are
    /// written first; the payload follows unchanged.
    pub fn to_nbt(&self) -> io::Result<Tag> {
        let uuid = uuid_words(&self.uuid).to_vec();
        let position = self.position.into_iter().map(Tag::Double).collect();
        let velocity = self.velocity.into_iter().map(Tag::Double).collect();
        let rotation = self.rotation.into_iter().map(Tag::Float).collect();
        let mut fields = Vec::with_capacity(self.payload.len() + CORE_FIELDS.len());
        fields.push(("id".into(), Tag::String(self.id.clone())));
        fields.push(("UUID".into(), Tag::IntArray(uuid)));
        fields.push(("Pos".into(), Tag::List(position)));
        fields.push(("Motion".into(), Tag::List(velocity)));
        fields.push(("Rotation".into(), Tag::List(rotation)));
        fields.extend(self.payload.iter().cloned());
        Ok(Tag::Compound(fields))
    }

    /// A non-core field carried by this entity, by name.
    pub fn payload_field(&self, name: &str) -> Option<&Tag> {
        self.payload
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value)
    }

    /// Insert or replace a non-core field.
    pub fn set_payload_field(&mut self, name: &str, value: Tag) {
        match self.payload.iter_mut().find(|(key, _)| key == name) {
            Some((_, slot)) => *slot = value,
            None => self.payload.push((name.to_owned(), value)),
        }
    }

    /// Chunk coordinates that contain this entity's position.
    pub fn chunk(&self) -> (i32, i32) {
        (
            self.position[0].div_euclid(16.0) as i32,
            self.position[2].div_euclid(16.0) as i32,
        )
    }
}

/// Decode a chunk root's `Entities` list. Both the modern and legacy keys are
/// read; the modern list wins for a shared UUID.
pub fn decode_entities(root: &Tag) -> io::Result<ChunkEntities> {
    let mut entities = Vec::new();
    if let Some(tag) = root.get(ENTITIES_KEY) {
        decode_entity_list(tag, &mut entities)?;
    }
    if let Some(tag) = root.get(LEGACY_ENTITIES_KEY) {
        decode_legacy_entity_list(tag, &mut entities)?;
    }
    Ok(entities)
}

fn decode_entity_list(tag: &Tag, entities: &mut ChunkEntities) -> io::Result<()> {
    let list = tag
        .as_list()
        .ok_or_else(|| invalid("chunk Entities must be a list tag"))?;
    for (index, entry) in list.iter().enumerate() {
        let entity = ChunkEntity::from_nbt(entry)
            .map_err(|error| invalid(format!("Entities[{index}]: {error}")))?;
        entities.push(entity);
    }
    Ok(())
}

fn decode_legacy_entity_list(tag: &Tag, entities: &mut ChunkEntities) -> io::Result<()> {
    let list = tag
        .as_list()
        .ok_or_else(|| invalid("chunk entities must be a list tag"))?;
    for (index, entry) in list.iter().enumerate() {
        let entity = ChunkEntity::from_nbt(entry)
            .map_err(|error| invalid(format!("entities[{index}]: {error}")))?;
        if !entities.iter().any(|existing| existing.uuid == entity.uuid) {
            entities.push(entity);
        }
    }
    Ok(())
}

/// Encode entities as a vanilla `Entities` list tag.
pub fn encode_entities(entities: &ChunkEntities) -> io::Result<Tag> {
    entities
        .iter()
        .map(ChunkEntity::to_nbt)
        .collect::<io::Result<Vec<_>>>()
        .map(Tag::List)
}

/// Install an `Entities` list on a chunk root, removing the legacy key.
pub fn store_entities(root: &mut Tag, entities: &ChunkEntities) -> io::Result<()> {
    let Tag::Compound(fields) = root else {
        return Err(invalid("chunk root must be a compound tag"));
    };
    fields.retain(|(key, _)| key != ENTITIES_KEY && key != LEGACY_ENTITIES_KEY);
    fields.push((ENTITIES_KEY.into(), encode_entities(entities)?));
    Ok(())
}

fn field<'a>(fields: &'a [(String, Tag)], name: &str) -> Option<&'a Tag> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

fn decode_uuid(tag: &Tag) -> io::Result<[u8; 16]> {
    let words = tag
        .as_int_array()
        .ok_or_else(|| invalid("entity `UUID` must be an int array"))?;
    if words.len() != 4 {
        return Err(invalid(format!(
            "entity `UUID` must contain exactly 4 words, got {}",
            words.len()
        )));
    }
    let mut uuid = [0u8; 16];
    for (index, word) in words.iter().enumerate() {
        uuid[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    Ok(uuid)
}

fn uuid_words(uuid: &[u8; 16]) -> [i32; 4] {
    let mut words = [0i32; 4];
    for (index, word) in words.iter_mut().enumerate() {
        let offset = index * 4;
        *word = i32::from_be_bytes([
            uuid[offset],
            uuid[offset + 1],
            uuid[offset + 2],
            uuid[offset + 3],
        ]);
    }
    words
}

fn decode_vec3(tag: &Tag, name: &str) -> io::Result<[f64; 3]> {
    let values = tag
        .as_list()
        .ok_or_else(|| invalid(format!("entity `{name}` must be a list tag")))?;
    if values.len() != 3 {
        return Err(invalid(format!(
            "entity `{name}` must contain exactly 3 numbers, got {}",
            values.len()
        )));
    }
    let mut out = [0.0; 3];
    for (index, value) in values.iter().enumerate() {
        out[index] = value
            .as_double()
            .ok_or_else(|| invalid(format!("entity `{name}[{index}]` must be a double")))?;
    }
    Ok(out)
}

fn decode_vec2(tag: &Tag, name: &str) -> io::Result<[f32; 2]> {
    let values = tag
        .as_list()
        .ok_or_else(|| invalid(format!("entity `{name}` must be a list tag")))?;
    if values.len() != 2 {
        return Err(invalid(format!(
            "entity `{name}` must contain exactly 2 numbers, got {}",
            values.len()
        )));
    }
    let mut out = [0.0; 2];
    for (index, value) in values.iter().enumerate() {
        out[index] = value
            .as_float()
            .ok_or_else(|| invalid(format!("entity `{name}[{index}]` must be a float")))?;
    }
    Ok(out)
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vanilla_entity() -> Tag {
        crate::snbt::parse(
            r#"{
                id: "minecraft:item",
                UUID: [I; 305419896, -1, 123456, -2147483648],
                Pos: [1.5d, 64.25d, -3.75d],
                Motion: [0.1d, -0.2d, 0.3d],
                Rotation: [45.5f, -12.25f],
                OnGround: 1b,
                Age: 600s,
                Item: { id: "minecraft:stone", Count: 3b }
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn round_trip_keeps_core_fields_and_type_payload() {
        let original = vanilla_entity();
        let entity = ChunkEntity::from_nbt(&original).unwrap();
        assert_eq!(entity.id, "minecraft:item");
        assert_eq!(entity.position, [1.5, 64.25, -3.75]);
        assert_eq!(entity.velocity, [0.1, -0.2, 0.3]);
        assert_eq!(entity.rotation, [45.5, -12.25]);
        assert_eq!(entity.uuid[0..4], [0x12, 0x34, 0x56, 0x78]);
        assert_eq!(entity.uuid[12..16], [0x80, 0x00, 0x00, 0x00]);

        let encoded = entity.to_nbt().unwrap();
        assert_eq!(encoded.get("id").unwrap().as_str(), Some("minecraft:item"));
        assert_eq!(
            encoded.get("UUID").unwrap().as_int_array().unwrap().len(),
            4
        );
        assert_eq!(encoded.get("Pos").unwrap(), original.get("Pos").unwrap());
        assert_eq!(
            encoded.get("Motion").unwrap(),
            original.get("Motion").unwrap()
        );
        assert_eq!(
            encoded.get("Rotation").unwrap(),
            original.get("Rotation").unwrap()
        );
        assert_eq!(encoded.get("OnGround"), original.get("OnGround"));
        assert_eq!(encoded.get("Age"), original.get("Age"));
        assert_eq!(encoded.get("Item"), original.get("Item"));

        let again = ChunkEntity::from_nbt(&encoded).unwrap();
        assert_eq!(again, entity);
    }

    #[test]
    fn decode_rejects_malformed_core_fields() {
        let missing_id = crate::snbt::parse("{Pos: [0.0d, 0.0d, 0.0d]}").unwrap();
        assert!(ChunkEntity::from_nbt(&missing_id).is_err());

        let bad_uuid = crate::snbt::parse(r#"{id: "x", UUID: [I; 1, 2]}"#).unwrap();
        assert!(ChunkEntity::from_nbt(&bad_uuid).is_err());

        let bad_pos = crate::snbt::parse(r#"{id: "x", Pos: [1.0d, 2.0d]}"#).unwrap();
        assert!(ChunkEntity::from_nbt(&bad_pos).is_err());

        let bad_motion = crate::snbt::parse(r#"{id: "x", Motion: [1, 2, 3]}"#).unwrap();
        assert!(ChunkEntity::from_nbt(&bad_motion).is_err());

        let bad_rotation = crate::snbt::parse(r#"{id: "x", Rotation: [1.0f]}"#).unwrap();
        assert!(ChunkEntity::from_nbt(&bad_rotation).is_err());
    }

    #[test]
    fn unknown_entity_type_is_preserved_without_a_registry_entry() {
        let tag = crate::snbt::parse(
            r#"{
                id: "somemod:unlisted_thing",
                UUID: [I; 1, 2, 3, 4],
                Pos: [0.0d, 1.0d, 2.0d],
                Motion: [0.0d, 0.0d, 0.0d],
                Rotation: [0.0f, 0.0f],
                mod_value: { nested: [1, 2, 3] }
            }"#,
        )
        .unwrap();
        let entity = ChunkEntity::from_nbt(&tag).unwrap();
        let encoded = entity.to_nbt().unwrap();
        assert_eq!(
            encoded.get("mod_value"),
            tag.get("mod_value"),
            "type-specific payload survives even for an unknown id"
        );
        assert!(
            encoded
                .get("id")
                .unwrap()
                .as_str()
                .unwrap()
                .contains("unlisted")
        );
    }

    #[test]
    fn entity_list_decode_rejects_a_non_list() {
        let root = crate::snbt::parse("{Entities: 3}").unwrap();
        assert!(decode_entities(&root).is_err());
    }

    #[test]
    fn chunk_key_reports_floor_divided_coordinates() {
        let entity = ChunkEntity::new("minecraft:cow", [0; 16], [-1.0, 70.0, -17.0]);
        assert_eq!(entity.chunk(), (-1, -2));
    }
}
