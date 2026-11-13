//! Focused tests for 1.21.11 chat signing: signed bodies, sequence/timestamp
//! bounds, tamper detection, profile-key provisioning in Login Success, and the
//! server config switch.

use folia_core::config::ServerProperties;
use folia_core::crypto;
use folia_core::player::{
    CHAT_MESSAGE_EXPIRES_AFTER_SERVER_MS, ChatSigningError, ChatSigningKey, ChatSigningSession,
    ProfileKeyPair, ProfilePublicKey, RemoteChatSession, SignedMessage,
};
use folia_core::protocol::{
    LAST_SEEN_MESSAGES_MAX, LastSeenMessages, LastSeenUpdate, MESSAGE_SIGNATURE_BYTES,
    MessageSignature, MessageSignatureCache, SERVERBOUND_CHAT, SERVERBOUND_CHAT_COMMAND,
    SERVERBOUND_CHAT_COMMAND_SIGNED, ServerboundChat, ServerboundChatCommand,
    ServerboundChatCommandSigned, SignedCommandArgument, SignedMessageBody, build_login_success,
    build_serverbound_chat, build_serverbound_chat_command, build_serverbound_chat_command_signed,
    parse_login_success, parse_serverbound_chat, parse_serverbound_chat_command,
    parse_serverbound_chat_command_signed,
};

const SENDER: [u8; 16] = [0x11; 16];
const SESSION_ID: [u8; 16] = [0x22; 16];
const SEED: [u8; 32] = [0x33; 32];
const NOW: i64 = 1_700_000_000_000;

fn signing_session() -> ChatSigningSession {
    ChatSigningSession::deterministic_for_tests(SENDER, SESSION_ID, SEED, NOW).unwrap()
}

fn remote(session: &ChatSigningSession, enforced: bool) -> RemoteChatSession {
    RemoteChatSession::new(
        SENDER,
        SESSION_ID,
        session.public_key().clone(),
        NOW,
        enforced,
        None,
    )
    .unwrap()
}

fn signature(byte: u8) -> MessageSignature {
    MessageSignature::new([byte; MESSAGE_SIGNATURE_BYTES])
}

#[test]
fn sign_and_verify_round_trip() {
    let mut signer = signing_session();
    let message = signer.sign("hello world", 42, NOW).unwrap();

    assert_eq!(message.sequence, 0);
    assert_eq!(message.sender, SENDER);
    assert_eq!(message.session_id, SESSION_ID);
    assert_eq!(message.body.salt, 42);
    assert_eq!(message.body.timestamp_ms, NOW);
    assert_eq!(message.content_hash, crypto::sha256(b"hello world"));
    assert_eq!(
        message.signature.as_ref().unwrap().as_bytes().len(),
        MESSAGE_SIGNATURE_BYTES
    );

    let mut verifier = remote(&signer, false);
    verifier.verify(&message, NOW).unwrap();
    assert_eq!(verifier.next_sequence(), 1);

    let second = signer.sign("second", 43, NOW + 1).unwrap();
    assert_eq!(second.sequence, 1);
    verifier.verify(&second, NOW + 1).unwrap();
    assert_eq!(verifier.next_sequence(), 2);
}

#[test]
fn tampering_with_content_signature_or_body_is_rejected() {
    let mut signer = signing_session();
    let message = signer.sign("original", 7, NOW).unwrap();
    let mut verifier = remote(&signer, false);

    let mut content_tampered = message.clone();
    content_tampered.body.content = "replaced".into();
    assert_eq!(
        verifier.verify(&content_tampered, NOW).unwrap_err(),
        ChatSigningError::InvalidSignature
    );

    let mut signature_tampered = message.clone();
    signature_tampered
        .signature
        .as_mut()
        .unwrap()
        .as_bytes_mut()[0] ^= 1;
    assert_eq!(
        verifier.verify(&signature_tampered, NOW).unwrap_err(),
        ChatSigningError::InvalidSignature
    );

    let mut salt_tampered = message.clone();
    salt_tampered.body.salt ^= 1;
    assert_eq!(
        verifier.verify(&salt_tampered, NOW).unwrap_err(),
        ChatSigningError::InvalidSignature
    );

    // Vanilla signs epoch seconds, so use a full second to change the payload.
    let mut timestamp_tampered = message.clone();
    timestamp_tampered.body.timestamp_ms += 1000;
    assert_eq!(
        verifier.verify(&timestamp_tampered, NOW).unwrap_err(),
        ChatSigningError::InvalidSignature
    );

    // The valid message still verifies after all the rejected attempts.
    verifier.verify(&message, NOW).unwrap();
}

