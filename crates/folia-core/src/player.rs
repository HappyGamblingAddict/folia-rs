use crate::crypto::{self, RsaKeyPair, RsaPublicKey};
use crate::inventory::{HOTBAR_SLOTS, INVENTORY_SLOTS, ItemStack, default_inventory};
use crate::item_components::{item_stack_from_nbt, item_stack_to_nbt};
use crate::nbt::{self, Tag};
use crate::player_physics::{
    ClientMove, MovementCorrection, PhysicsWorld, PlayerPhysics, Reconciliation,
};
use crate::protocol::{
    GameProfile, LastSeenMessages, MESSAGE_SIGNATURE_BYTES, MessageSignature, PacketReader,
    ProfileProperty, SignedMessageBody,
};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Playerdata fields this server parses and rewrites. Everything else in the
/// file is kept verbatim in [`PlayerSession::extra`] so a save never drops
/// fields it does not model (health, food, XP, ender chest, ...).
pub const KNOWN_PLAYERDATA_FIELDS: [&str; 20] = [
    "DataVersion",
    "Name",
    "UUID",
    "Pos",
    "Rotation",
    "OnGround",
    "Dimension",
    "SelectedItemSlot",
    "Inventory",
    "Cursor",
    "Health",
    "foodLevel",
    "foodSaturationLevel",
    "foodExhaustionLevel",
    "XpLevel",
    "XpP",
    "XpTotal",
    "Score",
    "Air",
    "playerGameType",
];

pub const SERVERBOUND_MOVE_POS: i32 = 29;
pub const SERVERBOUND_MOVE_POS_ROT: i32 = 30;
pub const SERVERBOUND_MOVE_ROT: i32 = 31;
pub const SERVERBOUND_MOVE_STATUS: i32 = 32;

#[derive(Debug, Clone, Copy, PartialEq)]
enum MovementUpdate {
    Position {
        x: f64,
        y: f64,
        z: f64,
        yaw: Option<f32>,
        pitch: Option<f32>,
        on_ground: bool,
    },
    Rotation {
        yaw: f32,
        pitch: f32,
        on_ground: bool,
    },
    Status {
        on_ground: bool,
    },
}

fn parse_movement(packet: &[u8]) -> io::Result<Option<MovementUpdate>> {
    let mut reader = PacketReader::new(packet);
    let id = reader.read_varint()?;
    let update = match id {
        SERVERBOUND_MOVE_POS => MovementUpdate::Position {
            x: reader.read_f64()?,
            y: reader.read_f64()?,
            z: reader.read_f64()?,
            yaw: None,
            pitch: None,
            on_ground: reader.read_u8()? & 1 != 0,
        },
        SERVERBOUND_MOVE_POS_ROT => MovementUpdate::Position {
            x: reader.read_f64()?,
            y: reader.read_f64()?,
            z: reader.read_f64()?,
            yaw: Some(reader.read_f32()?),
            pitch: Some(reader.read_f32()?),
            on_ground: reader.read_u8()? & 1 != 0,
        },
        SERVERBOUND_MOVE_ROT => MovementUpdate::Rotation {
            yaw: reader.read_f32()?,
            pitch: reader.read_f32()?,
            on_ground: reader.read_u8()? & 1 != 0,
        },
        SERVERBOUND_MOVE_STATUS => MovementUpdate::Status {
            on_ground: reader.read_u8()? & 1 != 0,
        },
        _ => return Ok(None),
    };
    Ok(Some(update))
}

// ===========================================================================
// Chat signing / profile keys
// ===========================================================================

/// `PlayerChatMessage.MESSAGE_EXPIRES_AFTER_SERVER`.
pub const CHAT_MESSAGE_EXPIRES_AFTER_SERVER_MS: i64 = 5 * 60 * 1000;
/// Tolerated client clock drift for a message timestamp in the future.
pub const CHAT_MAX_FUTURE_SKEW_MS: i64 = 2 * 60 * 1000;
/// Default profile-key lifetime. Vanilla clients refresh before this horizon;
/// the server-issued key here is only used for its own local signing path.
pub const PROFILE_KEY_DEFAULT_LIFETIME_MS: i64 = 48 * 60 * 60 * 1000;
/// GameProfile property that carries the server-issued profile public key.
pub const PROFILE_KEY_PROPERTY: &str = "profile_key";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatSigningError {
    /// A requested enforcement path needs a secure key, but only the explicit
    /// deterministic test key (or a too-small RSA key) is available.
    InsecureAlgorithm(String),
    MissingSignature,
    InvalidSignature,
    InvalidSession,
    SequenceOutOfBounds {
        expected: i32,
        received: i32,
    },
    TimestampOutOfOrder {
        last_ms: i64,
        received_ms: i64,
    },
    TimestampExpired {
        timestamp_ms: i64,
        now_ms: i64,
    },
    TimestampInFuture {
        timestamp_ms: i64,
        now_ms: i64,
    },
    ExpiredProfileKey,
    InvalidProfileKey(String),
    InvalidMessage(String),
    KeyGeneration(String),
}

impl fmt::Display for ChatSigningError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InsecureAlgorithm(message) => {
                write!(formatter, "secure chat algorithm unavailable: {message}")
            }
            Self::MissingSignature => {
                formatter.write_str("chat message has no profile-key signature")
            }
            Self::InvalidSignature => formatter.write_str("chat message signature is invalid"),
            Self::InvalidSession => {
                formatter.write_str("chat message belongs to a different sender/session")
            }
            Self::SequenceOutOfBounds { expected, received } => write!(
                formatter,
                "chat message sequence {received} does not match expected {expected}"
            ),
            Self::TimestampOutOfOrder {
                last_ms,
                received_ms,
            } => write!(
                formatter,
                "chat message timestamp {received_ms} precedes last accepted {last_ms}"
            ),
            Self::TimestampExpired {
                timestamp_ms,
                now_ms,
            } => write!(
                formatter,
                "chat message timestamp {timestamp_ms} expired before {now_ms}"
            ),
            Self::TimestampInFuture {
                timestamp_ms,
                now_ms,
            } => write!(
                formatter,
                "chat message timestamp {timestamp_ms} is too far ahead of {now_ms}"
            ),
            Self::ExpiredProfileKey => formatter.write_str("profile key has expired"),
            Self::InvalidProfileKey(message) => write!(formatter, "invalid profile key: {message}"),
            Self::InvalidMessage(message) => write!(formatter, "invalid chat message: {message}"),
            Self::KeyGeneration(message) => {
                write!(formatter, "profile key generation failed: {message}")
            }
        }
    }
}

impl std::error::Error for ChatSigningError {}

/// Public half of a chat profile key. The RSA variant is a DER
/// SubjectPublicKeyInfo (`SHA256withRSA`); the deterministic variant exists
/// only for tests and is never selected by the production provisioning path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatPublicKey {
    Rsa(Vec<u8>),
    /// NOT SECURE. Deterministic test verifier; `enforced` verification rejects
    /// it with [`ChatSigningError::InsecureAlgorithm`].
    Deterministic([u8; 32]),
}

