use crate::crypto::{self, AES_KEY_SIZE, Cfb8};
use crate::varint::{read_varint, read_varint_slice, write_varint};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Vanilla clients refuse packets larger than 2 MiB. Keeping the same limit in
/// the Rust core prevents malicious length prefixes from allocating unbounded
/// memory.
pub const MAX_PACKET_SIZE: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextState {
    Status,
    Login,
    Transfer,
}

impl NextState {
    pub fn from_id(id: i32) -> io::Result<Self> {
        match id {
            1 => Ok(NextState::Status),
            2 => Ok(NextState::Login),
            3 => Ok(NextState::Transfer),
            other => Err(invalid(format!("unknown handshake next state {other}"))),
        }
    }

    pub fn id(self) -> i32 {
        match self {
            NextState::Status => 1,
            NextState::Login => 2,
            NextState::Transfer => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handshake {
    pub protocol_version: i32,
    pub server_address: String,
    pub server_port: u16,
    pub next_state: NextState,
}

pub fn read_packet<R: Read>(reader: &mut R) -> io::Result<Vec<u8>> {
    let length = read_varint(reader)?;
    if length < 0 || length as usize > MAX_PACKET_SIZE {
        return Err(invalid(format!("invalid packet length {length}")));
    }
    let mut payload = vec![0u8; length as usize];
    reader.read_exact(&mut payload)?;
    Ok(payload)
}

pub fn write_packet<W: Write>(writer: &mut W, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_PACKET_SIZE {
        return Err(invalid("packet exceeds 2 MiB"));
    }
    write_varint(writer, payload.len() as i32)?;
    writer.write_all(payload)
}

/// A framed connection that can transparently enable Minecraft's AES-128/CFB8
/// encryption.
///
/// Vanilla installs the cipher into the Netty pipeline once the encryption
/// response has been read, so everything after that point (including the login
/// success packet and any compression switch) is encrypted. The wrapper keeps
/// the same ordering: callers keep using the ordinary packet framing and
/// [`Connection::enable_encryption`] only changes how later frames are read
/// and written.
pub struct Connection<S = TcpStream> {
    stream: S,
    encryption: Option<Cfb8>,
    decryption: Option<Cfb8>,
}

impl<S> Connection<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            encryption: None,
            decryption: None,
        }
    }

    pub fn get_ref(&self) -> &S {
        &self.stream
    }

    pub fn get_mut(&mut self) -> &mut S {
        &mut self.stream
    }

    pub fn into_inner(self) -> S {
        self.stream
    }
}

impl<S: Read + Write> Connection<S> {
    /// Enable encryption in both directions with the shared secret. Refuses to
    /// replace an already installed cipher, mirroring `Connection.setEncryptionKey`
    /// which only acts while `encrypted` is false.
    pub fn enable_encryption(&mut self, secret: &[u8; AES_KEY_SIZE]) -> io::Result<()> {
        if self.encryption.is_some() || self.decryption.is_some() {
            return Err(invalid("connection encryption is already enabled"));
        }
        self.encryption = Some(Cfb8::encryptor(secret));
        self.decryption = Some(Cfb8::decryptor(secret));
        Ok(())
    }

    pub fn is_encrypted(&self) -> bool {
        self.encryption.is_some()
    }

    pub fn read_packet(&mut self) -> io::Result<Vec<u8>> {
        let length = self.read_frame_length()?;
        let mut payload = vec![0u8; length];
        self.stream.read_exact(&mut payload)?;
        if let Some(cipher) = &mut self.decryption {
            cipher.process(&mut payload);
        }
        Ok(payload)
    }

    pub fn write_packet(&mut self, payload: &[u8]) -> io::Result<()> {
        if payload.len() > MAX_PACKET_SIZE {
            return Err(invalid("packet exceeds 2 MiB"));
        }
        let mut frame = Vec::with_capacity(payload.len() + 5);
        write_varint(&mut frame, payload.len() as i32)?;
        frame.extend_from_slice(payload);
        if let Some(cipher) = &mut self.encryption {
            cipher.process(&mut frame);
        }
        self.stream.write_all(&frame)
    }

    fn read_frame_length(&mut self) -> io::Result<usize> {
        let mut value = 0u32;
        for index in 0..5 {
            let byte = self.read_encrypted_byte()?;
            value |= u32::from(byte & 0x7f) << (7 * index);
            if byte & 0x80 == 0 {
                let length = value as i32;
                if length < 0 || length as usize > MAX_PACKET_SIZE {
                    return Err(invalid(format!("invalid packet length {length}")));
                }
                return Ok(length as usize);
            }
        }
        Err(invalid("VarInt exceeds five bytes"))
    }

    fn read_encrypted_byte(&mut self) -> io::Result<u8> {
        let mut byte = [0u8; 1];
        self.stream.read_exact(&mut byte)?;
        if let Some(cipher) = &mut self.decryption {
            byte[0] = cipher.process_byte(byte[0]);
        }
        Ok(byte[0])
    }
}

impl Connection<TcpStream> {
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.stream.set_read_timeout(timeout)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.stream.set_write_timeout(timeout)
    }

    pub fn peer_addr(&self) -> io::Result<std::net::SocketAddr> {
        self.stream.peer_addr()
    }
}