#[test]
fn sequence_replay_and_skips_are_rejected() {
    let mut signer = signing_session();
    let message = signer.sign("ordered", 1, NOW).unwrap();
    let mut verifier = remote(&signer, false);
    verifier.verify(&message, NOW).unwrap();

    assert_eq!(
        verifier.verify(&message, NOW).unwrap_err(),
        ChatSigningError::SequenceOutOfBounds {
            expected: 1,
            received: 0
        }
    );

    let mut skipped = message.clone();
    skipped.sequence = 5;
    assert_eq!(
        verifier.verify(&skipped, NOW).unwrap_err(),
        ChatSigningError::SequenceOutOfBounds {
            expected: 1,
            received: 5
        }
    );

    let mut negative = message.clone();
    negative.sequence = -1;
    assert_eq!(
        verifier.verify(&negative, NOW).unwrap_err(),
        ChatSigningError::SequenceOutOfBounds {
            expected: 1,
            received: -1
        }
    );
}

#[test]
fn timestamp_bounds_reject_expired_future_and_out_of_order_messages() {
    let mut expired_signer =
        ChatSigningSession::deterministic_for_tests(SENDER, SESSION_ID, SEED, NOW).unwrap();
    let stale_timestamp = NOW - CHAT_MESSAGE_EXPIRES_AFTER_SERVER_MS - 1;
    let expired_message = expired_signer.sign("stale", 1, stale_timestamp).unwrap();
    let expired_verifier = remote(&expired_signer, false);
    let mut expired_verifier = expired_verifier;
    assert!(matches!(
        expired_verifier.verify(&expired_message, NOW),
        Err(ChatSigningError::TimestampExpired { .. })
    ));

    let mut future_signer = signing_session();
    let future = future_signer
        .sign("future", 1, NOW + 3 * 60 * 1000)
        .unwrap();
    let mut future_verifier = remote(&future_signer, false);
    assert!(matches!(
        future_verifier.verify(&future, NOW),
        Err(ChatSigningError::TimestampInFuture { .. })
    ));

    let mut signer = signing_session();
    let mut verifier = remote(&signer, false);
    let first = signer.sign("first", 0, NOW).unwrap();
    verifier.verify(&first, NOW).unwrap();
    let mut out_of_order = signer.sign("later", 2, NOW + 1000).unwrap();
    out_of_order.body.timestamp_ms = NOW - 1;
    assert_eq!(
        verifier.verify(&out_of_order, NOW + 1000).unwrap_err(),
        ChatSigningError::TimestampOutOfOrder {
            last_ms: NOW,
            received_ms: NOW - 1
        }
    );
}

#[test]
fn unsigned_messages_are_rejected_when_a_signature_is_required() {
    let mut signer = signing_session();
    let body = SignedMessageBody::new("unsigned", NOW, 0, LastSeenMessages::empty()).unwrap();
    let unsigned = SignedMessage::new(0, SENDER, SESSION_ID, body);
    let mut verifier = remote(&signer, false);
    assert_eq!(
        verifier.verify(&unsigned, NOW).unwrap_err(),
        ChatSigningError::MissingSignature
    );

    let message = signer.sign("signed", 0, NOW).unwrap();
    let mut remote_key = message.signature.as_ref().unwrap().clone();
    remote_key.as_bytes_mut()[255] ^= 0x80;
    let mut tampered = message.clone();
    tampered.signature = Some(remote_key);
    assert_eq!(
        verifier.verify(&tampered, NOW).unwrap_err(),
        ChatSigningError::InvalidSignature
    );
}

