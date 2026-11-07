//! Cryptographic primitives for the Minecraft 1.21.11 online-mode login.
//!
//! Vanilla uses a 1024-bit RSA key pair generated at startup, PKCS#1 v1.5
//! encryption for the shared secret and verify token, AES-128/CFB8 with the
//! key as IV for the connection, and a SHA-1 server hash rendered through
//! `java.math.BigInteger.toString(16)`.
//!
//! The workspace intentionally has no crypto dependencies, so the primitives
//! live here: AES-128, CFB8, SHA-1, DER (SubjectPublicKeyInfo), PKCS#1 v1.5,
//! and a small big-integer/Montgomery implementation for RSA. Every primitive
//! is checked against Java/JCE fixtures in `tests/crypto.rs`; that is the
//! differential oracle for this module.

use std::cmp::Ordering;
use std::fmt;

/// AES-128 key size used by the Minecraft protocol.
pub const AES_KEY_SIZE: usize = 16;
/// RSA key size used by vanilla (`Crypt.ASYMMETRIC_BITS`).
pub const RSA_KEY_BITS: usize = 1024;
/// Default public exponent used by Java's `KeyPairGenerator` for RSA.
const RSA_PUBLIC_EXPONENT: u32 = 65537;
/// SHA-1 digest size.
pub const SHA1_DIGEST_SIZE: usize = 20;
/// SHA-256 digest size used by chat/profile-key signatures.
pub const SHA256_DIGEST_SIZE: usize = 32;
/// RSA key size used for server-issued 1.21.11 chat profile keys. Chat message
/// signatures are fixed at 256 bytes, which requires a 2048-bit modulus.
pub const PROFILE_KEY_BITS: usize = 2048;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    EntropyUnavailable,
    KeyGenerationFailed,
    InvalidKey,
    InvalidCiphertext,
    InvalidPadding,
    InvalidDer,
}

impl fmt::Display for CryptoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CryptoError::EntropyUnavailable => {
                formatter.write_str("no operating-system randomness source is available")
            }
            CryptoError::KeyGenerationFailed => {
                formatter.write_str("RSA key generation did not converge")
            }
            CryptoError::InvalidKey => formatter.write_str("invalid RSA key"),
            CryptoError::InvalidCiphertext => formatter.write_str("invalid RSA ciphertext"),
            CryptoError::InvalidPadding => formatter.write_str("invalid PKCS#1 v1.5 padding"),
            CryptoError::InvalidDer => formatter.write_str("invalid DER encoding"),
        }
    }
}

impl std::error::Error for CryptoError {}

/// Fill `buffer` with bytes from the operating system's CSPRNG.
///
/// Windows uses `BCryptGenRandom` from `bcrypt.dll`; Unix reads
/// `/dev/urandom`. Platforms without either fail loudly instead of silently
/// generating predictable key material.
pub fn random_bytes(buffer: &mut [u8]) -> Result<(), CryptoError> {
    if buffer.is_empty() {
        return Ok(());
    }
    if os_random(buffer) {
        Ok(())
    } else {
        Err(CryptoError::EntropyUnavailable)
    }
}

#[cfg(windows)]
fn os_random(buffer: &mut [u8]) -> bool {
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut core::ffi::c_void,
            buffer: *mut u8,
            length: u32,
            flags: u32,
        ) -> i32;
    }
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    let Ok(length) = u32::try_from(buffer.len()) else {
        return false;
    };
    // SAFETY: `buffer` is a valid writable slice of `length` bytes and the
    // system-preferred RNG accepts a null algorithm handle.
    unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            buffer.as_mut_ptr(),
            length,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        ) == 0
    }
}

#[cfg(unix)]
fn os_random(buffer: &mut [u8]) -> bool {
    use std::io::Read;
    match std::fs::File::open("/dev/urandom") {
        Ok(mut file) => file.read_exact(buffer).is_ok(),
        Err(_) => false,
    }
}

#[cfg(not(any(windows, unix)))]
fn os_random(_buffer: &mut [u8]) -> bool {
    false
}

/// Constant-time byte comparison. Length is allowed to leak (all callers use
/// fixed-size protocol fields); the contents are compared without an early
/// exit.
pub fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in left.iter().zip(right) {
        difference |= a ^ b;
    }
    difference == 0
}

// ===========================================================================
// SHA-1
// ===========================================================================

/// Incremental SHA-1 (`java.security.MessageDigest.getInstance("SHA-1")`).
#[derive(Clone)]
pub struct Sha1 {
    state: [u32; 5],
    buffer: [u8; 64],
    buffered: usize,
    length: u64,
}