pub fn parse_handshake(payload: &[u8]) -> io::Result<Handshake> {
    let mut reader = PacketReader::new(payload);
    let packet_id = reader.read_varint()?;
    if packet_id != 0 {
        return Err(invalid(format!(
            "expected handshake packet id 0, got {packet_id}"
        )));
    }
    let protocol_version = reader.read_varint()?;
    let server_address = reader.read_string()?;
    let server_port = reader.read_u16()?;
    let next_state = NextState::from_id(reader.read_varint()?)?;
    if !reader.is_empty() {
        return Err(invalid("trailing bytes after handshake"));
    }
    Ok(Handshake {
        protocol_version,
        server_address,
        server_port,
        next_state,
    })
}

pub fn build_handshake(
    protocol_version: i32,
    server_address: &str,
    server_port: u16,
    next_state: NextState,
) -> io::Result<Vec<u8>> {
    let mut writer = PacketWriter::new();
    writer.write_varint(0)?;
    writer.write_varint(protocol_version)?;
    writer.write_string(server_address)?;
    writer.write_u16(server_port)?;
    writer.write_varint(next_state.id())?;
    Ok(writer.into_inner())
}

pub fn build_status_request() -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(0).expect("writing to Vec cannot fail");
    writer.into_inner()
}

pub fn build_ping(payload: i64) -> Vec<u8> {
    let mut writer = PacketWriter::new();
    writer.write_varint(1).expect("writing to Vec cannot fail");
    writer
        .write_i64(payload)
        .expect("writing to Vec cannot fail");
    writer.into_inner()
}

pub fn build_pong(payload: i64) -> Vec<u8> {
    build_ping(payload)
}

pub fn build_status_response(json: &str) -> io::Result<Vec<u8>> {
    let mut writer = PacketWriter::new();
    writer.write_varint(0)?;
    writer.write_string(json)?;
    Ok(writer.into_inner())
}

pub fn parse_status_response(payload: &[u8]) -> io::Result<String> {
    let mut reader = PacketReader::new(payload);
    let packet_id = reader.read_varint()?;
    if packet_id != 0 {
        return Err(invalid(format!(
            "expected status response id 0, got {packet_id}"
        )));
    }
    reader.read_string()
}

pub fn parse_ping(payload: &[u8]) -> io::Result<i64> {
    let mut reader = PacketReader::new(payload);
    let packet_id = reader.read_varint()?;
    if packet_id != 1 {
        return Err(invalid(format!("expected ping id 1, got {packet_id}")));
    }
    reader.read_i64()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginStart {
    pub name: String,
    /// Present in 1.20.2+ login packets; older clients omit it.
    pub uuid: Option<[u8; 16]>,
}

pub fn parse_login_start(payload: &[u8]) -> io::Result<LoginStart> {
    let mut reader = PacketReader::new(payload);
    let packet_id = reader.read_varint()?;
    if packet_id != 0 {
        return Err(invalid(format!(
            "expected login start id 0, got {packet_id}"
        )));
    }
    let name = reader.read_string()?;
    // `ServerboundHelloPacket` reads the name with `readUtf(16)`.
    if name.chars().count() > 16 {
        return Err(invalid("player name is longer than 16 characters"));
    }
    let uuid = if reader.remaining() >= 16 {
        let bytes = reader.read_bytes(16)?;
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(bytes);
        Some(uuid)
    } else {
        None
    };
    if !reader.is_empty() {
        return Err(invalid("trailing bytes after login start"));
    }
    Ok(LoginStart { name, uuid })
}

pub fn build_login_start(name: &str, uuid: Option<[u8; 16]>) -> io::Result<Vec<u8>> {
    let mut writer = PacketWriter::new();
    writer.write_varint(0)?;
    writer.write_string(name)?;
    if let Some(uuid) = uuid {
        writer.write_bytes(&uuid);
    }
    Ok(writer.into_inner())
}

pub fn build_login_disconnect(reason_json: &str) -> io::Result<Vec<u8>> {
    let mut writer = PacketWriter::new();
    writer.write_varint(0)?;
    writer.write_string(reason_json)?;
    Ok(writer.into_inner())
}

/// Login protocol packet ids (`LoginProtocols` in 1.21.11).
pub const LOGIN_CLIENTBOUND_DISCONNECT: i32 = 0;
pub const LOGIN_CLIENTBOUND_HELLO: i32 = 1;
pub const LOGIN_CLIENTBOUND_SUCCESS: i32 = 2;
pub const LOGIN_CLIENTBOUND_COMPRESSION: i32 = 3;
pub const LOGIN_SERVERBOUND_KEY: i32 = 1;

/// `ClientboundHelloPacket`, the encryption request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionRequest {
    pub server_id: String,
    /// DER-encoded X.509 SubjectPublicKeyInfo (`PublicKey.getEncoded()`).
    pub public_key: Vec<u8>,
    pub verify_token: Vec<u8>,
    pub should_authenticate: bool,
}

pub fn build_encryption_request(request: &EncryptionRequest) -> io::Result<Vec<u8>> {
    let mut writer = PacketWriter::new();
    writer.write_varint(LOGIN_CLIENTBOUND_HELLO)?;
    writer.write_string(&request.server_id)?;
    writer.write_byte_array(&request.public_key)?;
    writer.write_byte_array(&request.verify_token)?;
    writer.write_u8(u8::from(request.should_authenticate))?;
    Ok(writer.into_inner())
}