#[test]
fn profile_key_property_round_trips_through_login_success() {
    let key_pair =
        ProfileKeyPair::deterministic_for_tests(&SENDER, SEED, NOW, 60 * 60 * 1000).unwrap();
    let property = key_pair.property();
    assert_eq!(property.name, "profile_key");

    let decoded = ProfilePublicKey::from_property_value(&property.value).unwrap();
    assert_eq!(&decoded, key_pair.public());
    assert_eq!(decoded.validator(), key_pair.public().key.clone());
    decoded
        .validate(&SENDER, &key_pair.public().key, NOW)
        .unwrap();

    // A key signed for another profile id must not validate.
    assert!(matches!(
        decoded.validate(&[0x44; 16], &key_pair.public().key, NOW),
        Err(ChatSigningError::InvalidProfileKey(_))
    ));

    let profile = key_pair.login_profile(SENDER, "Steve");
    let wire = build_login_success(&profile).unwrap();
    let parsed = parse_login_success(&wire).unwrap();
    assert_eq!(parsed.uuid, SENDER);
    assert_eq!(parsed.name, "Steve");
    let parsed_property = parsed
        .properties
        .iter()
        .find(|property| property.name == "profile_key")
        .expect("login success carries the profile key");
    let parsed_key = ProfilePublicKey::from_property_value(&parsed_property.value).unwrap();
    assert_eq!(&parsed_key, key_pair.public());

    let mut expired = decoded;
    expired.expires_at_ms = NOW - 1;
    assert_eq!(
        expired.validate(&SENDER, &key_pair.public().key, NOW),
        Err(ChatSigningError::ExpiredProfileKey)
    );
}

#[test]
fn enforced_secure_chat_rejects_the_deterministic_test_key() {
    let signer = signing_session();
    assert!(matches!(
        RemoteChatSession::new(
            SENDER,
            SESSION_ID,
            signer.public_key().clone(),
            NOW,
            true,
            None
        ),
        Err(ChatSigningError::InsecureAlgorithm(_))
    ));
}

#[test]
fn a_too_small_rsa_key_fails_loudly_instead_of_claiming_secure_chat() {
    let rsa = crypto::RsaKeyPair::generate().unwrap();
    let key_pair =
        ProfileKeyPair::from_signing_key(ChatSigningKey::Rsa(rsa), &SENDER, NOW, 60 * 60 * 1000)
            .unwrap();
    assert!(!key_pair.is_secure());
    key_pair
        .public()
        .validate(&SENDER, &key_pair.public().key, NOW)
        .unwrap();
    let mut signer =
        ChatSigningSession::from_key_pair(SENDER, SESSION_ID, key_pair.clone(), NOW).unwrap();
    assert!(matches!(
        signer.sign("too small", 0, NOW),
        Err(ChatSigningError::InsecureAlgorithm(_))
    ));
    assert!(matches!(
        RemoteChatSession::new(
            SENDER,
            SESSION_ID,
            key_pair.public().clone(),
            NOW,
            true,
            None
        ),
        Err(ChatSigningError::InsecureAlgorithm(_))
    ));
}

#[test]
fn profile_keys_can_be_installed_on_a_player_session() {
    let mut session = folia_core::player::PlayerSession::new("Alex".into(), SENDER, 0.0, 64.0, 0.0);
    assert!(session.chat_signing.is_none());
    assert!(session.login_profile().properties.is_empty());

    session
        .provision_test_chat_signing(SESSION_ID, SEED, NOW)
        .unwrap();
    let profile = session.login_profile();
    assert_eq!(profile.uuid, SENDER);
    assert_eq!(profile.name, "Alex");
    assert_eq!(profile.properties.len(), 1);
    assert_eq!(profile.properties[0].name, "profile_key");
    assert_eq!(
        session.chat_signing.as_ref().unwrap().session_id(),
        &SESSION_ID
    );
}

#[test]
fn signed_body_wire_layout_is_exact() {
    let body = SignedMessageBody::new(
        "hello",
        1_700_000_000_123,
        0x0102_0304_0506_0708,
        LastSeenMessages::empty(),
    )
    .unwrap();
    let mut writer = folia_core::protocol::PacketWriter::new();
    body.write_packed(&mut writer, &MessageSignatureCache::default())
        .unwrap();
    let bytes = writer.into_inner();

    let mut expected = Vec::new();
    expected.extend_from_slice(&[5]); // VarInt string length
    expected.extend_from_slice(b"hello");
    expected.extend_from_slice(&1_700_000_000_123i64.to_be_bytes());
    expected.extend_from_slice(&0x0102_0304_0506_0708i64.to_be_bytes());
    expected.push(0); // empty last-seen collection
    assert_eq!(bytes, expected);

    let mut reader = folia_core::protocol::PacketReader::new(&bytes);
    let decoded =
        SignedMessageBody::read_packed(&mut reader, &MessageSignatureCache::default()).unwrap();
    assert!(reader.is_empty());
    assert_eq!(decoded, body);
}