impl ChatPublicKey {
    /// A secure key is a 2048-bit RSA key: 1.21.11 fixes
    /// `MessageSignature.BYTES` at 256, and a smaller modulus cannot produce a
    /// wire-compatible signature.
    pub fn is_secure(&self) -> bool {
        match self {
            Self::Rsa(der) => RsaPublicKey::from_der(der)
                .is_ok_and(|key| key.modulus_bytes() == MESSAGE_SIGNATURE_BYTES),
            Self::Deterministic(_) => false,
        }
    }

    pub fn verify(&self, payload: &[u8], signature: &[u8]) -> bool {
        match self {
            Self::Rsa(der) => match RsaPublicKey::from_der(der) {
                Ok(key) => key.verify_sha256(payload, signature),
                Err(_) => false,
            },
            Self::Deterministic(seed) => {
                crypto::constant_time_eq(&deterministic_signature(seed, payload), signature)
            }
        }
    }

    fn encoded(&self) -> Vec<u8> {
        match self {
            Self::Rsa(der) => der.clone(),
            Self::Deterministic(seed) => seed.to_vec(),
        }
    }

    fn algorithm(&self) -> &'static str {
        match self {
            Self::Rsa(_) => "rsa",
            Self::Deterministic(_) => "insecure-deterministic",
        }
    }

    fn from_algorithm(algorithm: &str, bytes: &[u8]) -> Result<Self, ChatSigningError> {
        match algorithm {
            "rsa" => {
                RsaPublicKey::from_der(bytes)
                    .map_err(|error| ChatSigningError::InvalidProfileKey(error.to_string()))?;
                Ok(Self::Rsa(bytes.to_vec()))
            }
            "insecure-deterministic" => {
                let seed: [u8; 32] = bytes.try_into().map_err(|_| {
                    ChatSigningError::InvalidProfileKey(
                        "deterministic test key must be 32 bytes".into(),
                    )
                })?;
                Ok(Self::Deterministic(seed))
            }
            other => Err(ChatSigningError::InvalidProfileKey(format!(
                "unknown profile-key algorithm {other}"
            ))),
        }
    }
}

/// Explicitly insecure deterministic signer used by tests and offline capture.
/// Production provisioning goes through [`ProfileKeyPair::generate_secure`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeterministicTestKey {
    pub seed: [u8; 32],
}

impl DeterministicTestKey {
    pub fn new(seed: [u8; 32]) -> Self {
        Self { seed }
    }

    pub fn public_key(&self) -> ChatPublicKey {
        ChatPublicKey::Deterministic(self.seed)
    }

    pub fn sign(&self, payload: &[u8]) -> Vec<u8> {
        deterministic_signature(&self.seed, payload)
    }
}

fn deterministic_signature(seed: &[u8; 32], payload: &[u8]) -> Vec<u8> {
    let mut signature = Vec::with_capacity(MESSAGE_SIGNATURE_BYTES);
    let mut counter = 0u8;
    while signature.len() < MESSAGE_SIGNATURE_BYTES {
        let mut hasher = crypto::Sha256::new();
        hasher.update(b"Folia-RS insecure deterministic chat test signer");
        hasher.update(seed);
        hasher.update(&[counter]);
        hasher.update(payload);
        signature.extend_from_slice(&hasher.finish());
        counter = counter.wrapping_add(1);
    }
    signature.truncate(MESSAGE_SIGNATURE_BYTES);
    signature
}

/// Private half of a chat profile key.
#[derive(Debug, Clone)]
pub enum ChatSigningKey {
    Rsa(RsaKeyPair),
    Deterministic(DeterministicTestKey),
}

impl ChatSigningKey {
    pub fn is_secure(&self) -> bool {
        self.public_key().is_secure()
    }

    pub fn public_key(&self) -> ChatPublicKey {
        match self {
            Self::Rsa(key) => ChatPublicKey::Rsa(key.public_key_der()),
            Self::Deterministic(key) => key.public_key(),
        }
    }

    /// Sign a profile-key record. The key-signature field is a length-prefixed
    /// byte array (up to 4096 bytes), not the fixed 256-byte chat signature.
    pub fn sign_raw(&self, payload: &[u8]) -> Result<Vec<u8>, ChatSigningError> {
        match self {
            Self::Rsa(key) => key
                .sign_sha256(payload)
                .map_err(|_| ChatSigningError::InvalidSignature),
            Self::Deterministic(key) => Ok(key.sign(payload)),
        }
    }

    /// Sign a chat message. 1.21.11 requires exactly 256 bytes, so a too-small
    /// RSA key must fail instead of producing a non-wire-compatible signature.
    pub fn sign(&self, payload: &[u8]) -> Result<Vec<u8>, ChatSigningError> {
        let signature = self.sign_raw(payload)?;
        if signature.len() != MESSAGE_SIGNATURE_BYTES {
            return Err(ChatSigningError::InsecureAlgorithm(format!(
                "RSA key produces {} byte signatures; 1.21.11 requires {} byte signatures \
                 (2048-bit RSA)",
                signature.len(),
                MESSAGE_SIGNATURE_BYTES
            )));
        }
        Ok(signature)
    }
}

/// A signed public key record as sent in the Login Success GameProfile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfilePublicKey {
    pub expires_at_ms: i64,
    pub key: ChatPublicKey,
    pub key_signature: Vec<u8>,
}

impl ProfilePublicKey {
    pub fn is_expired(&self, now_ms: i64) -> bool {
        now_ms > self.expires_at_ms
    }

    /// `ProfilePublicKey.Data.signedPayload`: UUID (big-endian), epoch millis,
    /// then the encoded public key.
    pub fn signed_payload(&self, profile_id: &[u8; 16]) -> Vec<u8> {
        let key = self.key.encoded();
        let mut payload = Vec::with_capacity(24 + key.len());
        payload.extend_from_slice(profile_id);
        payload.extend_from_slice(&self.expires_at_ms.to_be_bytes());
        payload.extend_from_slice(&key);
        payload
    }

    /// Validate that a trusted issuer signed this key for `profile_id`.
    pub fn validate(
        &self,
        profile_id: &[u8; 16],
        issuer: &ChatPublicKey,
        now_ms: i64,
    ) -> Result<(), ChatSigningError> {
        if self.is_expired(now_ms) {
            return Err(ChatSigningError::ExpiredProfileKey);
        }
        if !issuer.verify(&self.signed_payload(profile_id), &self.key_signature) {
            return Err(ChatSigningError::InvalidProfileKey(
                "key signature does not match the issuer".into(),
            ));
        }
        Ok(())
    }

    pub fn validator(&self) -> ChatPublicKey {
        self.key.clone()
    }

    /// Encode into the string carried by the `profile_key` GameProfile
    /// property: `v1:<algorithm>:<expires_ms>:<key_hex>:<signature_hex>`.
    pub fn to_property_value(&self) -> String {
        format!(
            "v1:{}:{}:{}:{}",
            self.key.algorithm(),
            self.expires_at_ms,
            encode_hex(&self.key.encoded()),
            encode_hex(&self.key_signature)
        )
    }