pub fn parse_encryption_request(payload: &[u8]) -> io::Result<EncryptionRequest> {
    let mut reader = PacketReader::new(payload);
    let id = reader.read_varint()?;
    if id != LOGIN_CLIENTBOUND_HELLO {
        return Err(invalid(format!(
            "expected encryption request id {LOGIN_CLIENTBOUND_HELLO}, got {id}"
        )));
    }
    let server_id = reader.read_string()?;
    let public_key = reader.read_byte_array(4096)?;
    let verify_token = reader.read_byte_array(4096)?;
    let should_authenticate = reader.read_bool()?;
    if !reader.is_empty() {
        return Err(invalid("trailing bytes after encryption request"));
    }
    Ok(EncryptionRequest {
        server_id,
        public_key,
        verify_token,
        should_authenticate,
    })
}

/// `ServerboundKeyPacket`, the encryption response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptionResponse {
    pub shared_secret: Vec<u8>,
    pub verify_token: Vec<u8>,
}

pub fn build_encryption_response(response: &EncryptionResponse) -> io::Result<Vec<u8>> {
    let mut writer = PacketWriter::new();
    writer.write_varint(LOGIN_SERVERBOUND_KEY)?;
    writer.write_byte_array(&response.shared_secret)?;
    writer.write_byte_array(&response.verify_token)?;
    Ok(writer.into_inner())
}

pub fn parse_encryption_response(payload: &[u8]) -> io::Result<EncryptionResponse> {
    let mut reader = PacketReader::new(payload);
    let id = reader.read_varint()?;
    if id != LOGIN_SERVERBOUND_KEY {
        return Err(invalid(format!(
            "expected encryption response id {LOGIN_SERVERBOUND_KEY}, got {id}"
        )));
    }
    let shared_secret = reader.read_byte_array(4096)?;
    let verify_token = reader.read_byte_array(4096)?;
    if !reader.is_empty() {
        return Err(invalid("trailing bytes after encryption response"));
    }
    Ok(EncryptionResponse {
        shared_secret,
        verify_token,
    })
}

/// One entry of `GameProfile.properties` (skin/cape data).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileProperty {
    pub name: String,
    pub value: String,
    pub signature: Option<String>,
}

impl ProfileProperty {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            signature: None,
        }
    }
}

/// The GameProfile carried by `ClientboundLoginFinishedPacket` and returned by
/// Mojang's `hasJoinedServer`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameProfile {
    pub uuid: [u8; 16],
    pub name: String,
    pub properties: Vec<ProfileProperty>,
}

impl GameProfile {
    pub fn new(uuid: [u8; 16], name: impl Into<String>) -> Self {
        Self {
            uuid,
            name: name.into(),
            properties: Vec::new(),
        }
    }
}

/// `ClientboundLoginFinishedPacket` (Login Success).
///
/// Layout is the `GAME_PROFILE` codec: UUID, name (max 16 UTF-8 bytes), then a
/// counted list of `(name, value, nullable signature)` properties. Offline
/// logins send zero properties, which is byte-identical to the previous
/// implementation.
pub fn build_login_success(profile: &GameProfile) -> io::Result<Vec<u8>> {
    let mut writer = PacketWriter::new();
    writer.write_varint(LOGIN_CLIENTBOUND_SUCCESS)?;
    writer.write_bytes(&profile.uuid);
    writer.write_string(&profile.name)?;
    let property_count = i32::try_from(profile.properties.len())
        .map_err(|_| invalid("too many profile properties"))?;
    writer.write_varint(property_count)?;
    for property in &profile.properties {
        writer.write_string(&property.name)?;
        writer.write_string(&property.value)?;
        match &property.signature {
            Some(signature) => {
                writer.write_u8(1)?;
                writer.write_string(signature)?;
            }
            None => writer.write_u8(0)?,
        }
    }
    Ok(writer.into_inner())
}

pub fn parse_login_success(payload: &[u8]) -> io::Result<GameProfile> {
    let mut reader = PacketReader::new(payload);
    let id = reader.read_varint()?;
    if id != LOGIN_CLIENTBOUND_SUCCESS {
        return Err(invalid(format!(
            "expected login success id {LOGIN_CLIENTBOUND_SUCCESS}, got {id}"
        )));
    }
    let uuid = reader.read_bytes(16)?;
    let mut uuid_bytes = [0u8; 16];
    uuid_bytes.copy_from_slice(uuid);
    let name = reader.read_string()?;
    let count = reader.read_varint()?;
    if count < 0 || count as usize > 16 {
        return Err(invalid(format!("invalid profile property count {count}")));
    }
    let mut properties = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let name = reader.read_string()?;
        let value = reader.read_string()?;
        let signature = if reader.read_bool()? {
            Some(reader.read_string()?)
        } else {
            None
        };
        properties.push(ProfileProperty {
            name,
            value,
            signature,
        });
    }
    if !reader.is_empty() {
        return Err(invalid("trailing bytes after login success"));
    }
    Ok(GameProfile {
        uuid: uuid_bytes,
        name,
        properties,
    })
}

// ===========================================================================
// 1.21.11 chat signing wire types
// ===========================================================================