impl Default for Sha1 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha1 {
    pub fn new() -> Self {
        Self {
            state: [
                0x6745_2301,
                0xEFCD_AB89,
                0x98BA_DCFE,
                0x1032_5476,
                0xC3D2_E1F0,
            ],
            buffer: [0u8; 64],
            buffered: 0,
            length: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        if self.buffered > 0 {
            let needed = 64 - self.buffered;
            let take = needed.min(data.len());
            self.buffer[self.buffered..self.buffered + take].copy_from_slice(&data[..take]);
            self.buffered += take;
            data = &data[take..];
            if self.buffered == 64 {
                let block = self.buffer;
                self.compress(&block);
                self.buffered = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            self.compress(&block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.buffered = data.len();
        }
    }

    pub fn finish(mut self) -> [u8; SHA1_DIGEST_SIZE] {
        let bit_length = self.length.wrapping_mul(8);
        self.update(&[0x80]);
        while self.buffered != 56 {
            self.update(&[0x00]);
        }
        self.update(&bit_length.to_be_bytes());
        let mut digest = [0u8; SHA1_DIGEST_SIZE];
        for (chunk, word) in digest.chunks_exact_mut(4).zip(self.state) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }
        digest
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut words = [0u32; 80];
        for (index, chunk) in block.chunks_exact(4).enumerate() {
            words[index] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for index in 16..80 {
            words[index] =
                (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16])
                    .rotate_left(1);
        }

        let [mut a, mut b, mut c, mut d, mut e] = self.state;
        for (index, word) in words.iter().enumerate() {
            let (function, constant) = match index {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temporary = a
                .rotate_left(5)
                .wrapping_add(function)
                .wrapping_add(e)
                .wrapping_add(constant)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temporary;
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
    }
}

pub fn sha1(data: &[u8]) -> [u8; SHA1_DIGEST_SIZE] {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hasher.finish()
}

// ===========================================================================
// SHA-256
// ===========================================================================

#[rustfmt::skip]
const SHA256_K: [u32; 64] = [
    0x428a_2f98, 0x7137_4491, 0xb5c0_fbcf, 0xe9b5_dba5, 0x3956_c25b, 0x59f1_11f1, 0x923f_82a4, 0xab1c_5ed5,
    0xd807_aa98, 0x1283_5b01, 0x2431_85be, 0x550c_7dc3, 0x72be_5d74, 0x80de_b1fe, 0x9bdc_06a7, 0xc19b_f174,
    0xe49b_69c1, 0xefbe_4786, 0x0fc1_9dc6, 0x240c_a1cc, 0x2de9_2c6f, 0x4a74_84aa, 0x5cb0_a9dc, 0x76f9_88da,
    0x983e_5152, 0xa831_c66d, 0xb003_27c8, 0xbf59_7fc7, 0xc6e0_0bf3, 0xd5a7_9147, 0x06ca_6351, 0x1429_2967,
    0x27b7_0a85, 0x2e1b_2138, 0x4d2c_6dfc, 0x5338_0d13, 0x650a_7354, 0x766a_0abb, 0x81c2_c92e, 0x9272_2c85,
    0xa2bf_e8a1, 0xa81a_664b, 0xc24b_8b70, 0xc76c_51a3, 0xd192_e819, 0xd699_0624, 0xf40e_3585, 0x106a_a070,
    0x19a4_c116, 0x1e37_6c08, 0x2748_774c, 0x34b0_bcb5, 0x391c_0cb3, 0x4ed8_aa4a, 0x5b9c_ca4f, 0x682e_6ff3,
    0x748f_82ee, 0x78a5_636f, 0x84c8_7814, 0x8cc7_0208, 0x90be_fffa, 0xa450_6ceb, 0xbef9_a3f7, 0xc671_78f2,
];

/// Incremental SHA-256 (`java.security.MessageDigest.getInstance("SHA-256")`).
#[derive(Clone)]
pub struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    buffered: usize,
    length: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        Self {
            state: [
                0x6a09_e667,
                0xbb67_ae85,
                0x3c6e_f372,
                0xa54f_f53a,
                0x510e_527f,
                0x9b05_688c,
                0x1f83_d9ab,
                0x5be0_cd19,
            ],
            buffer: [0u8; 64],
            buffered: 0,
            length: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        if self.buffered > 0 {
            let needed = 64 - self.buffered;
            let take = needed.min(data.len());
            self.buffer[self.buffered..self.buffered + take].copy_from_slice(&data[..take]);
            self.buffered += take;
            data = &data[take..];
            if self.buffered == 64 {
                let block = self.buffer;
                self.compress(&block);
                self.buffered = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            self.compress(&block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.buffered = data.len();
        }
    }

    pub fn finish(mut self) -> [u8; SHA256_DIGEST_SIZE] {
        let bit_length = self.length.wrapping_mul(8);
        self.update(&[0x80]);
        while self.buffered != 56 {
            self.update(&[0x00]);
        }
        self.update(&bit_length.to_be_bytes());
        let mut digest = [0u8; SHA256_DIGEST_SIZE];
        for (chunk, word) in digest.chunks_exact_mut(4).zip(self.state) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }
        digest
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut words = [0u32; 64];
        for (index, chunk) in block.chunks_exact(4).enumerate() {
            words[index] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for (word, constant) in words.iter().zip(SHA256_K) {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choice = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(choice)
                .wrapping_add(constant)
                .wrapping_add(*word);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(majority);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        for (state, value) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *state = state.wrapping_add(value);
        }
    }
}

pub fn sha256(data: &[u8]) -> [u8; SHA256_DIGEST_SIZE] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finish()
}

/// Java's `new BigInteger(digest).toString(16)`.
///
/// `BigInteger(byte[])` treats the digest as a big-endian two's-complement
/// integer, so a digest whose first byte has the high bit set renders with a
/// leading `-` and the magnitude of the two's complement. Vanilla feeds this
/// string to Mojang's `hasJoinedServer`.
pub fn java_bigint_hex(digest: &[u8]) -> String {
    if digest.is_empty() {
        return "0".to_owned();
    }
    if digest[0] & 0x80 == 0 {
        let mut start = 0;
        while start < digest.len() && digest[start] == 0 {
            start += 1;
        }
        if start == digest.len() {
            return "0".to_owned();
        }
        return encode_hex_minimal(&digest[start..]);
    }

    // Negative: magnitude = two's complement of the full input width.
    let mut magnitude = digest.to_vec();
    for byte in &mut magnitude {
        *byte = !*byte;
    }
    let mut carry = 1u16;
    for byte in magnitude.iter_mut().rev() {
        let sum = u16::from(*byte) + carry;
        *byte = sum as u8;
        carry = sum >> 8;
    }
    let mut start = 0;
    while start < magnitude.len() && magnitude[start] == 0 {
        start += 1;
    }
    if start == magnitude.len() {
        return "0".to_owned();
    }
    format!("-{}", encode_hex_minimal(&magnitude[start..]))
}

/// Lowercase hex with no leading zero nibble, matching `BigInteger.toString(16)`
/// (`1` instead of `01`, `100` instead of `0100`).
fn encode_hex_minimal(bytes: &[u8]) -> String {
    let Some((&first, rest)) = bytes.split_first() else {
        return "0".to_owned();
    };
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    if first >> 4 == 0 {
        output.push(char::from(DIGITS[usize::from(first & 0x0f)]));
    } else {
        output.push(char::from(DIGITS[usize::from(first >> 4)]));
        output.push(char::from(DIGITS[usize::from(first & 0x0f)]));
    }
    for byte in rest {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

/// Vanilla's `Crypt.digestData(serverId, publicKey, secretKey)` hashed through
/// Java's signed `BigInteger` hex representation.
///
/// `server_id` is empty in 1.21.11, but it stays a parameter for fidelity.
pub fn server_hash(server_id: &str, public_key_der: &[u8], secret: &[u8; AES_KEY_SIZE]) -> String {
    let mut hasher = Sha1::new();
    // Java uses ISO-8859-1. The server id is constrained to ASCII in practice;
    // Latin-1 bytes match for every character vanilla allows.
    hasher.update(server_id.as_bytes());
    hasher.update(secret);
    hasher.update(public_key_der);
    java_bigint_hex(&hasher.finish())
}

// ===========================================================================
// AES-128 + CFB8
// ===========================================================================

#[rustfmt::skip]
const AES_SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

#[rustfmt::skip]
const AES_RCON: [u8; 11] = [
    0x00, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36,
];

/// AES-128 block cipher (encryption only; CFB8 only needs the forward
/// direction).
#[derive(Clone)]
pub struct Aes128 {
    round_keys: [[u8; 16]; 11],
}

impl Aes128 {
    pub fn new(key: &[u8; AES_KEY_SIZE]) -> Self {
        let mut words = [[0u8; 4]; 44];
        for (index, chunk) in key.chunks_exact(4).enumerate() {
            words[index].copy_from_slice(chunk);
        }
        for index in 4..44 {
            let mut temporary = words[index - 1];
            if index % 4 == 0 {
                temporary = [
                    AES_SBOX[usize::from(temporary[1])],
                    AES_SBOX[usize::from(temporary[2])],
                    AES_SBOX[usize::from(temporary[3])],
                    AES_SBOX[usize::from(temporary[0])],
                ];
                temporary[0] ^= AES_RCON[index / 4];
            }
            for byte in 0..4 {
                words[index][byte] = words[index - 4][byte] ^ temporary[byte];
            }
        }

        let mut round_keys = [[0u8; 16]; 11];
        for (round, round_key) in round_keys.iter_mut().enumerate() {
            for column in 0..4 {
                round_key[column * 4..column * 4 + 4].copy_from_slice(&words[round * 4 + column]);
            }
        }
        Self { round_keys }
    }

    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        add_round_key(block, &self.round_keys[0]);
        for round in 1..10 {
            sub_bytes(block);
            shift_rows(block);
            mix_columns(block);
            add_round_key(block, &self.round_keys[round]);
        }
        sub_bytes(block);
        shift_rows(block);
        add_round_key(block, &self.round_keys[10]);
    }
}

fn add_round_key(state: &mut [u8; 16], key: &[u8; 16]) {
    for (byte, key_byte) in state.iter_mut().zip(key) {
        *byte ^= key_byte;
    }
}

fn sub_bytes(state: &mut [u8; 16]) {
    for byte in state.iter_mut() {
        *byte = AES_SBOX[usize::from(*byte)];
    }
}

fn shift_rows(state: &mut [u8; 16]) {
    // State is stored column-major: index = row + 4 * column.
    let original = *state;
    for row in 1..4 {
        for column in 0..4 {
            state[row + 4 * column] = original[row + 4 * ((column + row) % 4)];
        }
    }
}

fn mix_columns(state: &mut [u8; 16]) {
    for column in 0..4 {
        let index = column * 4;
        let a0 = state[index];
        let a1 = state[index + 1];
        let a2 = state[index + 2];
        let a3 = state[index + 3];
        state[index] = multiply_xtime(a0) ^ (multiply_xtime(a1) ^ a1) ^ a2 ^ a3;
        state[index + 1] = a0 ^ multiply_xtime(a1) ^ (multiply_xtime(a2) ^ a2) ^ a3;
        state[index + 2] = a0 ^ a1 ^ multiply_xtime(a2) ^ (multiply_xtime(a3) ^ a3);
        state[index + 3] = (multiply_xtime(a0) ^ a0) ^ a1 ^ a2 ^ multiply_xtime(a3);
    }
}

fn multiply_xtime(value: u8) -> u8 {
    (value << 1) ^ if value & 0x80 != 0 { 0x1b } else { 0x00 }
}

/// AES-128/CFB8 stream state. Java initialises `IvParameterSpec` with the key
/// bytes, which both directions here do as well.
#[derive(Clone)]
pub struct Cfb8 {
    cipher: Aes128,
    register: [u8; 16],
    decrypt: bool,
}

impl Cfb8 {
    pub fn encryptor(key: &[u8; AES_KEY_SIZE]) -> Self {
        Self::with_iv(key, key, false)
    }

    pub fn decryptor(key: &[u8; AES_KEY_SIZE]) -> Self {
        Self::with_iv(key, key, true)
    }

    /// Explicit-IV constructor. Minecraft always uses the key as IV; this is
    /// exposed so the NIST CFB8 vector can exercise the implementation.
    pub fn with_iv(key: &[u8; AES_KEY_SIZE], iv: &[u8; AES_KEY_SIZE], decrypt: bool) -> Self {
        Self {
            cipher: Aes128::new(key),
            register: *iv,
            decrypt,
        }
    }

    /// Transform bytes in place. Encryption and decryption are byte-oriented,
    /// so callers may split the stream at arbitrary packet boundaries.
    pub fn process(&mut self, data: &mut [u8]) {
        for byte in data {
            *byte = self.process_byte(*byte);
        }
    }

    pub fn process_byte(&mut self, byte: u8) -> u8 {
        let mut block = self.register;
        self.cipher.encrypt_block(&mut block);
        let output = byte ^ block[0];
        let feedback = if self.decrypt { byte } else { output };
        self.register.copy_within(1.., 0);
        self.register[15] = feedback;
        output
    }
}

// ===========================================================================
// Big unsigned integers + Montgomery arithmetic
// ===========================================================================

#[derive(Clone, Debug, PartialEq, Eq)]
struct BigUint {
    /// Little-endian base-2^32 limbs, no trailing zero limbs.
    limbs: Vec<u32>,
}

impl BigUint {
    fn zero() -> Self {
        Self { limbs: Vec::new() }
    }

    fn one() -> Self {
        Self::from_u64(1)
    }

    fn from_u64(value: u64) -> Self {
        if value == 0 {
            return Self::zero();
        }
        let mut limbs = vec![value as u32];
        if value >> 32 != 0 {
            limbs.push((value >> 32) as u32);
        }
        Self { limbs }
    }

    fn from_be_bytes(bytes: &[u8]) -> Self {
        let mut limbs = Vec::with_capacity(bytes.len().div_ceil(4));
        let mut end = bytes.len();
        while end > 0 {
            let start = end.saturating_sub(4);
            let mut word = 0u32;
            for &byte in &bytes[start..end] {
                word = (word << 8) | u32::from(byte);
            }
            limbs.push(word);
            end = start;
        }
        Self::normalize(limbs)
    }

    fn normalize(mut limbs: Vec<u32>) -> Self {
        while limbs.last() == Some(&0) {
            limbs.pop();
        }
        Self { limbs }
    }

    fn is_zero(&self) -> bool {
        self.limbs.is_empty()
    }

    fn is_odd(&self) -> bool {
        self.limbs.first().is_some_and(|limb| limb & 1 == 1)
    }

    fn is_even(&self) -> bool {
        !self.is_odd()
    }

    fn bit_len(&self) -> usize {
        match self.limbs.last() {
            Some(&top) => self.limbs.len() * 32 - top.leading_zeros() as usize,
            None => 0,
        }
    }

    fn bit(&self, index: usize) -> bool {
        let limb = index / 32;
        limb < self.limbs.len() && (self.limbs[limb] >> (index % 32)) & 1 == 1
    }

    fn set_bit(&mut self, index: usize) {
        let limb = index / 32;
        if self.limbs.len() <= limb {
            self.limbs.resize(limb + 1, 0);
        }
        self.limbs[limb] |= 1 << (index % 32);
    }

    fn limb(&self, index: usize) -> u32 {
        self.limbs.get(index).copied().unwrap_or(0)
    }

    fn to_be_bytes(&self, length: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; length];
        for (index, byte) in bytes.iter_mut().enumerate() {
            let bit_index = (length - 1 - index) * 8;
            let limb = bit_index / 32;
            let shift = bit_index % 32;
            *byte = if limb < self.limbs.len() {
                (self.limbs[limb] >> shift) as u8
            } else {
                0
            };
        }
        bytes
    }

    fn cmp_big(&self, other: &Self) -> Ordering {
        if self.limbs.len() != other.limbs.len() {
            return self.limbs.len().cmp(&other.limbs.len());
        }
        for index in (0..self.limbs.len()).rev() {
            match self.limbs[index].cmp(&other.limbs[index]) {
                Ordering::Equal => continue,
                ordering => return ordering,
            }
        }
        Ordering::Equal
    }

    fn add(&self, other: &Self) -> Self {
        let length = self.limbs.len().max(other.limbs.len());
        let mut limbs = Vec::with_capacity(length + 1);
        let mut carry = 0u64;
        for index in 0..length {
            let sum = u64::from(self.limb(index)) + u64::from(other.limb(index)) + carry;
            limbs.push(sum as u32);
            carry = sum >> 32;
        }
        if carry != 0 {
            limbs.push(carry as u32);
        }
        Self::normalize(limbs)
    }

    /// `self - other`; the caller guarantees `self >= other`.
    fn sub(&self, other: &Self) -> Self {
        let length = self.limbs.len();
        let mut limbs = Vec::with_capacity(length);
        let mut borrow = 0i64;
        for index in 0..length {
            let difference = i64::from(self.limb(index)) - i64::from(other.limb(index)) + borrow;
            if difference < 0 {
                limbs.push((difference + (1i64 << 32)) as u32);
                borrow = -1;
            } else {
                limbs.push(difference as u32);
                borrow = 0;
            }
        }
        debug_assert_eq!(borrow, 0, "BigUint subtraction underflow");
        Self::normalize(limbs)
    }

    fn mul(&self, other: &Self) -> Self {
        if self.is_zero() || other.is_zero() {
            return Self::zero();
        }
        let mut limbs = vec![0u32; self.limbs.len() + other.limbs.len() + 1];
        for i in 0..self.limbs.len() {
            let mut carry = 0u64;
            for j in 0..other.limbs.len() {
                let index = i + j;
                let product = u64::from(self.limbs[i]) * u64::from(other.limbs[j])
                    + u64::from(limbs[index])
                    + carry;
                limbs[index] = product as u32;
                carry = product >> 32;
            }
            let mut index = i + other.limbs.len();
            while carry != 0 {
                let sum = u64::from(limbs[index]) + carry;
                limbs[index] = sum as u32;
                carry = sum >> 32;
                index += 1;
            }
        }
        Self::normalize(limbs)
    }

    fn shl_bits(&self, shift: usize) -> Self {
        if self.is_zero() {
            return Self::zero();
        }
        let word_shift = shift / 32;
        let bit_shift = shift % 32;
        let mut limbs = vec![0u32; word_shift];
        if bit_shift == 0 {
            limbs.extend_from_slice(&self.limbs);
        } else {
            let mut carry = 0u32;
            for &limb in &self.limbs {
                limbs.push((limb << bit_shift) | carry);
                carry = limb >> (32 - bit_shift);
            }
            if carry != 0 {
                limbs.push(carry);
            }
        }
        Self::normalize(limbs)
    }

    fn shr_bits(&self, shift: usize) -> Self {
        let word_shift = shift / 32;
        if word_shift >= self.limbs.len() {
            return Self::zero();
        }
        let bit_shift = shift % 32;
        let mut limbs = self.limbs[word_shift..].to_vec();
        if bit_shift != 0 {
            let mut carry = 0u32;
            for limb in limbs.iter_mut().rev() {
                let next_carry = *limb << (32 - bit_shift);
                *limb = (*limb >> bit_shift) | carry;
                carry = next_carry;
            }
        }
        Self::normalize(limbs)
    }

    fn trailing_zeros(&self) -> usize {
        for (index, &limb) in self.limbs.iter().enumerate() {
            if limb != 0 {
                return index * 32 + limb.trailing_zeros() as usize;
            }
        }
        0
    }

    fn rem_u32(&self, divisor: u32) -> u32 {
        debug_assert_ne!(divisor, 0);
        let mut remainder = 0u64;
        for &limb in self.limbs.iter().rev() {
            remainder = ((remainder << 32) | u64::from(limb)) % u64::from(divisor);
        }
        remainder as u32
    }

    /// Keep only the low `bits` bits.
    fn mask_bits(&self, bits: usize) -> Self {
        let limbs = bits.div_ceil(32);
        let mut masked = self.limbs[..self.limbs.len().min(limbs)].to_vec();
        if !bits.is_multiple_of(32)
            && let Some(top) = masked.last_mut()
        {
            *top &= (1u32 << (bits % 32)) - 1;
        }
        Self::normalize(masked)
    }

    /// Remainder by binary long division. Only used for RSA CRT reduction,
    /// where the operands are small enough that the simple loop is cheap.
    fn rem(&self, divisor: &Self) -> Self {
        debug_assert!(!divisor.is_zero());
        if divisor.cmp_big(self) == Ordering::Greater {
            return self.clone();
        }
        let mut remainder = Self::zero();
        for index in (0..self.bit_len()).rev() {
            remainder = remainder.shl_bits(1);
            if self.bit(index) {
                remainder.set_bit(0);
            }
            if remainder.cmp_big(divisor) != Ordering::Less {
                remainder = remainder.sub(divisor);
            }
        }
        remainder
    }

    fn add_mod(&self, other: &Self, modulus: &Self) -> Self {
        let sum = self.add(other);
        if sum.cmp_big(modulus) != Ordering::Less {
            sum.sub(modulus)
        } else {
            sum
        }
    }

    fn sub_mod(&self, other: &Self, modulus: &Self) -> Self {
        if self.cmp_big(other) != Ordering::Less {
            self.sub(other)
        } else {
            (self.add(modulus)).sub(other)
        }
    }
}

fn gcd(mut a: BigUint, mut b: BigUint) -> BigUint {
    if a.is_zero() {
        return b;
    }
    if b.is_zero() {
        return a;
    }
    let a_shift = a.trailing_zeros();
    let b_shift = b.trailing_zeros();
    let shift = a_shift.min(b_shift);
    a = a.shr_bits(a_shift);
    b = b.shr_bits(b_shift);
    loop {
        match a.cmp_big(&b) {
            Ordering::Equal => break,
            Ordering::Greater => {
                a = a.sub(&b);
                if a.is_zero() {
                    break;
                }
                a = a.shr_bits(a.trailing_zeros());
            }
            Ordering::Less => {
                b = b.sub(&a);
                if b.is_zero() {
                    break;
                }
                b = b.shr_bits(b.trailing_zeros());
            }
        }
    }
    a.shl_bits(shift)
}

/// `value^-1 mod modulus`, or `None` when not invertible.
///
/// The binary extended-GCD kernel needs an odd modulus. RSA deliberately asks
/// for inverses modulo even numbers (`p - 1`, `q - 1`), so an even modulus is
/// split into `2^s * odd` and the two inverses are combined with CRT. For
/// `e = 65537` this is a normal case, not an edge case.
fn mod_inverse(value: &BigUint, modulus: &BigUint) -> Option<BigUint> {
    if modulus.bit_len() < 2 {
        return None;
    }
    if modulus.is_odd() {
        return mod_inverse_odd(&value.rem(modulus), modulus);
    }
    if value.is_even() {
        return None;
    }
    let shift = modulus.trailing_zeros();
    let odd = modulus.shr_bits(shift);
    let two_shift = BigUint::one().shl_bits(shift);
    let inverse_power = inverse_power_of_two(value, shift);
    if odd == BigUint::one() {
        return Some(inverse_power);
    }
    let inverse_odd = mod_inverse_odd(&value.rem(&odd), &odd)?;
    let shift_mod = two_shift.rem(&odd);
    let shift_inverse = mod_inverse_odd(&shift_mod, &odd)?;
    let difference = inverse_odd.sub_mod(&inverse_power.rem(&odd), &odd);
    let montgomery = Montgomery::new(odd.clone());
    let correction = montgomery.multiply(&montgomery.to_montgomery(&difference), &shift_inverse);
    Some(inverse_power.add(&two_shift.mul(&correction)))
}

/// Inverse of an odd `value` modulo an odd `modulus`.
fn mod_inverse_odd(value: &BigUint, modulus: &BigUint) -> Option<BigUint> {
    if modulus.cmp_big(&BigUint::one()) != Ordering::Greater {
        return None;
    }
    let one = BigUint::one();
    let mut u = value.rem(modulus);
    if u.is_zero() {
        return None;
    }
    let mut v = modulus.clone();
    let mut x1 = one.clone();
    let mut x2 = BigUint::zero();

    loop {
        if u.is_zero() || v.is_zero() {
            return None;
        }
        if u == one {
            return Some(x1);
        }
        if v == one {
            return Some(x2);
        }
        if u == v {
            return None;
        }
        while u.is_even() {
            u = u.shr_bits(1);
            x1 = if x1.is_even() {
                x1.shr_bits(1)
            } else {
                // (x1 + modulus) / 2, not reduced: the sum is even because
                // both operands are odd.
                x1.add(modulus).shr_bits(1)
            };
        }
        while v.is_even() {
            v = v.shr_bits(1);
            x2 = if x2.is_even() {
                x2.shr_bits(1)
            } else {
                x2.add(modulus).shr_bits(1)
            };
        }
        if u == v {
            return None;
        }
        if u.cmp_big(&v) != Ordering::Less {
            u = u.sub(&v);
            x1 = x1.sub_mod(&x2, modulus);
        } else {
            v = v.sub(&u);
            x2 = x2.sub_mod(&x1, modulus);
        }
    }
}

/// Inverse of an odd `value` modulo `2^bits`, by Newton doubling precision.
fn inverse_power_of_two(value: &BigUint, bits: usize) -> BigUint {
    let mut inverse = BigUint::one();
    let mut precision = 1usize;
    while precision < bits {
        // inverse = inverse * (2 - value * inverse) mod 2^bits
        let product = value.mul(&inverse).mask_bits(bits);
        let factor = BigUint::from_u64(2)
            .add(&BigUint::one().shl_bits(bits))
            .sub(&product)
            .mask_bits(bits);
        inverse = inverse.mul(&factor).mask_bits(bits);
        precision = precision.saturating_mul(2);
    }
    inverse.mask_bits(bits)
}

struct Montgomery {
    modulus: BigUint,
    negative_inverse: u32,
    r_squared: BigUint,
    limbs: usize,
}

impl Montgomery {
    fn new(modulus: BigUint) -> Self {
        debug_assert!(modulus.is_odd());
        let limbs = modulus.limbs.len();
        let mut inverse = 1u32;
        let least = modulus.limbs[0];
        for _ in 0..5 {
            inverse = inverse.wrapping_mul(2u32.wrapping_sub(least.wrapping_mul(inverse)));
        }
        let negative_inverse = inverse.wrapping_neg();

        // R^2 mod n, where R = 2^(32 * limbs), by repeated doubling.
        let mut r_squared = BigUint::one();
        for _ in 0..(64 * limbs) {
            r_squared = r_squared.add_mod(&r_squared, &modulus);
        }
        Self {
            modulus,
            negative_inverse,
            r_squared,
            limbs,
        }
    }

    fn multiply(&self, left: &BigUint, right: &BigUint) -> BigUint {
        let limbs = self.limbs;
        let mut workspace = vec![0u32; 2 * limbs + 2];
        for i in 0..limbs {
            let mut carry = 0u64;
            let left_limb = u64::from(left.limb(i));
            for j in 0..limbs {
                let index = i + j;
                let value =
                    left_limb * u64::from(right.limb(j)) + u64::from(workspace[index]) + carry;
                workspace[index] = value as u32;
                carry = value >> 32;
            }
            let mut index = i + limbs;
            while carry != 0 {
                let value = u64::from(workspace[index]) + carry;
                workspace[index] = value as u32;
                carry = value >> 32;
                index += 1;
            }
        }

        for i in 0..limbs {
            let multiplier = workspace[i].wrapping_mul(self.negative_inverse);
            let mut carry = 0u64;
            for j in 0..limbs {
                let index = i + j;
                let value = u64::from(multiplier) * u64::from(self.modulus.limb(j))
                    + u64::from(workspace[index])
                    + carry;
                workspace[index] = value as u32;
                carry = value >> 32;
            }
            let mut index = i + limbs;
            while carry != 0 {
                let value = u64::from(workspace[index]) + carry;
                workspace[index] = value as u32;
                carry = value >> 32;
                index += 1;
            }
        }

        let mut result = BigUint::normalize(workspace[limbs..].to_vec());
        // Montgomery reduction of a product of two reduced operands is < 2n.
        if result.cmp_big(&self.modulus) != Ordering::Less {
            result = result.sub(&self.modulus);
        }
        result
    }

    fn to_montgomery(&self, value: &BigUint) -> BigUint {
        self.multiply(value, &self.r_squared)
    }

    fn reduce_montgomery(&self, value: &BigUint) -> BigUint {
        self.multiply(value, &BigUint::one())
    }

    fn pow(&self, base: &BigUint, exponent: &BigUint) -> BigUint {
        let mut result = self.to_montgomery(&BigUint::one());
        let base = self.to_montgomery(base);
        for index in (0..exponent.bit_len()).rev() {
            result = self.multiply(&result, &result);
            if exponent.bit(index) {
                result = self.multiply(&result, &base);
            }
        }
        self.reduce_montgomery(&result)
    }
}

fn mod_pow(base: &BigUint, exponent: &BigUint, modulus: &BigUint) -> BigUint {
    if modulus.is_zero() || modulus == &BigUint::one() {
        return BigUint::zero();
    }
    debug_assert!(base.cmp_big(modulus) == Ordering::Less);
    Montgomery::new(modulus.clone()).pow(base, exponent)
}

const SMALL_PRIMES: &[u32] = &[
    3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59, 61, 67, 71, 73, 79, 83, 89, 97,
    101, 103, 107, 109, 113, 127, 131, 137, 139, 149, 151, 157, 163, 167, 173, 179, 181, 191, 193,
    197, 199, 211, 223, 227, 229, 233, 239, 241, 251, 257, 263, 269, 271, 277, 281, 283, 293, 307,
    311, 313, 317, 331, 337, 347, 349, 353, 359, 367, 373, 379, 383, 389, 397, 401, 409, 419, 421,
    431, 433, 439, 443, 449, 457, 461, 463, 467, 479, 487, 491, 499, 503, 509, 521, 523, 541, 547,
    557, 563, 569, 571, 577, 587, 593, 599, 601, 607, 613, 617, 619, 631, 641, 643, 647, 653, 659,
    661, 673, 677, 683, 691, 701, 709, 719, 727, 733, 739, 743, 751, 757, 761, 769, 773, 787, 797,
    809, 811, 821, 823, 827, 829, 839, 853, 857, 859, 863, 877, 881, 883, 887, 907, 911, 919, 929,
    937, 941, 947, 953, 967, 971, 977, 983, 991, 997,
];

/// Miller-Rabin bases used for freshly generated random candidates. Fixed
/// bases are sound for randomly chosen candidates (an attacker never chooses
/// them); the rounds only guard against accidental composites.
const MILLER_RABIN_BASES: &[u32] = &[2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37];

fn is_probable_prime(candidate: &BigUint) -> bool {
    if candidate.bit_len() < 2 {
        return false;
    }
    if candidate.is_even() {
        return candidate == &BigUint::from_u64(2);
    }
    for &prime in SMALL_PRIMES {
        let prime = BigUint::from_u64(u64::from(prime));
        if candidate == &prime {
            return true;
        }
        if candidate.rem_u32(prime.limbs[0]) == 0 {
            return false;
        }
    }

    let one = BigUint::one();
    let candidate_minus_one = candidate.sub(&one);
    let mut exponent = candidate_minus_one.clone();
    let mut shifts = 0usize;
    while exponent.is_even() {
        exponent = exponent.shr_bits(1);
        shifts += 1;
    }

    let montgomery = Montgomery::new(candidate.clone());
    'bases: for &base in MILLER_RABIN_BASES {
        let base = BigUint::from_u64(u64::from(base));
        if base.cmp_big(candidate) != Ordering::Less {
            continue;
        }
        let mut value = montgomery.pow(&base, &exponent);
        if value == one || value == candidate_minus_one {
            continue;
        }
        for _ in 1..shifts {
            value = montgomery.multiply(&value, &value);
            if value == candidate_minus_one {
                continue 'bases;
            }
        }
        return false;
    }
    true
}

fn generate_prime(
    bits: usize,
    random: &mut impl FnMut(&mut [u8]) -> Result<(), CryptoError>,
) -> Result<BigUint, CryptoError> {
    let prime_bytes = bits / 16;
    for _ in 0..200_000 {
        let mut bytes = vec![0u8; prime_bytes];
        random(&mut bytes)?;
        // Force the exact requested bit length, so two factors always produce a
        // modulus of `bits` bits.
        bytes[0] |= 0xC0;
        bytes[prime_bytes - 1] |= 1;
        let candidate = BigUint::from_be_bytes(&bytes);
        if is_probable_prime(&candidate) {
            return Ok(candidate);
        }
    }
    Err(CryptoError::KeyGenerationFailed)
}

// ===========================================================================
// DER (SubjectPublicKeyInfo)
// ===========================================================================

fn der_length(length: usize, output: &mut Vec<u8>) {
    if length < 0x80 {
        output.push(length as u8);
    } else if length <= 0xFF {
        output.push(0x81);
        output.push(length as u8);
    } else if length <= 0xFFFF {
        output.push(0x82);
        output.push((length >> 8) as u8);
        output.push(length as u8);
    } else {
        output.push(0x83);
        output.push((length >> 16) as u8);
        output.push((length >> 8) as u8);
        output.push(length as u8);
    }
}

fn der_tlv(tag: u8, contents: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(contents.len() + 6);
    output.push(tag);
    der_length(contents.len(), &mut output);
    output.extend_from_slice(contents);
    output
}

fn der_unsigned_integer(value: &[u8]) -> Vec<u8> {
    let mut start = 0;
    while start < value.len() && value[start] == 0 {
        start += 1;
    }
    if start == value.len() {
        return der_tlv(0x02, &[0]);
    }
    let mut contents = Vec::with_capacity(value.len() + 1);
    if value[start] & 0x80 != 0 {
        contents.push(0);
    }
    contents.extend_from_slice(&value[start..]);
    der_tlv(0x02, &contents)
}

// Contents of the rsaEncryption AlgorithmIdentifier sequence:
// OID 1.2.840.113549.1.1.1 followed by NULL.
const RSA_ALGORITHM_IDENTIFIER: [u8; 13] = [
    0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x01, 0x05, 0x00,
];

fn strip_leading_zeros(value: &[u8]) -> &[u8] {
    let mut start = 0;
    while start < value.len() && value[start] == 0 {
        start += 1;
    }
    &value[start..]
}

fn der_read<'a>(data: &mut &'a [u8], expected_tag: u8) -> Result<&'a [u8], CryptoError> {
    let Some((&tag, rest)) = data.split_first() else {
        return Err(CryptoError::InvalidDer);
    };
    if tag != expected_tag {
        return Err(CryptoError::InvalidDer);
    }
    let (&first, mut rest) = rest.split_first().ok_or(CryptoError::InvalidDer)?;
    let length = if first & 0x80 == 0 {
        usize::from(first)
    } else {
        let count = usize::from(first & 0x7F);
        if count == 0 || count > 4 || rest.len() < count {
            return Err(CryptoError::InvalidDer);
        }
        let mut value = 0usize;
        for _ in 0..count {
            value = (value << 8) | usize::from(rest[0]);
            rest = &rest[1..];
        }
        value
    };
    if rest.len() < length {
        return Err(CryptoError::InvalidDer);
    }
    let (contents, remaining) = rest.split_at(length);
    *data = remaining;
    Ok(contents)
}

// ===========================================================================
// RSA
// ===========================================================================

/// RSA public key. Only the modulus and exponent are needed for DER encoding
/// and for the client-side encryption used by tests.
#[derive(Clone, Debug)]
pub struct RsaPublicKey {
    modulus: BigUint,
    exponent: u32,
}

impl RsaPublicKey {
    pub fn from_der(der: &[u8]) -> Result<Self, CryptoError> {
        let mut data = der;
        let outer = der_read(&mut data, 0x30)?;
        if !data.is_empty() {
            return Err(CryptoError::InvalidDer);
        }
        let mut outer = outer;
        let algorithm = der_read(&mut outer, 0x30)?;
        if algorithm != RSA_ALGORITHM_IDENTIFIER {
            return Err(CryptoError::InvalidDer);
        }
        let bit_string = der_read(&mut outer, 0x03)?;
        if bit_string.first() != Some(&0) {
            return Err(CryptoError::InvalidDer);
        }
        let mut key_data = &bit_string[1..];
        let key_sequence = der_read(&mut key_data, 0x30)?;
        if !key_data.is_empty() {
            return Err(CryptoError::InvalidDer);
        }

        let mut key_sequence = key_sequence;
        let modulus = der_read(&mut key_sequence, 0x02)?;
        if modulus.first().is_some_and(|byte| byte & 0x80 != 0) {
            return Err(CryptoError::InvalidDer);
        }
        let modulus = BigUint::from_be_bytes(strip_leading_zeros(modulus));
        let exponent = der_read(&mut key_sequence, 0x02)?;
        if exponent.is_empty() || exponent.len() > 4 {
            return Err(CryptoError::InvalidDer);
        }
        let exponent = strip_leading_zeros(exponent);
        if exponent.is_empty() {
            return Err(CryptoError::InvalidDer);
        }
        let mut exponent_value = 0u32;
        for &byte in exponent {
            exponent_value = (exponent_value << 8) | u32::from(byte);
        }
        if modulus.is_even() || exponent_value < 3 || exponent_value.is_multiple_of(2) {
            return Err(CryptoError::InvalidKey);
        }
        Ok(Self {
            modulus,
            exponent: exponent_value,
        })
    }