    pub fn from_property_value(value: &str) -> Result<Self, ChatSigningError> {
        if value.len() > 16 * 1024 {
            return Err(ChatSigningError::InvalidProfileKey(
                "profile-key property is too large".into(),
            ));
        }
        let parts: Vec<&str> = value.split(':').collect();
        if parts.len() != 5 || parts[0] != "v1" {
            return Err(ChatSigningError::InvalidProfileKey(
                "expected v1:<algorithm>:<expires_ms>:<key>:<signature>".into(),
            ));
        }
        let expires_at_ms = parts[2].parse().map_err(|_| {
            ChatSigningError::InvalidProfileKey("profile-key expiry is not an integer".into())
        })?;
        let key = ChatPublicKey::from_algorithm(parts[1], &decode_hex(parts[3])?)?;
        let key_signature = decode_hex(parts[4])?;
        Ok(Self {
            expires_at_ms,
            key,
            key_signature,
        })
    }
}

/// A complete profile key pair. `generate_secure` is the production path and
/// uses a fresh 2048-bit RSA key per login; the deterministic constructor is
/// explicitly marked as test-only.
#[derive(Debug, Clone)]
pub struct ProfileKeyPair {
    private: ChatSigningKey,
    public: ProfilePublicKey,
}

impl ProfileKeyPair {
    /// Wrap an existing private key, primarily for key recovery/persistence.
    /// The caller is responsible for [`ChatSigningKey::is_secure`] when
    /// enforcement is enabled.
    pub fn from_signing_key(
        private: ChatSigningKey,
        profile_id: &[u8; 16],
        now_ms: i64,
        lifetime_ms: i64,
    ) -> Result<Self, ChatSigningError> {
        Self::self_sign(private, profile_id, now_ms, lifetime_ms)
    }

    pub fn generate_secure(
        profile_id: &[u8; 16],
        now_ms: i64,
        lifetime_ms: i64,
    ) -> Result<Self, ChatSigningError> {
        let private = ChatSigningKey::Rsa(
            RsaKeyPair::generate_with_bits(crypto::PROFILE_KEY_BITS)
                .map_err(|error| ChatSigningError::KeyGeneration(error.to_string()))?,
        );
        Self::self_sign(private, profile_id, now_ms, lifetime_ms)
    }

    pub fn deterministic_for_tests(
        profile_id: &[u8; 16],
        seed: [u8; 32],
        now_ms: i64,
        lifetime_ms: i64,
    ) -> Result<Self, ChatSigningError> {
        Self::self_sign(
            ChatSigningKey::Deterministic(DeterministicTestKey::new(seed)),
            profile_id,
            now_ms,
            lifetime_ms,
        )
    }

    fn self_sign(
        private: ChatSigningKey,
        profile_id: &[u8; 16],
        now_ms: i64,
        lifetime_ms: i64,
    ) -> Result<Self, ChatSigningError> {
        if lifetime_ms <= 0 {
            return Err(ChatSigningError::InvalidProfileKey(
                "profile-key lifetime must be positive".into(),
            ));
        }
        let mut public = ProfilePublicKey {
            expires_at_ms: now_ms.saturating_add(lifetime_ms),
            key: private.public_key(),
            key_signature: Vec::new(),
        };
        public.key_signature = private.sign_raw(&public.signed_payload(profile_id))?;
        Ok(Self { private, public })
    }

    pub fn sign(&self, payload: &[u8]) -> Result<Vec<u8>, ChatSigningError> {
        self.private.sign(payload)
    }

    pub fn public(&self) -> &ProfilePublicKey {
        &self.public
    }

    pub fn is_secure(&self) -> bool {
        self.private.is_secure()
    }

    pub fn property(&self) -> ProfileProperty {
        ProfileProperty::new(PROFILE_KEY_PROPERTY, self.public.to_property_value())
    }

    /// The exact GameProfile the login path must send in Login Success.
    pub fn login_profile(&self, uuid: [u8; 16], name: impl Into<String>) -> GameProfile {
        let mut profile = GameProfile::new(uuid, name);
        profile.properties.push(self.property());
        profile
    }
}

/// One signed player chat message. `sequence` is Java's `SignedMessageLink.index`
/// and `content_hash` is a local SHA-256 convenience/integrity field: it is not
/// part of the vanilla wire format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedMessage {
    pub sequence: i32,
    pub sender: [u8; 16],
    pub session_id: [u8; 16],
    pub body: SignedMessageBody,
    pub signature: Option<MessageSignature>,
    pub content_hash: [u8; crypto::SHA256_DIGEST_SIZE],
}

impl SignedMessage {
    pub fn new(
        sequence: i32,
        sender: [u8; 16],
        session_id: [u8; 16],
        body: SignedMessageBody,
    ) -> Self {
        let content_hash = body.content_hash();
        Self {
            sequence,
            sender,
            session_id,
            body,
            signature: None,
            content_hash,
        }
    }

    pub fn unsigned(
        sequence: i32,
        sender: [u8; 16],
        session_id: [u8; 16],
        body: SignedMessageBody,
    ) -> Self {
        Self::new(sequence, sender, session_id, body)
    }

    /// `PlayerChatMessage.updateSignature`: protocol version marker `1`, link,
    /// then body. The timestamp is signed as epoch seconds.
    pub fn signature_payload(&self) -> Vec<u8> {
        let mut payload = Vec::with_capacity(128 + self.body.content.len());
        payload.extend_from_slice(&1i32.to_be_bytes());
        payload.extend_from_slice(&self.sender);
        payload.extend_from_slice(&self.session_id);
        payload.extend_from_slice(&self.sequence.to_be_bytes());
        payload.extend_from_slice(&self.body.salt.to_be_bytes());
        payload.extend_from_slice(&self.body.timestamp_ms.div_euclid(1000).to_be_bytes());
        payload.extend_from_slice(&(self.body.content.len() as i32).to_be_bytes());
        payload.extend_from_slice(self.body.content.as_bytes());
        payload.extend_from_slice(&(self.body.last_seen.len() as i32).to_be_bytes());
        for signature in self.body.last_seen.entries() {
            payload.extend_from_slice(signature.as_bytes());
        }
        payload
    }

    pub fn verify(&self, key: &ChatPublicKey) -> bool {
        if self.content_hash != self.body.content_hash() {
            return false;
        }
        let Some(signature) = &self.signature else {
            return false;
        };
        key.verify(&self.signature_payload(), signature.as_bytes())
    }

    pub fn has_signature(&self) -> bool {
        self.signature.is_some()
    }
}

/// Local signing chain: one profile key, one session id, and monotonically
/// increasing sequence/timestamp state.
#[derive(Debug, Clone)]
pub struct ChatSigningSession {
    sender: [u8; 16],
    session_id: [u8; 16],
    key_pair: ProfileKeyPair,
    next_sequence: i32,
    last_timestamp_ms: i64,
    last_seen: LastSeenMessages,
}

impl PartialEq for ChatSigningSession {
    fn eq(&self, other: &Self) -> bool {
        self.sender == other.sender
            && self.session_id == other.session_id
            && self.key_pair.public == other.key_pair.public
            && self.next_sequence == other.next_sequence
            && self.last_timestamp_ms == other.last_timestamp_ms
            && self.last_seen == other.last_seen
    }
}