/// `MessageSignature.BYTES`: every profile-key signature is exactly 256 bytes.
pub const MESSAGE_SIGNATURE_BYTES: usize = 256;
/// `LastSeenMessages.LAST_SEEN_MESSAGES_MAX_LENGTH`.
pub const LAST_SEEN_MESSAGES_MAX: usize = 20;
/// `ServerboundChatPacket` reads the message with `readUtf(256)`.
pub const MAX_CHAT_MESSAGE_LENGTH: usize = 256;
/// `GameProtocolTypes.SERVERBOUND_CHAT` registration index.
pub const SERVERBOUND_CHAT: i32 = 8;
/// `MessageSignatureCache.DEFAULT_CAPACITY`.
pub const MESSAGE_SIGNATURE_CACHE_CAPACITY: usize = 128;

/// A 256-byte RSA signature (`net.minecraft.network.chat.MessageSignature`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MessageSignature {
    bytes: [u8; MESSAGE_SIGNATURE_BYTES],
}

impl MessageSignature {
    pub fn new(bytes: [u8; MESSAGE_SIGNATURE_BYTES]) -> Self {
        Self { bytes }
    }

    pub fn from_slice(bytes: &[u8]) -> io::Result<Self> {
        let bytes: [u8; MESSAGE_SIGNATURE_BYTES] = bytes.try_into().map_err(|_| {
            invalid(format!(
                "message signature must be {MESSAGE_SIGNATURE_BYTES} bytes, got {}",
                bytes.len()
            ))
        })?;
        Ok(Self { bytes })
    }

    pub fn as_bytes(&self) -> &[u8; MESSAGE_SIGNATURE_BYTES] {
        &self.bytes
    }

    /// Mutable access for tamper-detection tests. Never use this on a signature
    /// that is about to be sent or verified in production code.
    pub fn as_bytes_mut(&mut self) -> &mut [u8; MESSAGE_SIGNATURE_BYTES] {
        &mut self.bytes
    }

    pub fn read(reader: &mut PacketReader<'_>) -> io::Result<Self> {
        let mut bytes = [0u8; MESSAGE_SIGNATURE_BYTES];
        bytes.copy_from_slice(reader.read_bytes(MESSAGE_SIGNATURE_BYTES)?);
        Ok(Self { bytes })
    }

    pub fn write(&self, writer: &mut PacketWriter) -> io::Result<()> {
        writer.write_bytes(&self.bytes);
        Ok(())
    }

    /// Java `Arrays.hashCode(byte[])`, used by `LastSeenMessages.computeChecksum`.
    pub fn checksum(&self) -> i32 {
        let mut result = 1i32;
        for byte in self.bytes {
            result = result.wrapping_mul(31).wrapping_add(i32::from(byte as i8));
        }
        result
    }
}

/// `MessageSignature.Packed`: a cache id, or `-1` followed by the full
/// signature on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackedMessageSignature {
    Cached(i32),
    Full(Box<MessageSignature>),
}

impl PackedMessageSignature {
    pub fn read(reader: &mut PacketReader<'_>) -> io::Result<Self> {
        let id = reader.read_varint()?.wrapping_sub(1);
        if id == -1 {
            Ok(Self::Full(Box::new(MessageSignature::read(reader)?)))
        } else {
            Ok(Self::Cached(id))
        }
    }

    pub fn write(&self, writer: &mut PacketWriter) -> io::Result<()> {
        match self {
            Self::Cached(id) => writer.write_varint(id.wrapping_add(1)),
            Self::Full(signature) => {
                writer.write_varint(0)?;
                signature.write(writer)
            }
        }
    }

    pub fn unpack(&self, cache: &MessageSignatureCache) -> io::Result<MessageSignature> {
        match self {
            Self::Cached(id) => cache.unpack(*id).cloned().ok_or_else(|| {
                invalid(format!(
                    "message signature cache id {id} is not present in this session"
                ))
            }),
            Self::Full(signature) => Ok((**signature).clone()),
        }
    }
}

/// Bounded signature cache used by `SignedMessageBody.Packed`.
#[derive(Debug, Clone)]
pub struct MessageSignatureCache {
    entries: Vec<Option<MessageSignature>>,
}

impl Default for MessageSignatureCache {
    fn default() -> Self {
        Self::new(MESSAGE_SIGNATURE_CACHE_CAPACITY)
    }
}

impl MessageSignatureCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: vec![None; capacity.max(1)],
        }
    }

    pub fn pack(&self, signature: &MessageSignature) -> PackedMessageSignature {
        for (index, entry) in self.entries.iter().enumerate() {
            if entry.as_ref() == Some(signature) {
                return PackedMessageSignature::Cached(index as i32);
            }
        }
        PackedMessageSignature::Full(Box::new(signature.clone()))
    }

    pub fn unpack(&self, index: i32) -> Option<&MessageSignature> {
        usize::try_from(index)
            .ok()
            .and_then(|index| self.entries.get(index))
            .and_then(Option::as_ref)
    }

    /// `MessageSignatureCache.push`: newest signatures occupy the low indexes
    /// and entries that are no longer referenced are discarded.
    pub fn push(&mut self, body: &SignedMessageBody, signature: Option<&MessageSignature>) {
        let mut incoming: Vec<MessageSignature> = body.last_seen.entries().to_vec();
        if let Some(signature) = signature {
            incoming.push(signature.clone());
        }
        let mut new_set: Vec<MessageSignature> = Vec::new();
        for signature in &incoming {
            if !new_set.contains(signature) {
                new_set.push(signature.clone());
            }
        }

        let old: Vec<Option<MessageSignature>> = self.entries.clone();
        let mut incoming_reversed = incoming;
        let mut old_front: Vec<MessageSignature> = Vec::new();
        for old_entry in old {
            if let Some(old_entry) = old_entry
                && !new_set.contains(&old_entry)
                && !old_front.contains(&old_entry)
            {
                old_front.push(old_entry);
            }
        }
        for entry in &mut self.entries {
            *entry = incoming_reversed.pop().or_else(|| {
                if old_front.is_empty() {
                    None
                } else {
                    Some(old_front.remove(0))
                }
            });
        }
    }
}