    pub fn to_der(&self) -> Vec<u8> {
        let length = self.modulus.to_be_bytes(self.modulus.bit_len().div_ceil(8));
        let mut key = Vec::new();
        key.extend_from_slice(&der_unsigned_integer(&length));
        key.extend_from_slice(&der_unsigned_integer(&self.exponent.to_be_bytes()));
        let key = der_tlv(0x30, &key);

        let mut bit_string = Vec::with_capacity(key.len() + 1);
        bit_string.push(0);
        bit_string.extend_from_slice(&key);

        let mut spki = Vec::new();
        spki.extend_from_slice(&der_tlv(0x30, &RSA_ALGORITHM_IDENTIFIER));
        spki.extend_from_slice(&der_tlv(0x03, &bit_string));
        der_tlv(0x30, &spki)
    }

    pub fn modulus_bytes(&self) -> usize {
        self.modulus.bit_len().div_ceil(8)
    }

    /// PKCS#1 v1.5 encryption with random non-zero padding. Used by clients
    /// (including the mocked client in tests) to answer the encryption
    /// request.
    pub fn encrypt_pkcs1v15(&self, message: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let modulus_bytes = self.modulus_bytes();
        if message.len() + 11 > modulus_bytes {
            return Err(CryptoError::InvalidKey);
        }
        let mut block = vec![0u8; modulus_bytes];
        block[0] = 0;
        block[1] = 2;
        let padding_length = modulus_bytes - message.len() - 3;
        random_bytes(&mut block[2..2 + padding_length])?;
        for byte in &mut block[2..2 + padding_length] {
            while *byte == 0 {
                random_bytes(std::slice::from_mut(byte))?;
            }
        }
        block[2 + padding_length] = 0;
        block[3 + padding_length..].copy_from_slice(message);

        let message = BigUint::from_be_bytes(&block);
        let ciphertext = mod_pow(
            &message,
            &BigUint::from_u64(u64::from(self.exponent)),
            &self.modulus,
        );
        Ok(ciphertext.to_be_bytes(modulus_bytes))
    }