#[test]
fn last_seen_update_uses_a_fixed_twenty_bit_bitset() {
    let update = LastSeenUpdate::new(3, 0b1000_0000_0000_0000_0101, 0xab).unwrap();
    let mut writer = folia_core::protocol::PacketWriter::new();
    update.write(&mut writer).unwrap();
    let bytes = writer.into_inner();
    assert_eq!(bytes, [3, 0x05, 0x00, 0x08, 0xab]);
    assert!(update.acknowledges(0));
    assert!(!update.acknowledges(1));
    assert!(update.acknowledges(2));
    assert!(update.acknowledges(20 - 1));

    let mut reader = folia_core::protocol::PacketReader::new(&bytes);
    assert_eq!(LastSeenUpdate::read(&mut reader).unwrap(), update);
    assert!(reader.is_empty());

    assert!(LastSeenUpdate::new(-1, 0, 0).is_err());
    assert!(LastSeenUpdate::new(0, 1 << LAST_SEEN_MESSAGES_MAX, 0).is_err());
}

#[test]
fn serverbound_chat_wire_layout_round_trips_with_and_without_signature() {
    let chat = ServerboundChat {
        message: "hello".into(),
        timestamp_ms: NOW,
        salt: -5,
        signature: Some(signature(0x7f)),
        last_seen: LastSeenUpdate::new(0, 0b101, 0).unwrap(),
    };
    let bytes = build_serverbound_chat(&chat).unwrap();
    assert_eq!(bytes[0], SERVERBOUND_CHAT as u8);
    // VarInt prefix is not the packet payload; the first payload byte is the id.
    assert_eq!(parse_serverbound_chat(&bytes).unwrap(), chat);

    let unsigned = ServerboundChat {
        signature: None,
        ..chat.clone()
    };
    let bytes = build_serverbound_chat(&unsigned).unwrap();
    assert_eq!(parse_serverbound_chat(&bytes).unwrap(), unsigned);

    let mut trailing = bytes;
    trailing.push(0);
    assert!(parse_serverbound_chat(&trailing).is_err());
    assert!(parse_serverbound_chat(&[0]).is_err());
}

#[test]
fn signed_message_payload_matches_player_chat_message_layout() {
    let mut session = signing_session();
    let message = session.sign("abc", 0x1122_3344_5566_7788, NOW).unwrap();
    let payload = message.signature_payload();

    let mut expected = Vec::new();
    expected.extend_from_slice(&1i32.to_be_bytes()); // PlayerChatMessage marker
    expected.extend_from_slice(&SENDER);
    expected.extend_from_slice(&SESSION_ID);
    expected.extend_from_slice(&0i32.to_be_bytes()); // link index / sequence
    expected.extend_from_slice(&0x1122_3344_5566_7788i64.to_be_bytes());
    expected.extend_from_slice(&NOW.div_euclid(1000).to_be_bytes());
    expected.extend_from_slice(&3i32.to_be_bytes());
    expected.extend_from_slice(b"abc");
    expected.extend_from_slice(&0i32.to_be_bytes()); // last-seen count
    assert_eq!(payload, expected);

    assert!(message.verify(&session.public_key().key));
    assert!(message.has_signature());
}

#[test]
fn last_seen_messages_round_trip_with_a_signature_cache() {
    let first = signature(1);
    let second = signature(2);
    let mut cache = MessageSignatureCache::default();
    cache.push(
        &SignedMessageBody::new("x", NOW, 0, LastSeenMessages::empty()).unwrap(),
        Some(&first),
    );
    let messages = LastSeenMessages::new(vec![first.clone(), second.clone()]).unwrap();
    let mut writer = folia_core::protocol::PacketWriter::new();
    messages.write_packed(&mut writer, &cache).unwrap();
    let bytes = writer.into_inner();

    let mut reader = folia_core::protocol::PacketReader::new(&bytes);
    let decoded = LastSeenMessages::read_packed(&mut reader, &cache).unwrap();
    assert_eq!(decoded, messages);

    assert!(LastSeenMessages::new(vec![signature(1); LAST_SEEN_MESSAGES_MAX + 1]).is_err());
    assert_eq!(LastSeenMessages::empty().compute_checksum(), 1);
    assert_ne!(
        LastSeenMessages::new(vec![first])
            .unwrap()
            .compute_checksum(),
        1
    );
}