fn read_signature_list(
    reader: &mut PacketReader<'_>,
    cache: &MessageSignatureCache,
) -> io::Result<Vec<MessageSignature>> {
    let count = reader.read_varint()?;
    if count < 0 || count as usize > LAST_SEEN_MESSAGES_MAX {
        return Err(invalid(format!(
            "invalid last-seen signature count {count}"
        )));
    }
    let mut signatures = Vec::with_capacity(count as usize);
    for _ in 0..count {
        signatures.push(PackedMessageSignature::read(reader)?.unpack(cache)?);
    }
    Ok(signatures)
}

fn write_signature_list(
    writer: &mut PacketWriter,
    signatures: &[MessageSignature],
    cache: &MessageSignatureCache,
) -> io::Result<()> {
    if signatures.len() > LAST_SEEN_MESSAGES_MAX {
        return Err(invalid(format!(
            "last-seen signature list has {} entries, maximum is {LAST_SEEN_MESSAGES_MAX}",
            signatures.len()
        )));
    }
    writer.write_varint(signatures.len() as i32)?;
    for signature in signatures {
        cache.pack(signature).write(writer)?;
    }
    Ok(())
}

/// `LastSeenMessages`: the acknowledged signatures a client includes in the
/// next signed body.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LastSeenMessages {
    entries: Vec<MessageSignature>,
}

impl LastSeenMessages {
    pub fn new(entries: Vec<MessageSignature>) -> io::Result<Self> {
        if entries.len() > LAST_SEEN_MESSAGES_MAX {
            return Err(invalid(format!(
                "last-seen message list has {} entries, maximum is {LAST_SEEN_MESSAGES_MAX}",
                entries.len()
            )));
        }
        Ok(Self { entries })
    }

    pub fn empty() -> Self {
        Self::default()
    }

    pub fn entries(&self) -> &[MessageSignature] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn push(&mut self, signature: MessageSignature) -> io::Result<()> {
        if self.entries.len() >= LAST_SEEN_MESSAGES_MAX {
            self.entries.remove(0);
        }
        if !self.entries.contains(&signature) {
            self.entries.push(signature);
        }
        Ok(())
    }

    /// `LastSeenMessages.computeChecksum`.
    pub fn compute_checksum(&self) -> u8 {
        let mut value = 1i32;
        for signature in &self.entries {
            value = value.wrapping_mul(31).wrapping_add(signature.checksum());
        }
        let byte = value as u8;
        if byte == 0 { 1 } else { byte }
    }

    pub fn write_packed(
        &self,
        writer: &mut PacketWriter,
        cache: &MessageSignatureCache,
    ) -> io::Result<()> {
        write_signature_list(writer, &self.entries, cache)
    }

    pub fn read_packed(
        reader: &mut PacketReader<'_>,
        cache: &MessageSignatureCache,
    ) -> io::Result<Self> {
        Ok(Self {
            entries: read_signature_list(reader, cache)?,
        })
    }
}

/// `LastSeenMessages.Update`: the acknowledgement window sent with a chat
/// packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastSeenUpdate {
    pub offset: i32,
    pub acknowledged: u32,
    pub checksum: u8,
}

impl LastSeenUpdate {
    pub fn new(offset: i32, acknowledged: u32, checksum: u8) -> io::Result<Self> {
        if offset < 0 || acknowledged >= (1 << LAST_SEEN_MESSAGES_MAX) {
            return Err(invalid(format!(
                "invalid last-seen update offset {offset} or acknowledgement bits {acknowledged:#x}"
            )));
        }
        Ok(Self {
            offset,
            acknowledged,
            checksum,
        })
    }

    pub fn acknowledges(&self, index: usize) -> bool {
        index < LAST_SEEN_MESSAGES_MAX && (self.acknowledged >> index) & 1 != 0
    }

    pub fn read(reader: &mut PacketReader<'_>) -> io::Result<Self> {
        let offset = reader.read_varint()?;
        let bytes = reader.read_bytes(3)?;
        let acknowledged =
            u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16);
        let checksum = reader.read_u8()?;
        Self::new(offset, acknowledged, checksum)
    }

    pub fn write(&self, writer: &mut PacketWriter) -> io::Result<()> {
        Self::new(self.offset, self.acknowledged, self.checksum)?;
        writer.write_varint(self.offset)?;
        let bytes = self.acknowledged.to_le_bytes();
        writer.write_bytes(&bytes[..3]);
        writer.write_u8(self.checksum)
    }
}