    /// Verify an RSASSA-PKCS1-v1_5/SHA-256 signature. Malformed signatures and
    /// wrong lengths return `false`; they are untrusted network input.
    pub fn verify_sha256(&self, message: &[u8], signature: &[u8]) -> bool {
        let modulus_bytes = self.modulus_bytes();
        if signature.len() != modulus_bytes {
            return false;
        }
        let signature = BigUint::from_be_bytes(signature);
        if signature.cmp_big(&self.modulus) != Ordering::Less {
            return false;
        }
        let recovered = mod_pow(
            &signature,
            &BigUint::from_u64(u64::from(self.exponent)),
            &self.modulus,
        );
        let recovered = recovered.to_be_bytes(modulus_bytes);
        let Ok(expected) = emsa_pkcs1_v1_5_sha256(message, modulus_bytes) else {
            return false;
        };
        constant_time_eq(&recovered, &expected)
    }
}

/// DER-encoded `DigestInfo` prefix for SHA-256 (RFC 8017, section 9.2).
const SHA256_DIGEST_INFO_PREFIX: [u8; 19] = [
    0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05,
    0x00, 0x04, 0x20,
];

/// `EMSA-PKCS1-v1_5` encoding for SHA-256.
fn emsa_pkcs1_v1_5_sha256(message: &[u8], modulus_bytes: usize) -> Result<Vec<u8>, CryptoError> {
    let mut digest_info = Vec::with_capacity(SHA256_DIGEST_INFO_PREFIX.len() + SHA256_DIGEST_SIZE);
    digest_info.extend_from_slice(&SHA256_DIGEST_INFO_PREFIX);
    digest_info.extend_from_slice(&sha256(message));
    if modulus_bytes < digest_info.len() + 11 {
        return Err(CryptoError::InvalidKey);
    }
    let mut encoded = vec![0xFF; modulus_bytes];
    encoded[0] = 0x00;
    encoded[1] = 0x01;
    let separator = modulus_bytes - digest_info.len() - 1;
    encoded[separator] = 0x00;
    encoded[separator + 1..].copy_from_slice(&digest_info);
    Ok(encoded)
}

#[derive(Clone, Debug)]
enum RsaPrivateKey {
    /// Plain `c^d mod n`.
    Exponent(BigUint),
    /// Chinese remainder form, faster for decryption.
    Crt {
        p: BigUint,
        q: BigUint,
        dp: BigUint,
        dq: BigUint,
        q_inverse: BigUint,
    },
}

/// A Minecraft server RSA key pair.
#[derive(Clone, Debug)]
pub struct RsaKeyPair {
    public: RsaPublicKey,
    private: RsaPrivateKey,
}

impl RsaKeyPair {
    /// Generate a fresh 1024-bit RSA key pair with the Java default public
    /// exponent 65537. This is the login-encryption key size.
    pub fn generate() -> Result<Self, CryptoError> {
        Self::generate_with_bits(RSA_KEY_BITS)
    }