impl ChatSigningSession {
    /// Production provisioning: fresh 2048-bit RSA profile key and random
    /// session id per login.
    pub fn issue(profile_id: [u8; 16], now_ms: i64) -> Result<Self, ChatSigningError> {
        let key_pair =
            ProfileKeyPair::generate_secure(&profile_id, now_ms, PROFILE_KEY_DEFAULT_LIFETIME_MS)?;
        let mut session_id = [0u8; 16];
        crypto::random_bytes(&mut session_id)
            .map_err(|error| ChatSigningError::KeyGeneration(error.to_string()))?;
        Self::from_key_pair(profile_id, session_id, key_pair, now_ms)
    }

    pub fn deterministic_for_tests(
        profile_id: [u8; 16],
        session_id: [u8; 16],
        seed: [u8; 32],
        now_ms: i64,
    ) -> Result<Self, ChatSigningError> {
        let key_pair = ProfileKeyPair::deterministic_for_tests(
            &profile_id,
            seed,
            now_ms,
            PROFILE_KEY_DEFAULT_LIFETIME_MS,
        )?;
        Self::from_key_pair(profile_id, session_id, key_pair, now_ms)
    }

    pub fn from_key_pair(
        sender: [u8; 16],
        session_id: [u8; 16],
        key_pair: ProfileKeyPair,
        now_ms: i64,
    ) -> Result<Self, ChatSigningError> {
        if key_pair.public.is_expired(now_ms) {
            return Err(ChatSigningError::ExpiredProfileKey);
        }
        Ok(Self {
            sender,
            session_id,
            key_pair,
            next_sequence: 0,
            last_timestamp_ms: i64::MIN,
            last_seen: LastSeenMessages::empty(),
        })
    }

    pub fn sender(&self) -> &[u8; 16] {
        &self.sender
    }

    pub fn session_id(&self) -> &[u8; 16] {
        &self.session_id
    }

    pub fn public_key(&self) -> &ProfilePublicKey {
        self.key_pair.public()
    }

    pub fn profile_property(&self) -> ProfileProperty {
        self.key_pair.property()
    }

    pub fn login_profile(&self, name: impl Into<String>) -> GameProfile {
        self.key_pair.login_profile(self.sender, name)
    }

    pub fn is_secure(&self) -> bool {
        self.key_pair.is_secure()
    }

    pub fn last_seen(&self) -> &LastSeenMessages {
        &self.last_seen
    }

    /// Record an acknowledged signature for inclusion in the next body.
    pub fn acknowledge(&mut self, signature: MessageSignature) -> io::Result<()> {
        self.last_seen.push(signature)
    }

    pub fn sign(
        &mut self,
        content: impl Into<String>,
        salt: i64,
        now_ms: i64,
    ) -> Result<SignedMessage, ChatSigningError> {
        if self.next_sequence < 0 || self.next_sequence == i32::MAX {
            return Err(ChatSigningError::InvalidMessage(
                "chat message sequence is exhausted".into(),
            ));
        }
        if now_ms < self.last_timestamp_ms {
            return Err(ChatSigningError::TimestampOutOfOrder {
                last_ms: self.last_timestamp_ms,
                received_ms: now_ms,
            });
        }
        let body = SignedMessageBody::new(content, now_ms, salt, self.last_seen.clone())
            .map_err(|error| ChatSigningError::InvalidMessage(error.to_string()))?;
        let mut message =
            SignedMessage::new(self.next_sequence, self.sender, self.session_id, body);
        let signature = self.key_pair.sign(&message.signature_payload())?;
        message.signature = Some(
            MessageSignature::from_slice(&signature)
                .map_err(|_| ChatSigningError::InvalidSignature)?,
        );
        self.next_sequence += 1;
        self.last_timestamp_ms = now_ms;
        Ok(message)
    }
}

/// Remote chat chain: validates a peer message against the profile public key
/// announced in its chat-session update.
#[derive(Debug, Clone)]
pub struct RemoteChatSession {
    sender: [u8; 16],
    session_id: [u8; 16],
    public_key: ProfilePublicKey,
    next_sequence: i32,
    last_timestamp_ms: i64,
}

impl RemoteChatSession {
    /// `issuer` is the trusted key that signed the profile key (the server's
    /// own public key for server-issued keys). Passing `None` skips the key
    /// signature check, matching an offline-mode/unsigned deployment.
    pub fn new(
        sender: [u8; 16],
        session_id: [u8; 16],
        public_key: ProfilePublicKey,
        now_ms: i64,
        enforced: bool,
        issuer: Option<&ChatPublicKey>,
    ) -> Result<Self, ChatSigningError> {
        if public_key.is_expired(now_ms) {
            return Err(ChatSigningError::ExpiredProfileKey);
        }
        if enforced && !public_key.key.is_secure() {
            return Err(ChatSigningError::InsecureAlgorithm(
                "enforces-secure-chat is enabled but the remote profile key is not RSA".into(),
            ));
        }
        if let Some(issuer) = issuer {
            public_key.validate(&sender, issuer, now_ms)?;
        }
        Ok(Self {
            sender,
            session_id,
            public_key,
            next_sequence: 0,
            last_timestamp_ms: i64::MIN,
        })
    }

    pub fn sender(&self) -> &[u8; 16] {
        &self.sender
    }

    pub fn session_id(&self) -> &[u8; 16] {
        &self.session_id
    }

    pub fn public_key(&self) -> &ProfilePublicKey {
        &self.public_key
    }

    pub fn next_sequence(&self) -> i32 {
        self.next_sequence
    }

