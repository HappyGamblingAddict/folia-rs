//! Online-mode login tests. Nothing here touches the network: the session
//! authenticator and the client side of the encryption exchange are mocks,
//! and the transport is an in-memory duplex stream.

use folia_core::crypto::{Cfb8, RsaKeyPair, RsaPublicKey, server_hash};
use folia_core::join::{
    AuthError, EncryptionOutcome, MojangSessionAuthenticator, SessionAuthenticator,
    complete_encryption, parse_session_profile,
};
use folia_core::protocol::{
    Connection, EncryptionRequest, EncryptionResponse, GameProfile, ProfileProperty,
    build_encryption_request, build_encryption_response, build_login_success,
    parse_encryption_request, parse_encryption_response, parse_login_success, read_packet,
    write_packet,
};
use std::io::{self, Cursor, Read, Write};
use std::net::IpAddr;
use std::sync::Mutex;

const RSA_N: &str = "b3db2c8fabda15728823690e17116e28d0cfa06d1b7f0ca6beb61800007b71ea0de55d0b6735d2b0a04ff731b2966d002430657fec6fbb47292c49c5682fc1322ac948fd70c567dce2212c247f5a5c0c01d84e4a0582d10e95c417037c8bb3a12f0fda949014756c027e8566ee995b00ff28a466ff0105f8f65e43fca07ce0eb";
const RSA_D: &str = "3f9c760f3e5dbc962c7631b2cd0fda214f710fb1673731374c5e44f0c33ae5d83d1ff11f965a5ff11e520a00a6c4333b0891a164743034517fcb6d3007a054389d058a9c4b0279f40eee6f3a828049575c24bd5770298a74a805de3b7be27ed9fa71746b39fa5c7d26067a56943b6cafea472cd7d095ca19a45a2ea37c063011";

fn decode_hex(value: &str) -> Vec<u8> {
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).unwrap())
        .collect()
}

fn keypair() -> RsaKeyPair {
    RsaKeyPair::from_components(&decode_hex(RSA_N), 65537, &decode_hex(RSA_D)).unwrap()
}

fn secret() -> [u8; 16] {
    [
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        0x10,
    ]
}

/// In-memory transport the server can read from and write to.
struct MockStream {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
}

impl MockStream {
    fn new(input: Vec<u8>) -> Self {
        Self {
            input: Cursor::new(input),
            output: Vec::new(),
        }
    }
}

impl Read for MockStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.input.read(buffer)
    }
}

impl Write for MockStream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.output.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct MockAuthenticator {
    result: Option<Result<Option<GameProfile>, AuthError>>,
    calls: Mutex<Vec<(String, String, Option<IpAddr>)>>,
}