    /// Generate a fresh RSA key pair of `bits` bits with the Java default
    /// public exponent 65537. Chat profile keys need 2048 bits so their
    /// signatures are the 256 bytes required by `MessageSignature`.
    pub fn generate_with_bits(bits: usize) -> Result<Self, CryptoError> {
        if bits < 512 || !bits.is_multiple_of(16) {
            return Err(CryptoError::InvalidKey);
        }
        let mut random = |buffer: &mut [u8]| random_bytes(buffer);
        for _ in 0..1024 {
            let p = generate_prime(bits, &mut random)?;
            let q = generate_prime(bits, &mut random)?;
            if p == q {
                continue;
            }
            let modulus = p.mul(&q);
            if modulus.bit_len() != bits {
                continue;
            }
            let exponent = BigUint::from_u64(u64::from(RSA_PUBLIC_EXPONENT));
            let p_minus_one = p.sub(&BigUint::one());
            let q_minus_one = q.sub(&BigUint::one());
            if gcd(p_minus_one.clone(), exponent.clone()) != BigUint::one()
                || gcd(q_minus_one.clone(), exponent.clone()) != BigUint::one()
            {
                continue;
            }
            let Some(dp) = mod_inverse(&exponent, &p_minus_one) else {
                continue;
            };
            let Some(dq) = mod_inverse(&exponent, &q_minus_one) else {
                continue;
            };
            let Some(q_inverse) = mod_inverse(&q, &p) else {
                continue;
            };
            return Ok(Self {
                public: RsaPublicKey {
                    modulus,
                    exponent: RSA_PUBLIC_EXPONENT,
                },
                private: RsaPrivateKey::Crt {
                    p,
                    q,
                    dp,
                    dq,
                    q_inverse,
                },
            });
        }
        Err(CryptoError::KeyGenerationFailed)
    }