/// `SignedMessageBody.Packed`: the signed body carried by
/// `ClientboundPlayerChatPacket` and mirrored in `ServerboundChatPacket`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedMessageBody {
    pub content: String,
    pub timestamp_ms: i64,
    pub salt: i64,
    pub last_seen: LastSeenMessages,
}

impl SignedMessageBody {
    pub fn new(
        content: impl Into<String>,
        timestamp_ms: i64,
        salt: i64,
        last_seen: LastSeenMessages,
    ) -> io::Result<Self> {
        let content = content.into();
        if content.chars().count() > MAX_CHAT_MESSAGE_LENGTH {
            return Err(invalid(format!(
                "chat message is longer than {MAX_CHAT_MESSAGE_LENGTH} characters"
            )));
        }
        Ok(Self {
            content,
            timestamp_ms,
            salt,
            last_seen,
        })
    }

    pub fn content_hash(&self) -> [u8; crypto::SHA256_DIGEST_SIZE] {
        crypto::sha256(self.content.as_bytes())
    }

    pub fn write_packed(
        &self,
        writer: &mut PacketWriter,
        cache: &MessageSignatureCache,
    ) -> io::Result<()> {
        writer.write_string(&self.content)?;
        writer.write_i64(self.timestamp_ms)?;
        writer.write_i64(self.salt)?;
        self.last_seen.write_packed(writer, cache)
    }

    pub fn read_packed(
        reader: &mut PacketReader<'_>,
        cache: &MessageSignatureCache,
    ) -> io::Result<Self> {
        let content = reader.read_string()?;
        if content.chars().count() > MAX_CHAT_MESSAGE_LENGTH {
            return Err(invalid(format!(
                "chat message is longer than {MAX_CHAT_MESSAGE_LENGTH} characters"
            )));
        }
        let timestamp_ms = reader.read_i64()?;
        let salt = reader.read_i64()?;
        let last_seen = LastSeenMessages::read_packed(reader, cache)?;
        Ok(Self {
            content,
            timestamp_ms,
            salt,
            last_seen,
        })
    }
}

/// `ServerboundChatPacket` body for 1.21.11.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerboundChat {
    pub message: String,
    pub timestamp_ms: i64,
    pub salt: i64,
    pub signature: Option<MessageSignature>,
    pub last_seen: LastSeenUpdate,
}

pub fn parse_serverbound_chat(payload: &[u8]) -> io::Result<ServerboundChat> {
    let mut reader = PacketReader::new(payload);
    let id = reader.read_varint()?;
    if id != SERVERBOUND_CHAT {
        return Err(invalid(format!(
            "expected serverbound chat id {SERVERBOUND_CHAT}, got {id}"
        )));
    }
    let message = reader.read_string()?;
    if message.chars().count() > MAX_CHAT_MESSAGE_LENGTH {
        return Err(invalid(format!(
            "chat message is longer than {MAX_CHAT_MESSAGE_LENGTH} characters"
        )));
    }
    let timestamp_ms = reader.read_i64()?;
    let salt = reader.read_i64()?;
    let signature = if reader.read_bool()? {
        Some(MessageSignature::read(&mut reader)?)
    } else {
        None
    };
    let last_seen = LastSeenUpdate::read(&mut reader)?;
    if !reader.is_empty() {
        return Err(invalid("trailing bytes after serverbound chat"));
    }
    Ok(ServerboundChat {
        message,
        timestamp_ms,
        salt,
        signature,
        last_seen,
    })
}

pub fn build_serverbound_chat(chat: &ServerboundChat) -> io::Result<Vec<u8>> {
    if chat.message.chars().count() > MAX_CHAT_MESSAGE_LENGTH {
        return Err(invalid(format!(
            "chat message is longer than {MAX_CHAT_MESSAGE_LENGTH} characters"
        )));
    }
    let mut writer = PacketWriter::new();
    writer.write_varint(SERVERBOUND_CHAT)?;
    writer.write_string(&chat.message)?;
    writer.write_i64(chat.timestamp_ms)?;
    writer.write_i64(chat.salt)?;
    match &chat.signature {
        Some(signature) => {
            writer.write_u8(1)?;
            signature.write(&mut writer)?;
        }
        None => writer.write_u8(0)?,
    }
    chat.last_seen.write(&mut writer)?;
    Ok(writer.into_inner())
}

/// Packets at or above `threshold` are zlib-compressed after the login
/// `Set Compression` packet. Below the threshold, Minecraft sends a zero
/// uncompressed-length marker followed by the raw payload.
pub fn write_compressed_packet<W: Write>(
    writer: &mut W,
    payload: &[u8],
    threshold: usize,
) -> io::Result<()> {
    let mut inner = PacketWriter::new();
    if payload.len() >= threshold {
        inner.write_varint(payload.len() as i32)?;
        let compressed = crate::compression::compress(payload)?;
        inner.write_bytes(&compressed);
    } else {
        inner.write_varint(0)?;
        inner.write_bytes(payload);
    }
    write_packet(writer, &inner.into_inner())
}