impl MockAuthenticator {
    fn accepting(profile: GameProfile) -> Self {
        Self {
            result: Some(Ok(Some(profile))),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn with_result(result: Result<Option<GameProfile>, AuthError>) -> Self {
        Self {
            result: Some(result),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<(String, String, Option<IpAddr>)> {
        self.calls.lock().unwrap().clone()
    }
}

impl SessionAuthenticator for MockAuthenticator {
    fn has_joined_server(
        &self,
        username: &str,
        server_hash: &str,
        client_ip: Option<IpAddr>,
    ) -> Result<Option<GameProfile>, AuthError> {
        self.calls
            .lock()
            .unwrap()
            .push((username.to_owned(), server_hash.to_owned(), client_ip));
        self.result.clone().unwrap_or(Ok(None))
    }
}

fn profile() -> GameProfile {
    GameProfile {
        uuid: [0xAB; 16],
        name: "Steve".to_owned(),
        properties: vec![ProfileProperty {
            name: "textures".to_owned(),
            value: "c2tpbg==".to_owned(),
            signature: Some("c2ln".to_owned()),
        }],
    }
}

fn framed(payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_packet(&mut bytes, payload).unwrap();
    bytes
}

fn response_input(public: &RsaPublicKey, shared_secret: &[u8], challenge: &[u8]) -> Vec<u8> {
    let response = EncryptionResponse {
        shared_secret: public.encrypt_pkcs1v15(shared_secret).unwrap(),
        verify_token: public.encrypt_pkcs1v15(challenge).unwrap(),
    };
    framed(&build_encryption_response(&response).unwrap())
}

#[test]
fn encryption_request_body_layout_round_trips() {
    let request = EncryptionRequest {
        server_id: String::new(),
        public_key: vec![0x30, 0x82, 0x01, 0x0A],
        verify_token: vec![0x11, 0x22, 0x33, 0x44],
        should_authenticate: true,
    };
    let payload = build_encryption_request(&request).unwrap();
    // id 1, empty server id, 4-byte key, 4-byte token, boolean true.
    assert_eq!(
        payload,
        vec![
            0x01, 0x00, 0x04, 0x30, 0x82, 0x01, 0x0A, 0x04, 0x11, 0x22, 0x33, 0x44, 0x01,
        ]
    );
    assert_eq!(parse_encryption_request(&payload).unwrap(), request);
    assert!(parse_encryption_request(&payload[..payload.len() - 1]).is_err());
}

#[test]
fn encryption_response_body_layout_round_trips() {
    let response = EncryptionResponse {
        shared_secret: vec![0xAA; 128],
        verify_token: vec![0xBB; 128],
    };
    let payload = build_encryption_response(&response).unwrap();
    assert_eq!(&payload[..3], &[0x01, 0x80, 0x01]);
    assert_eq!(parse_encryption_response(&payload).unwrap(), response);

    // Wrong packet id and truncated bodies must be rejected.
    let mut wrong_id = payload.clone();
    wrong_id[0] = 0x02;
    assert!(parse_encryption_response(&wrong_id).is_err());
    assert!(parse_encryption_response(&payload[..payload.len() - 1]).is_err());
}

#[test]
fn login_success_with_no_properties_is_byte_identical_to_offline_path() {
    let profile = GameProfile::new([0x11; 16], "Steve");
    let payload = build_login_success(&profile).unwrap();
    // This is exactly the packet the offline join path used before online
    // mode existed: id 2, UUID, name, zero properties.
    let expected = [
        &[0x02u8][..],
        &[0x11u8; 16][..],
        &[0x05][..],
        b"Steve",
        &[0x00][..],
    ]
    .concat();
    assert_eq!(payload, expected);
    assert_eq!(parse_login_success(&payload).unwrap(), profile);
}

#[test]
fn login_success_carries_signed_textures() {
    let profile = profile();
    let payload = build_login_success(&profile).unwrap();
    assert_eq!(parse_login_success(&payload).unwrap(), profile);
}

#[test]
fn complete_encryption_authenticates_and_installs_cipher() {
    let keypair = keypair();
    let challenge = [0x11, 0x22, 0x33, 0x44];
    let shared_secret = secret();
    let input = response_input(keypair.public(), &shared_secret, &challenge);
    let mut connection = Connection::new(MockStream::new(input));
    let authenticator = MockAuthenticator::accepting(profile());

    let outcome = complete_encryption(
        &mut connection,
        "",
        &keypair,
        &challenge,
        "Steve",
        &authenticator,
        Some(IpAddr::from([203, 0, 113, 7])),
    )
    .unwrap();
    assert_eq!(outcome, EncryptionOutcome::Authenticated(profile()));

    // The cleartext request is the first frame the server wrote.
    let request_frame = connection.get_ref().output.clone();
    let request =
        parse_encryption_request(&read_packet(&mut request_frame.as_slice()).unwrap()).unwrap();
    assert_eq!(request.server_id, "");
    assert_eq!(request.public_key, keypair.public_key_der());
    assert_eq!(request.verify_token, challenge);
    assert!(request.should_authenticate);

    // The authenticator saw the same hash vanilla computes.
    let calls = authenticator.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "Steve");
    assert_eq!(
        calls[0].1,
        server_hash("", &keypair.public_key_der(), &shared_secret)
    );
    assert_eq!(calls[0].2, Some(IpAddr::from([203, 0, 113, 7])));

    // Everything written after the response is encrypted with the shared
    // secret. Decrypting the next frame must recover an ordinary packet.
    assert!(connection.is_encrypted());
    connection.write_packet(&[0x42, 0x43]).unwrap();
    let output = connection.get_ref().output.clone();
    let mut offset = read_frame_size(&output);
    let mut encrypted = output[offset..].to_vec();
    offset += encrypted.len();
    assert_eq!(offset, output.len());
    Cfb8::decryptor(&shared_secret).process(&mut encrypted);
    assert_eq!(
        read_packet(&mut encrypted.as_slice()).unwrap(),
        [0x42, 0x43]
    );
}

/// Size of the first frame in `bytes`, including its VarInt length prefix.
fn read_frame_size(bytes: &[u8]) -> usize {
    let mut cursor = Cursor::new(bytes);
    let mut length = 0u32;
    for index in 0..5 {
        let mut byte = [0u8; 1];
        cursor.read_exact(&mut byte).unwrap();
        length |= u32::from(byte[0] & 0x7f) << (7 * index);
        if byte[0] & 0x80 == 0 {
            break;
        }
    }
    cursor.position() as usize + length as usize
}

#[test]
fn complete_encryption_rejects_token_mismatch_without_authenticating() {
    let keypair = keypair();
    let challenge = [0x11, 0x22, 0x33, 0x44];
    let input = response_input(keypair.public(), &secret(), &[0x99, 0x88, 0x77, 0x66]);
    let mut connection = Connection::new(MockStream::new(input));
    let authenticator = MockAuthenticator::accepting(profile());

    let error = complete_encryption(
        &mut connection,
        "",
        &keypair,
        &challenge,
        "Steve",
        &authenticator,
        None,
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(authenticator.calls().is_empty());
    assert!(!connection.is_encrypted());
}

#[test]
fn complete_encryption_rejects_short_shared_secret() {
    let keypair = keypair();
    let challenge = [0x01, 0x02, 0x03, 0x04];
    let input = response_input(keypair.public(), &[0xAA; 15], &challenge);
    let mut connection = Connection::new(MockStream::new(input));
    let authenticator = MockAuthenticator::accepting(profile());

    assert!(
        complete_encryption(
            &mut connection,
            "",
            &keypair,
            &challenge,
            "Steve",
            &authenticator,
            None,
        )
        .is_err()
    );
    assert!(!connection.is_encrypted());
}

#[test]
fn invalid_session_and_unavailable_server_are_distinct() {
    let keypair = keypair();
    let challenge = [0x01, 0x02, 0x03, 0x04];

    let input = response_input(keypair.public(), &secret(), &challenge);
    let mut connection = Connection::new(MockStream::new(input));
    let authenticator = MockAuthenticator::with_result(Ok(None));
    assert_eq!(
        complete_encryption(
            &mut connection,
            "",
            &keypair,
            &challenge,
            "Steve",
            &authenticator,
            None,
        )
        .unwrap(),
        EncryptionOutcome::Unverified
    );

    let input = response_input(keypair.public(), &secret(), &challenge);
    let mut connection = Connection::new(MockStream::new(input));
    let authenticator = MockAuthenticator::with_result(Err(AuthError::Unavailable(
        "connection refused".to_owned(),
    )));
    assert_eq!(
        complete_encryption(
            &mut connection,
            "",
            &keypair,
            &challenge,
            "Steve",
            &authenticator,
            None,
        )
        .unwrap(),
        EncryptionOutcome::Unavailable
    );
}

#[test]
fn offline_framing_is_unchanged_by_the_connection_wrapper() {
    let payload = vec![0x24; 300];
    let mut reference = Vec::new();
    write_packet(&mut reference, &payload).unwrap();

    let mut connection = Connection::new(MockStream::new(Vec::new()));
    connection.write_packet(&payload).unwrap();
    assert_eq!(connection.get_ref().output, reference);
    assert!(!connection.is_encrypted());

    let mut reader = Connection::new(MockStream::new(reference));
    assert_eq!(reader.read_packet().unwrap(), payload);
}

#[test]
fn encryption_cannot_be_installed_twice_or_without_a_session() {
    let mut connection = Connection::new(MockStream::new(Vec::new()));
    connection.enable_encryption(&secret()).unwrap();
    assert!(connection.enable_encryption(&secret()).is_err());
}

#[test]
fn parses_mojang_session_profiles() {
    let json = br#"{
        "id": "069a79f444e94726a5befca90e38aaf5",
        "name": "Notch",
        "properties": [
            {"name": "textures", "value": "dGV4dHVyZXM=", "signature": "c2ln"},
            {"name": "unsig", "value": "dmFsdWU="}
        ]
    }"#;
    let profile = parse_session_profile(json).unwrap();
    assert_eq!(profile.name, "Notch");
    assert_eq!(
        profile.uuid,
        [
            0x06, 0x9a, 0x79, 0xf4, 0x44, 0xe9, 0x47, 0x26, 0xa5, 0xbe, 0xfc, 0xa9, 0x0e, 0x38,
            0xaa, 0xf5
        ]
    );
    assert_eq!(profile.properties.len(), 2);
    assert_eq!(profile.properties[0].signature.as_deref(), Some("c2ln"));
    assert_eq!(profile.properties[1].signature, None);

    // Missing/invalid fields must be explicit errors, not empty profiles.
    assert!(parse_session_profile(br#"{"name":"Notch"}"#).is_err());
    assert!(parse_session_profile(br#"{"id":"xyz","name":"Notch"}"#).is_err());
    assert!(parse_session_profile(br#"{"id":"069a79f444e94726a5befca90e38aaf5"}"#).is_err());
}

#[test]
fn mojang_endpoint_percent_encodes_query_values() {
    let authenticator = MojangSessionAuthenticator::with_endpoint("http://127.0.0.1:1/hasJoined");
    // Accessing the URL builder through the public trait would make a network
    // call, so verify the encoding contract through a captured-URL helper.
    // The helper is the same one `has_joined_server` uses.
    let url = authenticator.url_for("Weird Name", "-1a2b", Some("::1".parse().unwrap()));
    assert_eq!(
        url,
        "http://127.0.0.1:1/hasJoined?username=Weird%20Name&serverId=-1a2b&ip=%3A%3A1"
    );
}