    /// Inject a key from big-endian components, primarily for deterministic
    /// test fixtures and key recovery from an existing PEM.
    pub fn from_components(
        modulus: &[u8],
        public_exponent: u32,
        private_exponent: &[u8],
    ) -> Result<Self, CryptoError> {
        let modulus = BigUint::from_be_bytes(modulus);
        let private_exponent = BigUint::from_be_bytes(private_exponent);
        if modulus.bit_len() < 512
            || !modulus.is_odd()
            || public_exponent < 3
            || public_exponent.is_multiple_of(2)
            || private_exponent.is_zero()
        {
            return Err(CryptoError::InvalidKey);
        }
        Ok(Self {
            public: RsaPublicKey {
                modulus,
                exponent: public_exponent,
            },
            private: RsaPrivateKey::Exponent(private_exponent),
        })
    }

    pub fn public(&self) -> &RsaPublicKey {
        &self.public
    }

    pub fn public_key_der(&self) -> Vec<u8> {
        self.public.to_der()
    }

    /// RSA private-key operation: `input^d mod n`, using CRT when available.
    fn private_operation(&self, input: &BigUint) -> BigUint {
        match &self.private {
            RsaPrivateKey::Exponent(exponent) => mod_pow(input, exponent, &self.public.modulus),
            RsaPrivateKey::Crt {
                p,
                q,
                dp,
                dq,
                q_inverse,
            } => {
                let reduced_p = input.rem(p);
                let reduced_q = input.rem(q);
                let from_p = mod_pow(&reduced_p, dp, p);
                let from_q = mod_pow(&reduced_q, dq, q);
                // CRT combines modulo p, so the q residue must be reduced mod p
                // first (p and q are independent primes, either may be larger).
                let difference = from_p.sub_mod(&from_q.rem(p), p);
                let montgomery = Montgomery::new(p.clone());
                let h = montgomery.multiply(&montgomery.to_montgomery(&difference), q_inverse);
                from_q.add(&q.mul(&h))
            }
        }
    }