#[test]
#[ignore]
fn benchmark_secure_profile_key_generation() {
    let start = std::time::Instant::now();
    let key_pair = ProfileKeyPair::generate_secure(&SENDER, NOW, 60_000).unwrap();
    eprintln!("2048-bit profile key generation: {:?}", start.elapsed());
    let mut signer =
        ChatSigningSession::from_key_pair(SENDER, SESSION_ID, key_pair.clone(), NOW).unwrap();
    let message = signer.sign("secure", 0, NOW).unwrap();
    assert!(message.verify(&key_pair.public().key));
    assert!(key_pair.is_secure());
    key_pair
        .public()
        .validate(&SENDER, &key_pair.public().key, NOW)
        .unwrap();
}

#[test]
fn config_parses_enforces_secure_chat_with_false_default() {
    assert!(!ServerProperties::default().enforces_secure_chat);
    assert!(!ServerProperties::parse("").enforces_secure_chat);
    assert!(ServerProperties::parse("enforces-secure-chat=true\n").enforces_secure_chat);
    assert!(!ServerProperties::parse("enforces-secure-chat=false\n").enforces_secure_chat);
    assert!(ServerProperties::parse("enforce-secure-profile=true\n").enforces_secure_chat);
}

fn last_seen_update() -> LastSeenUpdate {
    let acknowledged = LastSeenMessages::new(vec![signature(0x10), signature(0x11)]).unwrap();
    LastSeenUpdate::new(2, 0b11, acknowledged.compute_checksum()).unwrap()
}

fn unsigned_command() -> ServerboundChatCommand {
    ServerboundChatCommand {
        command: "say hello".into(),
        timestamp_ms: NOW,
        salt: 42,
        last_seen: last_seen_update(),
    }
}

fn signed_command() -> ServerboundChatCommandSigned {
    let sig_a = MessageSignature::from_slice(&[0xAA; MESSAGE_SIGNATURE_BYTES]).unwrap();
    let sig_b = MessageSignature::from_slice(&[0xBB; MESSAGE_SIGNATURE_BYTES]).unwrap();
    ServerboundChatCommandSigned {
        command: "msg Steve hi".into(),
        timestamp_ms: NOW,
        salt: -7,
        argument_signatures: vec![
            SignedCommandArgument {
                name: "target".into(),
                signature: sig_a,
            },
            SignedCommandArgument {
                name: "message".into(),
                signature: sig_b,
            },
        ],
        last_seen: last_seen_update(),
    }
}

#[test]
fn serverbound_chat_command_round_trips_with_last_seen() {
    let command = unsigned_command();
    let bytes = build_serverbound_chat_command(&command).unwrap();
    assert_eq!(bytes[0], SERVERBOUND_CHAT_COMMAND as u8);
    assert_eq!(parse_serverbound_chat_command(&bytes).unwrap(), command);
}

#[test]
fn serverbound_chat_command_signed_round_trips_preserving_order_and_bytes() {
    let command = signed_command();
    let bytes = build_serverbound_chat_command_signed(&command).unwrap();
    assert_eq!(bytes[0], SERVERBOUND_CHAT_COMMAND_SIGNED as u8);
    let parsed = parse_serverbound_chat_command_signed(&bytes).unwrap();
    assert_eq!(parsed, command);
    assert_eq!(parsed.argument_signatures[0].name, "target");
    assert_eq!(
        parsed.argument_signatures[0].signature.as_bytes(),
        &[0xAA; MESSAGE_SIGNATURE_BYTES]
    );
    assert_eq!(parsed.argument_signatures[1].name, "message");
    assert_eq!(
        parsed.argument_signatures[1].signature.as_bytes(),
        &[0xBB; MESSAGE_SIGNATURE_BYTES]
    );
}