pub fn read_compressed_packet<R: Read>(reader: &mut R, maximum_size: usize) -> io::Result<Vec<u8>> {
    let packet = read_packet(reader)?;
    let mut inner = PacketReader::new(&packet);
    let uncompressed_length = inner.read_varint()?;
    if uncompressed_length == 0 {
        return Ok(inner.read_bytes(inner.remaining())?.to_vec());
    }
    if uncompressed_length < 0 || uncompressed_length as usize > maximum_size {
        return Err(invalid(format!(
            "invalid uncompressed packet length {uncompressed_length}"
        )));
    }
    let compressed = inner.read_bytes(inner.remaining())?;
    let payload = crate::compression::decompress(compressed, maximum_size)?;
    if payload.len() != uncompressed_length as usize {
        return Err(invalid(format!(
            "declared uncompressed length {uncompressed_length}, got {}",
            payload.len()
        )));
    }
    Ok(payload)
}

pub struct PacketReader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> PacketReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    pub fn position(&self) -> usize {
        self.position
    }

    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.position)
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    pub fn read_varint(&mut self) -> io::Result<i32> {
        let (value, consumed) = read_varint_slice(&self.data[self.position..])?;
        self.position += consumed;
        Ok(value)
    }

    pub fn read_varlong(&mut self) -> io::Result<i64> {
        let (value, consumed) = crate::varint::read_varlong_slice(&self.data[self.position..])?;
        self.position += consumed;
        Ok(value)
    }

    pub fn read_u8(&mut self) -> io::Result<u8> {
        if self.remaining() < 1 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "packet is truncated",
            ));
        }
        let value = self.data[self.position];
        self.position += 1;
        Ok(value)
    }

    pub fn read_u16(&mut self) -> io::Result<u16> {
        let bytes = self.read_bytes(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    pub fn read_i64(&mut self) -> io::Result<i64> {
        let bytes = self.read_bytes(8)?;
        let mut array = [0u8; 8];
        array.copy_from_slice(bytes);
        Ok(i64::from_be_bytes(array))
    }

    pub fn read_i32(&mut self) -> io::Result<i32> {
        let bytes = self.read_bytes(4)?;
        Ok(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub fn read_i16(&mut self) -> io::Result<i16> {
        let bytes = self.read_bytes(2)?;
        Ok(i16::from_be_bytes([bytes[0], bytes[1]]))
    }

    pub fn read_f32(&mut self) -> io::Result<f32> {
        let bytes = self.read_bytes(4)?;
        Ok(f32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    pub fn read_f64(&mut self) -> io::Result<f64> {
        let bytes = self.read_bytes(8)?;
        let mut array = [0u8; 8];
        array.copy_from_slice(bytes);
        Ok(f64::from_be_bytes(array))
    }

    pub fn read_string(&mut self) -> io::Result<String> {
        let length = self.read_varint()?;
        if length < 0 || length as usize > self.remaining() {
            return Err(invalid(format!("invalid string length {length}")));
        }
        let bytes = self.read_bytes(length as usize)?;
        String::from_utf8(bytes.to_vec())
            .map_err(|error| invalid(format!("invalid UTF-8 in packet string: {error}")))
    }

    pub fn read_bool(&mut self) -> io::Result<bool> {
        Ok(self.read_u8()? != 0)
    }

    pub fn read_nbt_root(&mut self) -> io::Result<crate::nbt::Tag> {
        let mut cursor = io::Cursor::new(&self.data[self.position..]);
        let tag = crate::nbt::read_root(&mut cursor)?;
        self.position += cursor.position() as usize;
        Ok(tag)
    }

    pub fn read_nbt_any(&mut self) -> io::Result<crate::nbt::Tag> {
        let mut cursor = io::Cursor::new(&self.data[self.position..]);
        let tag = crate::nbt::read_any_tag(&mut cursor)?;
        self.position += cursor.position() as usize;
        Ok(tag)
    }

    pub fn read_bytes(&mut self, length: usize) -> io::Result<&'a [u8]> {
        if self.remaining() < length {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "packet is truncated",
            ));
        }
        let start = self.position;
        self.position += length;
        Ok(&self.data[start..self.position])
    }

    /// `FriendlyByteBuf.readByteArray(max)`.
    pub fn read_byte_array(&mut self, maximum: usize) -> io::Result<Vec<u8>> {
        let length = self.read_varint()?;
        if length < 0 || length as usize > maximum || length as usize > self.remaining() {
            return Err(invalid(format!("invalid byte array length {length}")));
        }
        Ok(self.read_bytes(length as usize)?.to_vec())
    }
}

pub struct PacketWriter {
    data: Vec<u8>,
}

impl PacketWriter {
    pub fn new() -> Self {
        Self { data: Vec::new() }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
        }
    }

    pub fn write_varint(&mut self, value: i32) -> io::Result<()> {
        write_varint(&mut self.data, value)?;
        Ok(())
    }

    pub fn write_varlong(&mut self, value: i64) -> io::Result<()> {
        crate::varint::write_varlong(&mut self.data, value)?;
        Ok(())
    }

    pub fn write_u8(&mut self, value: u8) -> io::Result<()> {
        self.data.push(value);
        Ok(())
    }

    pub fn write_u16(&mut self, value: u16) -> io::Result<()> {
        self.data.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    pub fn write_i64(&mut self, value: i64) -> io::Result<()> {
        self.data.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    pub fn write_i32(&mut self, value: i32) -> io::Result<()> {
        self.data.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    pub fn write_i16(&mut self, value: i16) -> io::Result<()> {
        self.data.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    pub fn write_f32(&mut self, value: f32) -> io::Result<()> {
        self.data.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    pub fn write_f64(&mut self, value: f64) -> io::Result<()> {
        self.data.extend_from_slice(&value.to_be_bytes());
        Ok(())
    }

    pub fn write_string(&mut self, value: &str) -> io::Result<()> {
        if value.len() > i32::MAX as usize {
            return Err(invalid("string is too long"));
        }
        self.write_varint(value.len() as i32)?;
        self.data.extend_from_slice(value.as_bytes());
        Ok(())
    }

    pub fn write_bytes(&mut self, value: &[u8]) {
        self.data.extend_from_slice(value);
    }

    /// `FriendlyByteBuf.writeByteArray`: VarInt length followed by the bytes.
    pub fn write_byte_array(&mut self, value: &[u8]) -> io::Result<()> {
        self.write_varint(value.len() as i32)?;
        self.write_bytes(value);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.data
    }
}

impl Default for PacketWriter {
    fn default() -> Self {
        Self::new()
    }
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_framing_round_trip() {
        let payload = vec![0x42; 300];
        let mut framed = Vec::new();
        write_packet(&mut framed, &payload).unwrap();
        assert_eq!(framed[0..2], [0xAC, 0x02]);
        let decoded = read_packet(&mut framed.as_slice()).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn handshake_round_trip() {
        let bytes = build_handshake(774, "mc.example.com", 25565, NextState::Status).unwrap();
        let handshake = parse_handshake(&bytes).unwrap();
        assert_eq!(
            handshake,
            Handshake {
                protocol_version: 774,
                server_address: "mc.example.com".into(),
                server_port: 25565,
                next_state: NextState::Status,
            }
        );
    }

    #[test]
    fn status_response_round_trip() {
        let json = r#"{"version":{"name":"1.21.11","protocol":774}}"#;
        let payload = build_status_response(json).unwrap();
        assert_eq!(parse_status_response(&payload).unwrap(), json);
    }

    #[test]
    fn ping_round_trip() {
        let payload = build_ping(0x1122334455667788);
        assert_eq!(parse_ping(&payload).unwrap(), 0x1122334455667788);
    }

    #[test]
    fn rejects_unknown_packet_id() {
        let mut writer = PacketWriter::new();
        writer.write_varint(99).unwrap();
        assert!(parse_handshake(&writer.into_inner()).is_err());
    }

    #[test]
    fn rejects_oversized_packet() {
        let mut writer = Vec::new();
        write_varint(&mut writer, (MAX_PACKET_SIZE + 1) as i32).unwrap();
        assert!(read_packet(&mut writer.as_slice()).is_err());
    }

    #[test]
    fn rejects_truncated_packet() {
        let bytes = [0x05, 0x00];
        assert!(read_packet(&mut bytes.as_slice()).is_err());
    }

    #[test]
    fn handles_utf8_strings() {
        let mut writer = PacketWriter::new();
        writer.write_string("Folia-RS ✓").unwrap();
        let data = writer.into_inner();
        let mut reader = PacketReader::new(&data);
        assert_eq!(reader.read_string().unwrap(), "Folia-RS ✓");
    }

    #[test]
    fn compressed_packets_round_trip_above_and_below_threshold() {
        let small = vec![0x42; 10];
        let large = vec![0x24; 4096];

        let mut small_wire = Vec::new();
        write_compressed_packet(&mut small_wire, &small, 256).unwrap();
        assert_eq!(
            read_compressed_packet(&mut small_wire.as_slice(), 1 << 20).unwrap(),
            small
        );

        let mut large_wire = Vec::new();
        write_compressed_packet(&mut large_wire, &large, 256).unwrap();
        assert_eq!(
            read_compressed_packet(&mut large_wire.as_slice(), 1 << 20).unwrap(),
            large
        );
    }

    #[test]
    fn rejects_declared_compressed_length_mismatch() {
        let payload = vec![7u8; 2048];
        let compressed = crate::compression::compress(&payload).unwrap();

        let mut inner = PacketWriter::new();
        inner.write_varint((payload.len() + 1) as i32).unwrap();
        inner.write_bytes(&compressed);

        let mut wire = Vec::new();
        write_packet(&mut wire, &inner.into_inner()).unwrap();
        assert!(read_compressed_packet(&mut wire.as_slice(), 1 << 20).is_err());
    }

    #[test]
    fn login_start_round_trips_with_and_without_uuid() {
        let uuid = [0x11u8; 16];
        let payload = build_login_start("Steve", Some(uuid)).unwrap();
        assert_eq!(
            parse_login_start(&payload).unwrap(),
            LoginStart {
                name: "Steve".into(),
                uuid: Some(uuid),
            }
        );

        let payload = build_login_start("Alex", None).unwrap();
        assert_eq!(
            parse_login_start(&payload).unwrap(),
            LoginStart {
                name: "Alex".into(),
                uuid: None,
            }
        );
    }

    #[test]
    fn rejects_login_names_longer_than_vanilla_allows() {
        let payload = build_login_start("ABCDEFGHIJKLMNOPQ", None).unwrap();
        assert!(parse_login_start(&payload).is_err());
        let payload = build_login_start("ABCDEFGHIJKLMNOP", None).unwrap();
        assert!(parse_login_start(&payload).is_ok());
    }
}