    /// Decrypt one PKCS#1 v1.5 block and return the unpadded message.
    pub fn decrypt_pkcs1v15(&self, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let modulus_bytes = self.public.modulus_bytes();
        if ciphertext.len() != modulus_bytes {
            return Err(CryptoError::InvalidCiphertext);
        }
        let ciphertext = BigUint::from_be_bytes(ciphertext);
        if ciphertext.cmp_big(&self.public.modulus) != Ordering::Less {
            return Err(CryptoError::InvalidCiphertext);
        }
        let message = self.private_operation(&ciphertext);
        let encoded = message.to_be_bytes(modulus_bytes);
        decode_pkcs1_v1_5(&encoded)
    }

    /// Sign `message` with RSASSA-PKCS1-v1_5 and SHA-256
    /// (`SHA256withRSA`, the profile-key/message-signature algorithm).
    pub fn sign_sha256(&self, message: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let encoded = emsa_pkcs1_v1_5_sha256(message, self.public.modulus_bytes())?;
        let message = BigUint::from_be_bytes(&encoded);
        let signature = self.private_operation(&message);
        Ok(signature.to_be_bytes(self.public.modulus_bytes()))
    }
}

fn decode_pkcs1_v1_5(encoded: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if encoded.len() < 11 || encoded[0] != 0 || encoded[1] != 2 {
        return Err(CryptoError::InvalidPadding);
    }
    let mut separator = None;
    for (index, &byte) in encoded.iter().enumerate().skip(2) {
        if byte == 0 {
            separator = Some(index);
            break;
        }
    }
    let separator = separator.ok_or(CryptoError::InvalidPadding)?;
    if separator < 10 {
        return Err(CryptoError::InvalidPadding);
    }
    Ok(encoded[separator + 1..].to_vec())
}

#[cfg(test)]
mod self_tests {
    use super::*;