    pub fn verify(&mut self, message: &SignedMessage, now_ms: i64) -> Result<(), ChatSigningError> {
        if message.sender != self.sender || message.session_id != self.session_id {
            return Err(ChatSigningError::InvalidSession);
        }
        if self.next_sequence == i32::MAX {
            return Err(ChatSigningError::InvalidMessage(
                "chat message sequence is exhausted".into(),
            ));
        }
        if message.sequence != self.next_sequence {
            return Err(ChatSigningError::SequenceOutOfBounds {
                expected: self.next_sequence,
                received: message.sequence,
            });
        }
        if self.public_key.is_expired(now_ms) {
            return Err(ChatSigningError::ExpiredProfileKey);
        }
        if message.body.timestamp_ms < self.last_timestamp_ms {
            return Err(ChatSigningError::TimestampOutOfOrder {
                last_ms: self.last_timestamp_ms,
                received_ms: message.body.timestamp_ms,
            });
        }
        if now_ms.saturating_sub(message.body.timestamp_ms) > CHAT_MESSAGE_EXPIRES_AFTER_SERVER_MS {
            return Err(ChatSigningError::TimestampExpired {
                timestamp_ms: message.body.timestamp_ms,
                now_ms,
            });
        }
        if message.body.timestamp_ms.saturating_sub(now_ms) > CHAT_MAX_FUTURE_SKEW_MS {
            return Err(ChatSigningError::TimestampInFuture {
                timestamp_ms: message.body.timestamp_ms,
                now_ms,
            });
        }
        if message.signature.is_none() {
            return Err(ChatSigningError::MissingSignature);
        }
        if !message.verify(&self.public_key.key) {
            return Err(ChatSigningError::InvalidSignature);
        }
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.last_timestamp_ms = message.body.timestamp_ms;
        Ok(())
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

fn decode_hex(value: &str) -> Result<Vec<u8>, ChatSigningError> {
    if !value.len().is_multiple_of(2) {
        return Err(ChatSigningError::InvalidProfileKey(
            "hex profile-key field has odd length".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(value.len() / 2);
    let raw = value.as_bytes();
    for pair in raw.chunks_exact(2) {
        let high = hex_digit(pair[0])?;
        let low = hex_digit(pair[1])?;
        bytes.push((high << 4) | low);
    }
    Ok(bytes)
}

fn hex_digit(byte: u8) -> Result<u8, ChatSigningError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(ChatSigningError::InvalidProfileKey(format!(
            "invalid hex digit {:?}",
            char::from(byte)
        ))),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlayerSession {
    pub name: String,
    pub uuid: [u8; 16],
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    /// Vanilla game mode id (0 survival, 1 creative, 2 adventure, 3 spectator).
    /// `-1` means the file had no `playerGameType`; the join path fills in the
    /// server default before the session is used.
    pub game_mode: i32,
    pub selected_slot: i32,
    pub inventory: Vec<ItemStack>,
    pub cursor: ItemStack,
    /// Current health points; vanilla's survival default is 20.
    pub health: f32,
    /// `foodLevel`, 0-20.
    pub food_level: i32,
    /// `foodSaturationLevel`.
    pub food_saturation: f32,
    /// `foodExhaustionLevel`.
    pub food_exhaustion: f32,
    /// `XpLevel`.
    pub xp_level: i32,
    /// `XpP`, progress toward the next level (0.0-1.0).
    pub xp_progress: f32,
    /// `XpTotal`.
    pub xp_total: i32,
    /// `Score` (the death/scoreboard counter stored in playerdata).
    pub score: i32,
    /// `Air`, remaining breath ticks; vanilla's default is 300.
    pub air: i16,
    /// Server-authoritative movement state. `x`/`y`/`z` mirror its position.
    pub physics: PlayerPhysics,
    /// Per-login chat signing chain and server-issued profile key. Not written
    /// to playerdata: a fresh key is provisioned for every login.
    pub chat_signing: Option<ChatSigningSession>,
    /// Playerdata fields this server does not model, preserved across saves.
    pub extra: Vec<(String, Tag)>,
}

/// Result of applying one movement packet.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovementOutcome {
    pub was_movement: bool,
    pub position_changed: bool,
    pub correction: Option<MovementCorrection>,
    pub fall_damage: f32,
}

impl MovementOutcome {
    fn not_movement() -> Self {
        Self {
            was_movement: false,
            position_changed: false,
            correction: None,
            fall_damage: 0.0,
        }
    }

    fn from_reconciliation(reconciliation: Reconciliation) -> Self {
        Self {
            was_movement: true,
            position_changed: reconciliation.moved,
            correction: reconciliation.correction,
            fall_damage: reconciliation.fall_damage,
        }
    }
}

impl PlayerSession {
    pub fn new(name: String, uuid: [u8; 16], x: f64, y: f64, z: f64) -> Self {
        Self {
            name,
            uuid,
            x,
            y,
            z,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            game_mode: 0,
            selected_slot: 0,
            inventory: default_inventory(),
            cursor: ItemStack::empty(),
            health: 20.0,
            food_level: 20,
            food_saturation: 5.0,
            food_exhaustion: 0.0,
            xp_level: 0,
            xp_progress: 0.0,
            xp_total: 0,
            score: 0,
            air: 300,
            physics: PlayerPhysics::new(x, y, z),
            chat_signing: None,
            extra: Vec::new(),
        }
    }

    /// Issue the server-issued chat profile key for this login. `join.rs` must
    /// call this before Login Success and send [`Self::login_profile`] so the
    /// public key reaches the client; until then secure chat stays disabled.
    pub fn provision_chat_signing(&mut self, now_ms: i64) -> Result<(), ChatSigningError> {
        self.chat_signing = Some(ChatSigningSession::issue(self.uuid, now_ms)?);
        Ok(())
    }

    /// Deterministic test-only key provisioning (256-byte signatures, no RSA).
    pub fn provision_test_chat_signing(
        &mut self,
        session_id: [u8; 16],
        seed: [u8; 32],
        now_ms: i64,
    ) -> Result<(), ChatSigningError> {
        self.chat_signing = Some(ChatSigningSession::deterministic_for_tests(
            self.uuid, session_id, seed, now_ms,
        )?);
        Ok(())
    }

    /// Login Success profile, including the `profile_key` property when a chat
    /// key has been provisioned.
    pub fn login_profile(&self) -> GameProfile {
        match &self.chat_signing {
            Some(chat) => chat.login_profile(self.name.clone()),
            None => GameProfile::new(self.uuid, self.name.clone()),
        }
    }

    /// Rebuild the simulation state from the session's position. Used after
    /// login settling, teleports, respawn, and playerdata loads so the first
    /// movement packet is measured from the accepted position.
    pub fn reset_physics(&mut self) {
        self.physics.teleport(self.x, self.y, self.z);
        self.physics.yaw = self.yaw;
        self.physics.pitch = self.pitch;
        self.physics.on_ground = self.on_ground;
    }

    /// Move the session and its simulation together, discarding velocity and
    /// fall distance like a vanilla teleport.
    pub fn teleport(&mut self, x: f64, y: f64, z: f64, on_ground: bool) {
        self.x = x;
        self.y = y;
        self.z = z;
        self.on_ground = on_ground;
        self.physics.teleport(x, y, z);
        self.physics.on_ground = on_ground;
    }

    pub fn selected_item(&self) -> Option<&ItemStack> {
        self.inventory
            .get(self.selected_slot.max(0) as usize)
            .filter(|stack| !stack.is_empty())
    }

    pub fn consume_selected(&mut self) -> bool {
        let Some(stack) = self.inventory.get_mut(self.selected_slot.max(0) as usize) else {
            return false;
        };
        if stack.is_empty() {
            return false;
        }
        stack.shrink(1);
        true
    }

    /// Parse a serverbound play movement packet and reconcile its claimed
    /// position with the simulated player. Returns whether the packet was a
    /// movement packet, whether the simulated position changed, and any
    /// correction the caller must send back to the client.
    pub fn apply_movement(
        &mut self,
        packet: &[u8],
        world: &mut impl PhysicsWorld,
    ) -> io::Result<MovementOutcome> {
        self.apply_movement_with_allowance(
            packet,
            world,
            crate::player_physics::MAX_MOVEMENT_DISTANCE_SQUARED,
        )
    }

    /// [`Self::apply_movement`] with a caller-provided catch-up distance
    /// budget. The play loop raises it when the server has not read a movement
    /// packet for a while (for example while the initial chunk batch streams).
    pub fn apply_movement_with_allowance(
        &mut self,
        packet: &[u8],
        world: &mut impl PhysicsWorld,
        max_distance_squared: f64,
    ) -> io::Result<MovementOutcome> {
        let Some(update) = parse_movement(packet)? else {
            return Ok(MovementOutcome::not_movement());
        };
        match update {
            MovementUpdate::Position {
                x,
                y,
                z,
                yaw,
                pitch,
                on_ground,
            } => {
                let client = ClientMove {
                    x,
                    y,
                    z,
                    on_ground,
                    yaw,
                    pitch,
                };
                let reconciliation =
                    self.physics
                        .reconcile_with_allowance(world, &client, max_distance_squared);
                self.x = self.physics.x;
                self.y = self.physics.y;
                self.z = self.physics.z;
                self.yaw = self.physics.yaw;
                self.pitch = self.physics.pitch;
                self.on_ground = reconciliation.on_ground;
                Ok(MovementOutcome::from_reconciliation(reconciliation))
            }
            MovementUpdate::Rotation {
                yaw,
                pitch,
                on_ground,
            } => {
                self.yaw = yaw;
                self.pitch = pitch;
                self.on_ground = on_ground;
                self.physics.yaw = yaw;
                self.physics.pitch = pitch;
                self.physics.on_ground = on_ground;
                Ok(MovementOutcome {
                    was_movement: true,
                    position_changed: false,
                    correction: None,
                    fall_damage: 0.0,
                })
            }
            MovementUpdate::Status { on_ground } => {
                self.on_ground = on_ground;
                self.physics.on_ground = on_ground;
                Ok(MovementOutcome {
                    was_movement: true,
                    position_changed: false,
                    correction: None,
                    fall_damage: 0.0,
                })
            }
        }
    }

    pub fn save(&self, directory: impl AsRef<Path>) -> io::Result<PathBuf> {
        let directory = directory.as_ref();
        fs::create_dir_all(directory)?;
        let path = Self::path(directory, &self.uuid);
        let inventory_entries = self
            .inventory
            .iter()
            .enumerate()
            .filter(|(_, stack)| !stack.is_empty())
            .map(|(slot, stack)| {
                let mut fields = vec![("Slot".into(), Tag::Int(slot as i32))];
                if let Tag::Compound(inner) = item_stack_to_nbt(stack) {
                    fields.extend(inner);
                }
                Tag::Compound(fields)
            })
            .collect();
        let cursor = item_stack_to_nbt(&self.cursor);

        let mut entries = vec![
            ("DataVersion".into(), Tag::Int(crate::level::DATA_VERSION)),
            ("playerGameType".into(), Tag::Int(self.game_mode.max(0))),
            ("Health".into(), Tag::Float(self.health)),
            ("foodLevel".into(), Tag::Int(self.food_level)),
            (
                "foodSaturationLevel".into(),
                Tag::Float(self.food_saturation),
            ),
            (
                "foodExhaustionLevel".into(),
                Tag::Float(self.food_exhaustion),
            ),
            ("XpLevel".into(), Tag::Int(self.xp_level)),
            ("XpP".into(), Tag::Float(self.xp_progress)),
            ("XpTotal".into(), Tag::Int(self.xp_total)),
            ("Score".into(), Tag::Int(self.score)),
            ("Air".into(), Tag::Short(self.air)),
            ("Name".into(), Tag::String(self.name.clone())),
            ("UUID".into(), Tag::String(uuid_string(&self.uuid))),
            (
                "Pos".into(),
                Tag::List(vec![
                    Tag::Double(self.x),
                    Tag::Double(self.y),
                    Tag::Double(self.z),
                ]),
            ),
            (
                "Rotation".into(),
                Tag::List(vec![Tag::Float(self.yaw), Tag::Float(self.pitch)]),
            ),
            ("OnGround".into(), Tag::Byte(self.on_ground as i8)),
            (
                "Dimension".into(),
                Tag::String("minecraft:overworld".into()),
            ),
            ("SelectedItemSlot".into(), Tag::Int(self.selected_slot)),
            ("Inventory".into(), Tag::List(inventory_entries)),
            ("Cursor".into(), cursor),
        ];
        entries.extend(
            self.extra
                .iter()
                .filter(|(key, _)| !KNOWN_PLAYERDATA_FIELDS.contains(&key.as_str()))
                .cloned(),
        );
        let tag = Tag::Compound(entries);
        let bytes = nbt::write_gzip_root(&tag)?;
        let temporary = path.with_extension("tmp");
        fs::write(&temporary, bytes)?;
        fs::rename(temporary, &path)?;
        Ok(path)
    }

    pub fn load(directory: impl AsRef<Path>, uuid: [u8; 16]) -> io::Result<Option<Self>> {
        let path = Self::path(directory.as_ref(), &uuid);
        if !path.exists() {
            return Ok(None);
        }
        let bytes = fs::read(path)?;
        let root = nbt::read_gzip_root(&bytes)?;

        let name = root
            .get("Name")
            .and_then(Tag::as_str)
            .unwrap_or("Player")
            .to_owned();
        let position = root
            .get("Pos")
            .and_then(Tag::as_list)
            .ok_or_else(|| invalid("playerdata is missing Pos"))?;
        let rotation = root
            .get("Rotation")
            .and_then(Tag::as_list)
            .ok_or_else(|| invalid("playerdata is missing Rotation"))?;

        let coordinate = |index: usize| {
            position
                .get(index)
                .and_then(Tag::as_double)
                .ok_or_else(|| invalid("playerdata has invalid Pos"))
        };
        let angle = |index: usize| {
            rotation
                .get(index)
                .and_then(Tag::as_float)
                .ok_or_else(|| invalid("playerdata has invalid Rotation"))
        };

        let selected_slot = root
            .get("SelectedItemSlot")
            .and_then(Tag::as_int)
            .unwrap_or(0)
            .clamp(0, (HOTBAR_SLOTS - 1) as i32);
        let mut inventory = default_inventory();
        if let Some(entries) = root.get("Inventory").and_then(Tag::as_list) {
            inventory = vec![ItemStack::empty(); INVENTORY_SLOTS];
            for entry in entries {
                let Some(slot) = entry.get("Slot").and_then(Tag::as_int) else {
                    continue;
                };
                if !(0..INVENTORY_SLOTS as i32).contains(&slot) {
                    continue;
                }
                let stack = item_stack_from_nbt(entry)?;
                if !stack.is_empty() {
                    inventory[slot as usize] = stack;
                }
            }
        }
        let cursor = root
            .get("Cursor")
            .map(item_stack_from_nbt)
            .transpose()?
            .unwrap_or_else(ItemStack::empty);

        let health = root.get("Health").and_then(Tag::as_float).unwrap_or(20.0);
        let food_level = root.get("foodLevel").and_then(Tag::as_int).unwrap_or(20);
        let food_saturation = root
            .get("foodSaturationLevel")
            .and_then(Tag::as_float)
            .unwrap_or(5.0);
        let food_exhaustion = root
            .get("foodExhaustionLevel")
            .and_then(Tag::as_float)
            .unwrap_or(0.0);
        let xp_level = root.get("XpLevel").and_then(Tag::as_int).unwrap_or(0);
        let xp_progress = root.get("XpP").and_then(Tag::as_float).unwrap_or(0.0);
        let xp_total = root.get("XpTotal").and_then(Tag::as_int).unwrap_or(0);
        let score = root.get("Score").and_then(Tag::as_int).unwrap_or(0);
        let air = root
            .get("Air")
            .and_then(|tag| match tag {
                Tag::Short(value) => Some(*value),
                Tag::Int(value) => i16::try_from(*value).ok(),
                _ => None,
            })
            .unwrap_or(300);
        let game_mode = root
            .get("playerGameType")
            .and_then(Tag::as_int)
            .unwrap_or(-1)
            .clamp(-1, 3);
        let x = coordinate(0)?;
        let y = coordinate(1)?;
        let z = coordinate(2)?;
        let yaw = angle(0)?;
        let pitch = angle(1)?;
        let on_ground = root
            .get("OnGround")
            .and_then(Tag::as_byte)
            .is_some_and(|value| value != 0);
        let mut physics = PlayerPhysics::new(x, y, z);
        physics.yaw = yaw;
        physics.pitch = pitch;
        physics.on_ground = on_ground;

        let extra = root
            .as_compound()
            .map(|entries| {
                entries
                    .iter()
                    .filter(|(key, _)| !KNOWN_PLAYERDATA_FIELDS.contains(&key.as_str()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        Ok(Some(Self {
            name,
            uuid,
            x,
            y,
            z,
            yaw,
            pitch,
            on_ground,
            game_mode,
            selected_slot,
            inventory,
            cursor,
            health,
            food_level,
            food_saturation,
            food_exhaustion,
            xp_level,
            xp_progress,
            xp_total,
            score,
            air,
            physics,
            chat_signing: None,
            extra,
        }))
    }

    pub fn path(directory: &Path, uuid: &[u8; 16]) -> PathBuf {
        directory.join(format!("{}.dat", uuid_string(uuid)))
    }
}

pub fn uuid_string(uuid: &[u8; 16]) -> String {
    let hex: String = uuid.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// Result of one hunger tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoodTick {
    Nothing,
    Heal,
    Starve,
}

/// Result of one oxygen tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AirTick {
    Nothing,
    Consumed,
    Refilled,
    Drowning,
}

impl PlayerSession {
    /// Adds exhaustion, then burns saturation and food in whole 4-point units.
    pub fn add_exhaustion(&mut self, amount: f32) {
        self.food_exhaustion += amount.max(0.0);
        self.burn_exhaustion();
    }

    fn burn_exhaustion(&mut self) {
        while self.food_exhaustion >= 4.0 {
            self.food_exhaustion -= 4.0;
            if self.food_saturation > 0.0 {
                self.food_saturation = (self.food_saturation - 1.0).max(0.0);
            } else {
                self.food_level = (self.food_level - 1).max(0);
            }
        }
    }

    /// `FoodData.eat`: nutrition raises food and saturation, capped by the new
    /// food level.
    pub fn eat(&mut self, nutrition: i32, saturation_modifier: f32) {
        self.food_level = (self.food_level + nutrition.max(0)).clamp(0, 20);
        self.food_saturation = (self.food_saturation
            + nutrition as f32 * saturation_modifier.max(0.0) * 2.0)
            .min(self.food_level as f32);
    }

    /// Vanilla natural regeneration and starvation subset. `tick` is the game
    /// tick counter; player food ticks at 10/80-tick intervals.
    pub fn tick_food(&mut self, tick: u64) -> FoodTick {
        if self.food_level >= 18 {
            if self.food_saturation > 0.0 && tick.is_multiple_of(10) {
                self.health = (self.health + 1.0).min(20.0);
                self.add_exhaustion(6.0);
                return FoodTick::Heal;
            }
            if self.food_saturation <= 0.0 && tick.is_multiple_of(80) {
                self.health = (self.health + 1.0).min(20.0);
                self.add_exhaustion(6.0);
                return FoodTick::Heal;
            }
        } else if self.food_level == 0 && tick.is_multiple_of(80) {
            self.health = (self.health - 1.0).max(0.0);
            return FoodTick::Starve;
        }
        FoodTick::Nothing
    }

    /// `Player.getXpNeededForNextLevel`: points required to go from `level` to
    /// `level + 1`.
    pub fn xp_needed_for_next_level(level: i32) -> i32 {
        if level >= 30 {
            112 + (level - 30) * 9
        } else if level >= 15 {
            37 + (level - 15) * 5
        } else {
            7 + level * 2
        }
    }

    /// Total experience points required to reach `level` from zero.
    pub fn total_xp_for_level(level: i32) -> i32 {
        let mut total = 0;
        for current in 0..level.max(0) {
            total += Self::xp_needed_for_next_level(current);
        }
        total
    }

    /// Adds experience points, carrying progress across level-ups. Negative
    /// amounts are ignored; the level is capped like vanilla's practical limit.
    pub fn add_experience(&mut self, amount: i32) {
        if amount <= 0 {
            return;
        }
        self.xp_total = self.xp_total.saturating_add(amount);
        let mut remaining =
            self.xp_progress * Self::xp_needed_for_next_level(self.xp_level) as f32 + amount as f32;
        while self.xp_level < 21_863 {
            let needed = Self::xp_needed_for_next_level(self.xp_level) as f32;
            if remaining < needed {
                break;
            }
            remaining -= needed;
            self.xp_level += 1;
        }
        let needed = Self::xp_needed_for_next_level(self.xp_level) as f32;
        self.xp_progress = (remaining / needed).clamp(0.0, 0.999_999);
    }

    /// One oxygen tick. Submerged players lose one air per tick and take two
    /// damage every 20 ticks once the bar has drained; air refills by four per
    /// tick out of water. Returns what happened for packet/tests.
    pub fn tick_air(&mut self, submerged: bool) -> AirTick {
        if submerged {
            self.air = (self.air - 1).max(-20);
            if self.air == -20 {
                self.air = 0;
                self.health = (self.health - 2.0).max(0.0);
                return AirTick::Drowning;
            }
            AirTick::Consumed
        } else if self.air < 300 {
            self.air = (self.air + 4).min(300);
            AirTick::Refilled
        } else {
            AirTick::Nothing
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PacketWriter;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "folia-rs-player-{}-{}-{name}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn uuid_formatting_is_hyphenated() {
        let mut uuid = [0u8; 16];
        uuid[0] = 0x12;
        uuid[15] = 0xef;
        assert_eq!(uuid_string(&uuid), "12000000-0000-0000-0000-0000000000ef");
    }

    #[test]
    fn saves_and_loads_player_position() {
        let directory = temp_dir("save");
        let mut session = PlayerSession::new("Steve".into(), [0x11; 16], 12.5, 64.0, -3.25);
        session.yaw = 90.0;
        session.pitch = -15.0;
        session.on_ground = true;
        session.physics.on_ground = true;
        session.physics.yaw = 90.0;
        session.physics.pitch = -15.0;
        session.selected_slot = 2;
        session.inventory[0] = ItemStack::new("minecraft:dirt", 3);
        session.cursor = ItemStack::new("minecraft:stone", 2);
        session.health = 13.5;
        session.food_level = 11;
        session.food_saturation = 2.25;
        session.food_exhaustion = 1.5;
        session.xp_level = 7;
        session.xp_progress = 0.75;
        session.xp_total = 140;
        session.score = 9;
        session.air = 120;
        session.game_mode = 1;
        let path = session.save(&directory).unwrap();
        assert!(path.exists());
        let saved = nbt::read_gzip_root(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            saved.get("DataVersion").and_then(Tag::as_int),
            Some(crate::level::DATA_VERSION),
            "playerdata carries the current data version"
        );

        let loaded = PlayerSession::load(&directory, [0x11; 16])
            .unwrap()
            .expect("playerdata");
        assert_eq!(loaded, session);
        assert_eq!(loaded.game_mode, 1, "playerGameType round-trips");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn selected_item_consumes_one_per_placement() {
        let mut session = PlayerSession::new("Alex".into(), [0x44; 16], 0.0, 0.0, 0.0);
        session.selected_slot = 1;
        assert_eq!(session.selected_item().unwrap().count, 64);
        assert!(session.consume_selected());
        assert_eq!(session.selected_item().unwrap().count, 63);
        for _ in 0..63 {
            assert!(session.consume_selected());
        }
        assert!(session.selected_item().is_none());
        assert!(!session.consume_selected());
    }

    #[test]
    fn preserves_unknown_playerdata_fields() {
        let directory = temp_dir("extra");
        fs::create_dir_all(&directory).unwrap();
        let uuid = [0x66; 16];
        let root = Tag::Compound(vec![
            (
                "Pos".into(),
                Tag::List(vec![Tag::Double(1.5), Tag::Double(70.0), Tag::Double(-2.5)]),
            ),
            (
                "Rotation".into(),
                Tag::List(vec![Tag::Float(10.0), Tag::Float(0.0)]),
            ),
            ("Health".into(), Tag::Float(17.5)),
            ("foodLevel".into(), Tag::Int(12)),
            (
                "EnderItems".into(),
                Tag::List(vec![Tag::Compound(vec![(
                    "id".into(),
                    Tag::String("minecraft:stone".into()),
                )])]),
            ),
        ]);
        fs::write(
            PlayerSession::path(&directory, &uuid),
            nbt::write_gzip_root(&root).unwrap(),
        )
        .unwrap();

        let mut session = PlayerSession::load(&directory, uuid)
            .unwrap()
            .expect("playerdata");
        assert_eq!(session.extra.len(), 1, "only unknown fields are kept");
        assert_eq!(session.health, 17.5, "Health is parsed into the session");
        assert_eq!(
            session.food_level, 12,
            "foodLevel is parsed into the session"
        );
        assert_eq!(
            session.game_mode, -1,
            "a missing playerGameType leaves the server default to the join path"
        );
        session.y = 71.0;
        session.save(&directory).unwrap();

        let reloaded =
            nbt::read_gzip_root(&fs::read(PlayerSession::path(&directory, &uuid)).unwrap())
                .unwrap();
        for key in ["Health", "foodLevel", "EnderItems", "UUID"] {
            assert!(reloaded.get(key).is_some(), "{key} survived the save");
        }
        let position = reloaded.get("Pos").and_then(Tag::as_list).unwrap();
        assert_eq!(position[1].as_double(), Some(71.0));

        fs::remove_dir_all(directory).ok();
    }

    #[test]
    fn new_playerdata_starts_with_survival_defaults() {
        let directory = temp_dir("defaults");
        let session = PlayerSession::new("New".into(), [0x12; 16], 0.0, 64.0, 0.0);
        let path = session.save(&directory).unwrap();
        let root = nbt::read_gzip_root(&fs::read(path).unwrap()).unwrap();

        assert_eq!(root.get("Health").and_then(Tag::as_float), Some(20.0));
        assert_eq!(root.get("foodLevel").and_then(Tag::as_int), Some(20));
        assert_eq!(
            root.get("foodSaturationLevel").and_then(Tag::as_float),
            Some(5.0)
        );
        assert_eq!(
            root.get("Air").and_then(|tag| match tag {
                Tag::Short(value) => Some(*value),
                _ => None,
            }),
            Some(300)
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn missing_playerdata_returns_none() {
        let directory = temp_dir("missing");
        assert!(
            PlayerSession::load(&directory, [0x22; 16])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn parses_all_movement_packets() {
        use crate::player_physics::FlatPhysicsWorld;

        let mut session = PlayerSession::new("Alex".into(), [0x33; 16], 0.0, 0.0, 0.0);
        let mut world = FlatPhysicsWorld::new(-100);

        let mut writer = PacketWriter::new();
        writer.write_varint(SERVERBOUND_MOVE_POS).unwrap();
        writer.write_f64(1.0).unwrap();
        writer.write_f64(2.0).unwrap();
        writer.write_f64(3.0).unwrap();
        writer.write_u8(1).unwrap();
        let outcome = session
            .apply_movement(&writer.into_inner(), &mut world)
            .unwrap();
        assert!(outcome.was_movement);
        assert_eq!((session.x, session.y, session.z), (1.0, 2.0, 3.0));
        assert!(session.on_ground);

        let mut writer = PacketWriter::new();
        writer.write_varint(SERVERBOUND_MOVE_POS_ROT).unwrap();
        writer.write_f64(4.0).unwrap();
        writer.write_f64(5.0).unwrap();
        writer.write_f64(6.0).unwrap();
        writer.write_f32(45.0).unwrap();
        writer.write_f32(-10.0).unwrap();
        writer.write_u8(0).unwrap();
        let outcome = session
            .apply_movement(&writer.into_inner(), &mut world)
            .unwrap();
        assert!(outcome.was_movement);
        assert_eq!((session.yaw, session.pitch), (45.0, -10.0));
        assert!(!session.on_ground);

        let mut writer = PacketWriter::new();
        writer.write_varint(SERVERBOUND_MOVE_ROT).unwrap();
        writer.write_f32(90.0).unwrap();
        writer.write_f32(0.0).unwrap();
        writer.write_u8(1).unwrap();
        let outcome = session
            .apply_movement(&writer.into_inner(), &mut world)
            .unwrap();
        assert!(outcome.was_movement);
        assert_eq!(session.yaw, 90.0);

        let mut writer = PacketWriter::new();
        writer.write_varint(SERVERBOUND_MOVE_STATUS).unwrap();
        writer.write_u8(0).unwrap();
        let outcome = session
            .apply_movement(&writer.into_inner(), &mut world)
            .unwrap();
        assert!(outcome.was_movement);
        assert!(!session.on_ground);

        let mut writer = PacketWriter::new();
        writer.write_varint(99).unwrap();
        assert!(
            !session
                .apply_movement(&writer.into_inner(), &mut world)
                .unwrap()
                .was_movement
        );
    }
}