#[test]
fn chat_command_rejects_wrong_packet_id() {
    let mut bytes = build_serverbound_chat_command(&unsigned_command()).unwrap();
    bytes[0] = SERVERBOUND_CHAT_COMMAND_SIGNED as u8;
    assert!(parse_serverbound_chat_command(&bytes).is_err());

    let mut bytes = build_serverbound_chat_command_signed(&signed_command()).unwrap();
    bytes[0] = SERVERBOUND_CHAT_COMMAND as u8;
    assert!(parse_serverbound_chat_command_signed(&bytes).is_err());
}

#[test]
fn chat_command_rejects_empty_command() {
    let mut command = unsigned_command();
    command.command.clear();
    assert!(build_serverbound_chat_command(&command).is_err());

    let mut signed = signed_command();
    signed.command.clear();
    assert!(build_serverbound_chat_command_signed(&signed).is_err());
}

#[test]
fn chat_command_rejects_command_longer_than_max_length() {
    let long = "a".repeat(257);
    let mut command = unsigned_command();
    command.command = long.clone();
    assert!(build_serverbound_chat_command(&command).is_err());

    let mut signed = signed_command();
    signed.command = long;
    assert!(build_serverbound_chat_command_signed(&signed).is_err());
}

#[test]
fn signed_chat_command_rejects_negative_argument_count() {
    let bytes = build_serverbound_chat_command_signed(&signed_command()).unwrap();
    let mut tampered = vec![SERVERBOUND_CHAT_COMMAND_SIGNED as u8];
    // command "msg Steve hi" (12 chars)
    tampered.push(12);
    tampered.extend_from_slice(b"msg Steve hi");
    tampered.extend_from_slice(&NOW.to_be_bytes());
    tampered.extend_from_slice(&(-7i64).to_be_bytes());
    tampered.push(0xFF); // VarInt -1
    tampered.extend_from_slice(&bytes[bytes.len() - 5..]); // last_seen
    assert!(parse_serverbound_chat_command_signed(&tampered).is_err());
}

#[test]
fn signed_chat_command_rejects_too_many_arguments() {
    let mut command = signed_command();
    command.argument_signatures = (0..9)
        .map(|i| SignedCommandArgument {
            name: format!("a{i}"),
            signature: MessageSignature::from_slice(&[i as u8; MESSAGE_SIGNATURE_BYTES]).unwrap(),
        })
        .collect();
    assert!(build_serverbound_chat_command_signed(&command).is_err());

    // Also ensure parse rejects 9 encoded arguments
    let ok = signed_command();
    let bytes = build_serverbound_chat_command_signed(&ok).unwrap();
    let mut tampered = Vec::new();
    tampered.push(SERVERBOUND_CHAT_COMMAND_SIGNED as u8);
    tampered.push(12);
    tampered.extend_from_slice(b"msg Steve hi");
    tampered.extend_from_slice(&NOW.to_be_bytes());
    tampered.extend_from_slice(&(-7i64).to_be_bytes());
    tampered.push(9); // count
    for i in 0..9u8 {
        let name = format!("a{i}");
        tampered.push(name.len() as u8);
        tampered.extend_from_slice(name.as_bytes());
        tampered.extend_from_slice(&[i; MESSAGE_SIGNATURE_BYTES]);
    }
    // last_seen
    let last_seen = last_seen_update();
    tampered.push(last_seen.offset as u8);
    let ack = last_seen.acknowledged.to_le_bytes();
    tampered.extend_from_slice(&ack[..3]);
    tampered.push(last_seen.checksum);
    assert!(parse_serverbound_chat_command_signed(&tampered).is_err());
}

#[test]
fn signed_chat_command_rejects_empty_argument_name() {
    let mut command = signed_command();
    command.argument_signatures[0].name.clear();
    assert!(build_serverbound_chat_command_signed(&command).is_err());
}

#[test]
fn signed_chat_command_rejects_duplicate_argument_names() {
    let mut command = signed_command();
    command.argument_signatures[1].name = "target".into();
    assert!(build_serverbound_chat_command_signed(&command).is_err());
}

#[test]
fn chat_command_rejects_trailing_bytes() {
    let mut bytes = build_serverbound_chat_command(&unsigned_command()).unwrap();
    bytes.push(0);
    assert!(parse_serverbound_chat_command(&bytes).is_err());

    let mut bytes = build_serverbound_chat_command_signed(&signed_command()).unwrap();
    bytes.push(0);
    assert!(parse_serverbound_chat_command_signed(&bytes).is_err());
}