    fn mul_mod(a: &BigUint, b: &BigUint, m: &BigUint) -> BigUint {
        let montgomery = Montgomery::new(m.clone());
        montgomery.multiply(&montgomery.to_montgomery(a), b)
    }

    #[test]
    fn generated_q_inverse_is_consistent() {
        let keypair = RsaKeyPair::generate().unwrap();
        let RsaPrivateKey::Crt {
            p, q, q_inverse, ..
        } = &keypair.private
        else {
            panic!("generated keys use CRT");
        };
        let q_mod_p = q.rem(p);
        assert_eq!(mul_mod(&q_mod_p, q_inverse, p), BigUint::one());
    }

    #[test]
    fn sha256_matches_nist_vectors() {
        assert_eq!(
            sha256(b""),
            [
                0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
                0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
                0x78, 0x52, 0xb8, 0x55,
            ]
        );
        assert_eq!(
            sha256(b"abc"),
            [
                0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
                0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
                0xf2, 0x00, 0x15, 0xad,
            ]
        );
        let long = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
        assert_eq!(
            sha256(long),
            [
                0x24, 0x8d, 0x6a, 0x61, 0xd2, 0x06, 0x38, 0xb8, 0xe5, 0xc0, 0x26, 0x93, 0x0c, 0x3e,
                0x60, 0x39, 0xa3, 0x3c, 0xe4, 0x59, 0x64, 0xff, 0x21, 0x67, 0xf6, 0xec, 0xed, 0xd4,
                0x19, 0xdb, 0x06, 0xc1,
            ]
        );
    }

    #[test]
    fn rsa_sha256_sign_verify_round_trip_and_tamper() {
        let keypair = RsaKeyPair::generate().unwrap();
        let message = b"Folia-RS signed chat payload";
        let signature = keypair.sign_sha256(message).unwrap();
        assert_eq!(signature.len(), keypair.public().modulus_bytes());
        assert!(keypair.public().verify_sha256(message, &signature));

        let mut tampered = signature.clone();
        tampered[0] ^= 0x01;
        assert!(!keypair.public().verify_sha256(message, &tampered));
        assert!(!keypair.public().verify_sha256(b"different", &signature));
        assert!(!keypair.public().verify_sha256(message, &signature[1..]));
    }

    #[test]
    fn mod_inverse_matches_small_bruteforce() {
        for modulus in [3u32, 4, 5, 6, 7, 8, 10, 11, 12, 16, 97, 65537] {
            let m = BigUint::from_u64(u64::from(modulus));
            for value in 1..modulus {
                let v = BigUint::from_u64(u64::from(value));
                let result = mod_inverse(&v, &m);
                if gcd(v.clone(), m.clone()) != BigUint::one() {
                    assert!(result.is_none(), "{value} not invertible mod {modulus}");
                    continue;
                }
                assert!(
                    result.is_some(),
                    "{value} is invertible mod {modulus} but got None"
                );
                let inverse = result.unwrap();
                // Montgomery needs an odd modulus; compare by multiplying
                // without reduction for the even cases.
                let product = v.mul(&inverse);
                assert_eq!(product.rem(&m), BigUint::one(), "{value}^-1 mod {modulus}");
            }
        }
    }
}
