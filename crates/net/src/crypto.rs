//! Optional pre-shared-key authenticated encryption for UDP payloads.
//!
//! Handshake: the client proves knowledge of a 32-byte pre-shared key with an HMAC over a
//! random nonce; the server replies with its own nonce plus an HMAC. Both sides derive a
//! per-session ChaCha20-Poly1305 key with HKDF-SHA256. Application packets are AEAD-sealed
//! with a nonce made from session id, direction and a strictly increasing per-session
//! counter carried in the sealed payload prefix; packet header fields are authenticated as
//! associated data. The counter is independent of protocol sequence numbers, so retries and
//! replies can never reuse a nonce.
//!
//! This is an explicit pre-shared-key mode for private/dedicated servers, not a public
//! matchmaking identity system. It protects confidentiality and integrity from passive and
//! off-path attackers that do not know the key; it does not hide packet metadata.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::protocol::Header;

type HmacSha256 = Hmac<Sha256>;

/// AEAD tag size appended to encrypted payloads.
pub const TAG_LEN: usize = 16;
/// Session key length.
pub const KEY_LEN: usize = 32;

/// Encryption/decryption failure. Callers drop malformed or unauthenticated datagrams.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CryptoError {
    /// The payload could not be authenticated or decrypted.
    Decrypt,
    /// Encrypting failed (length/allocation).
    Encrypt,
    /// Randomness or key derivation failed.
    KeyDerivation,
}

impl core::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Decrypt => f.write_str("authentication/decryption failed"),
            Self::Encrypt => f.write_str("encryption failed"),
            Self::KeyDerivation => f.write_str("key derivation failed"),
        }
    }
}

impl std::error::Error for CryptoError {}

/// Per-session AEAD state.
#[derive(Clone)]
pub struct SessionCrypto {
    key: [u8; KEY_LEN],
    session: u32,
    send_direction: u8,
    send_counter: Arc<AtomicU64>,
}

impl core::fmt::Debug for SessionCrypto {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SessionCrypto")
            .field("session", &self.session)
            .field("direction", &self.send_direction)
            .finish_non_exhaustive()
    }
}

impl SessionCrypto {
    /// Creates a session cipher. `direction` is 0 for client→server and 1 for server→client.
    #[must_use]
    pub fn new(key: [u8; KEY_LEN], session: u32, send_direction: u8) -> Self {
        Self {
            key,
            session,
            send_direction,
            send_counter: Arc::new(AtomicU64::new(0)),
        }
    }

    fn nonce(&self, direction: u8, counter: u32) -> [u8; 12] {
        let mut nonce = [0u8; 12];
        nonce[..4].copy_from_slice(&self.session.to_le_bytes());
        nonce[4..8].copy_from_slice(&counter.to_le_bytes());
        nonce[8] = direction;
        nonce
    }

    /// Encrypts and authenticates one application message.
    ///
    /// # Errors
    /// Returns [`CryptoError::Encrypt`] if the cipher rejects the input.
    pub fn seal(&self, header: &Header, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let cipher = ChaCha20Poly1305::new((&self.key).into());
        let aad = associated_data(header);
        // A 32-bit counter still gives 4 billion packets per session; refuse wraparound
        // rather than ever reuse a nonce.
        let counter = u32::try_from(self.send_counter.fetch_add(1, Ordering::Relaxed))
            .map_err(|_| CryptoError::Encrypt)?;
        let sealed = cipher
            .encrypt(
                (&self.nonce(self.send_direction, counter)).into(),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| CryptoError::Encrypt)?;
        let mut out = Vec::with_capacity(4 + sealed.len());
        out.extend_from_slice(&counter.to_le_bytes());
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// Verifies and decrypts one application message.
    ///
    /// # Errors
    /// Returns [`CryptoError::Decrypt`] for a bad tag, wrong key or tampered header.
    pub fn open(&self, header: &Header, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        let (counter, sealed) = ciphertext
            .split_first_chunk::<4>()
            .ok_or(CryptoError::Decrypt)?;
        let cipher = ChaCha20Poly1305::new((&self.key).into());
        let aad = associated_data(header);
        cipher
            .decrypt(
                (&self.nonce(self.send_direction ^ 1, u32::from_le_bytes(*counter))).into(),
                Payload {
                    msg: sealed,
                    aad: &aad,
                },
            )
            .map_err(|_| CryptoError::Decrypt)
    }
}

fn associated_data(header: &Header) -> [u8; 19] {
    let mut aad = [0u8; 19];
    aad[0] = header.kind as u8;
    aad[1..3].copy_from_slice(&header.flags.to_le_bytes());
    aad[3..7].copy_from_slice(&header.session.to_le_bytes());
    aad[7..11].copy_from_slice(&header.seq.to_le_bytes());
    aad[11..15].copy_from_slice(&header.ack.to_le_bytes());
    aad[15..19].copy_from_slice(&header.ack_bits.to_le_bytes());
    aad
}

/// Derives a per-session key from the pre-shared key and both handshake nonces.
#[must_use]
pub fn derive_session_key(
    pre_shared_key: &[u8; KEY_LEN],
    client_nonce: u64,
    server_nonce: u64,
) -> [u8; KEY_LEN] {
    let mut salt = [0u8; 16];
    salt[..8].copy_from_slice(&client_nonce.to_le_bytes());
    salt[8..].copy_from_slice(&server_nonce.to_le_bytes());
    let hkdf = Hkdf::<Sha256>::new(Some(&salt), pre_shared_key);
    let mut key = [0u8; KEY_LEN];
    hkdf.expand(b"moba-session-v1", &mut key)
        .expect("32-byte HKDF output is valid");
    key
}

/// Computes the client hello authenticator.
#[must_use]
pub fn hello_tag(
    pre_shared_key: &[u8; KEY_LEN],
    protocol: u16,
    nonce: u64,
    reconnect: u32,
    spectate: bool,
) -> [u8; 32] {
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(pre_shared_key).expect("HMAC accepts any key");
    mac.update(b"moba-hello-v1");
    mac.update(&protocol.to_le_bytes());
    mac.update(&nonce.to_le_bytes());
    mac.update(&reconnect.to_le_bytes());
    mac.update(&[u8::from(spectate)]);
    mac.finalize().into_bytes().into()
}

/// Verifies the client hello authenticator.
#[must_use]
pub fn verify_hello_tag(
    pre_shared_key: &[u8; KEY_LEN],
    protocol: u16,
    nonce: u64,
    reconnect: u32,
    spectate: bool,
    tag: &[u8; 32],
) -> bool {
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(pre_shared_key).expect("HMAC accepts any key");
    mac.update(b"moba-hello-v1");
    mac.update(&protocol.to_le_bytes());
    mac.update(&nonce.to_le_bytes());
    mac.update(&reconnect.to_le_bytes());
    mac.update(&[u8::from(spectate)]);
    mac.verify_slice(tag).is_ok()
}

/// Computes the server welcome authenticator.
#[must_use]
pub fn welcome_tag(
    pre_shared_key: &[u8; KEY_LEN],
    client_nonce: u64,
    server_nonce: u64,
    session: u32,
    slot: u8,
    content_hash: u64,
) -> [u8; 32] {
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(pre_shared_key).expect("HMAC accepts any key");
    mac.update(b"moba-welcome-v1");
    mac.update(&client_nonce.to_le_bytes());
    mac.update(&server_nonce.to_le_bytes());
    mac.update(&session.to_le_bytes());
    mac.update(&[slot]);
    mac.update(&content_hash.to_le_bytes());
    mac.finalize().into_bytes().into()
}

/// Verifies the server welcome authenticator.
#[must_use]
pub fn verify_welcome_tag(
    pre_shared_key: &[u8; KEY_LEN],
    client_nonce: u64,
    server_nonce: u64,
    session: u32,
    slot: u8,
    content_hash: u64,
    tag: &[u8; 32],
) -> bool {
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(pre_shared_key).expect("HMAC accepts any key");
    mac.update(b"moba-welcome-v1");
    mac.update(&client_nonce.to_le_bytes());
    mac.update(&server_nonce.to_le_bytes());
    mac.update(&session.to_le_bytes());
    mac.update(&[slot]);
    mac.update(&content_hash.to_le_bytes());
    mac.verify_slice(tag).is_ok()
}

/// Generates a cryptographically random handshake nonce.
///
/// # Errors
/// Returns an I/O error when the OS randomness source is unavailable.
pub fn random_nonce() -> std::io::Result<u64> {
    let mut bytes = [0u8; 8];
    getrandom::getrandom(&mut bytes)
        .map_err(|error| std::io::Error::other(format!("randomness: {error}")))?;
    Ok(u64::from_le_bytes(bytes))
}

/// Derives the Ed25519 public key for a 32-byte identity seed.
#[must_use]
pub fn identity_public_key(seed: &[u8; 32]) -> [u8; 32] {
    SigningKey::from_bytes(seed).verifying_key().to_bytes()
}

fn identity_message(
    protocol: u16,
    client_nonce: u64,
    server_nonce: u64,
    session: u32,
    slot: u8,
    content_hash: u64,
) -> Vec<u8> {
    let mut message = Vec::with_capacity(39);
    message.extend_from_slice(b"moba-server-id-v1");
    message.extend_from_slice(&protocol.to_le_bytes());
    message.extend_from_slice(&client_nonce.to_le_bytes());
    message.extend_from_slice(&server_nonce.to_le_bytes());
    message.extend_from_slice(&session.to_le_bytes());
    message.push(slot);
    message.extend_from_slice(&content_hash.to_le_bytes());
    message
}

/// Signs a content pack's canonical message.
#[must_use]
pub fn sign_pack(content: &moba_game::GameContent, seed: &[u8; 32]) -> moba_game::PackAttestation {
    let key = SigningKey::from_bytes(seed);
    moba_game::PackAttestation {
        publisher: key.verifying_key().to_bytes(),
        signature: key.sign(&content.signing_message()).to_bytes(),
    }
}

/// Verifies a pack's embedded Ed25519 attestation.
#[must_use]
pub fn verify_pack(content: &moba_game::GameContent) -> bool {
    let Some(attestation) = content.attestation.as_ref() else {
        return false;
    };
    let Ok(key) = VerifyingKey::from_bytes(&attestation.publisher) else {
        return false;
    };
    key.verify(
        &content.signing_message(),
        &Signature::from_bytes(&attestation.signature),
    )
    .is_ok()
}

/// Signs the server identity challenge embedded in `Welcome`.
#[must_use]
pub fn sign_server_welcome(
    seed: &[u8; 32],
    protocol: u16,
    client_nonce: u64,
    server_nonce: u64,
    session: u32,
    slot: u8,
    content_hash: u64,
) -> [u8; 64] {
    let key = SigningKey::from_bytes(seed);
    let message = identity_message(
        protocol,
        client_nonce,
        server_nonce,
        session,
        slot,
        content_hash,
    );
    key.sign(&message).to_bytes()
}

/// Verifies a pinned server identity signature.
#[must_use]
pub fn verify_server_welcome(
    public_key: &[u8; 32],
    protocol: u16,
    client_nonce: u64,
    server_nonce: u64,
    session: u32,
    slot: u8,
    content_hash: u64,
    signature: &[u8; 64],
) -> bool {
    let Ok(key) = VerifyingKey::from_bytes(public_key) else {
        return false;
    };
    let message = identity_message(
        protocol,
        client_nonce,
        server_nonce,
        session,
        slot,
        content_hash,
    );
    key.verify(&message, &Signature::from_bytes(signature))
        .is_ok()
}

fn client_identity_message(
    protocol: u16,
    client_nonce: u64,
    challenge: u64,
    reconnect: u32,
    spectate: bool,
) -> Vec<u8> {
    let mut message = Vec::with_capacity(28);
    message.extend_from_slice(b"moba-client-id-v1");
    message.extend_from_slice(&protocol.to_le_bytes());
    message.extend_from_slice(&client_nonce.to_le_bytes());
    message.extend_from_slice(&challenge.to_le_bytes());
    message.extend_from_slice(&reconnect.to_le_bytes());
    message.push(u8::from(spectate));
    message
}

/// Signs a server challenge to prove possession of a client identity key.
#[must_use]
pub fn sign_client_hello(
    seed: &[u8; 32],
    protocol: u16,
    client_nonce: u64,
    challenge: u64,
    reconnect: u32,
    spectate: bool,
) -> [u8; 64] {
    let key = SigningKey::from_bytes(seed);
    key.sign(&client_identity_message(
        protocol,
        client_nonce,
        challenge,
        reconnect,
        spectate,
    ))
    .to_bytes()
}

/// Verifies a client identity response.
#[must_use]
pub fn verify_client_hello(
    public_key: &[u8; 32],
    protocol: u16,
    client_nonce: u64,
    challenge: u64,
    reconnect: u32,
    spectate: bool,
    signature: &[u8; 64],
) -> bool {
    let Ok(key) = VerifyingKey::from_bytes(public_key) else {
        return false;
    };
    key.verify(
        &client_identity_message(protocol, client_nonce, challenge, reconnect, spectate),
        &Signature::from_bytes(signature),
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PacketKind;

    #[test]
    fn pack_attestation_signs_and_rejects_tampering() {
        let mut content = moba_game::GameContent::default();
        let seed = [7u8; 32];
        content.attestation = Some(sign_pack(&content, &seed));
        assert!(verify_pack(&content));
        content.manifest.version.push_str("-tampered");
        assert!(!verify_pack(&content));
        content.attestation = None;
        assert!(!verify_pack(&content));
    }

    fn header(seq: u32) -> Header {
        Header {
            kind: PacketKind::Input,
            flags: 0,
            session: 7,
            seq,
            ack: 1,
            ack_bits: 0,
        }
    }

    #[test]
    fn seal_open_round_trip_and_tamper_rejection() {
        let key = derive_session_key(&[9; 32], 11, 22);
        let sender = SessionCrypto::new(key, 42, 0);
        let receiver = SessionCrypto::new(key, 42, 1);
        let header = header(3);
        let sealed = sender.seal(&header, b"hello").unwrap();
        assert_ne!(sealed, b"hello");
        assert_eq!(receiver.open(&header, &sealed).unwrap(), b"hello");
        let mut tampered = sealed.clone();
        tampered[0] ^= 1;
        assert_eq!(receiver.open(&header, &tampered), Err(CryptoError::Decrypt));
        let mut wrong_header = header;
        wrong_header.ack ^= 1;
        assert_eq!(
            receiver.open(&wrong_header, &sealed),
            Err(CryptoError::Decrypt)
        );
    }

    #[test]
    fn repeated_seals_use_distinct_counters_and_nonces() {
        let key = derive_session_key(&[5; 32], 1, 2);
        let sender = SessionCrypto::new(key, 7, 0);
        let receiver = SessionCrypto::new(key, 7, 1);
        let header = header(6);
        let first = sender.seal(&header, b"one").unwrap();
        let second = sender.seal(&header, b"two").unwrap();
        assert_ne!(first[..4], second[..4]);
        assert_eq!(receiver.open(&header, &first).unwrap(), b"one");
        assert_eq!(receiver.open(&header, &second).unwrap(), b"two");
    }

    #[test]
    fn server_direction_round_trip() {
        let key = derive_session_key(&[4; 32], 7, 8);
        let server = SessionCrypto::new(key, 99, 1);
        let client = SessionCrypto::new(key, 99, 0);
        let header = header(1);
        let sealed = server.seal(&header, b"snapshot").unwrap();
        assert_eq!(client.open(&header, &sealed).unwrap(), b"snapshot");
    }

    #[test]
    fn client_identity_signature_is_challenge_bound() {
        let seed = [3u8; 32];
        let public = identity_public_key(&seed);
        let signature = sign_client_hello(&seed, 12, 77, 9001, 0, false);
        assert!(verify_client_hello(
            &public, 12, 77, 9001, 0, false, &signature
        ));
        assert!(!verify_client_hello(
            &public, 12, 77, 9002, 0, false, &signature
        ));
        assert!(!verify_client_hello(
            &public, 12, 78, 9001, 0, false, &signature
        ));
        assert!(!verify_client_hello(
            &[4; 32], 12, 77, 9001, 0, false, &signature
        ));
    }

    #[test]
    fn handshake_tags_are_keyed() {
        let tag = hello_tag(&[1; 32], 7, 123, 0, false);
        assert!(verify_hello_tag(&[1; 32], 7, 123, 0, false, &tag));
        assert!(!verify_hello_tag(&[2; 32], 7, 123, 0, false, &tag));
        assert!(!verify_hello_tag(&[1; 32], 7, 123, 0, true, &tag));
        let welcome = welcome_tag(&[1; 32], 123, 456, 9, 3, 0xCAFE);
        assert!(verify_welcome_tag(
            &[1; 32], 123, 456, 9, 3, 0xCAFE, &welcome
        ));
        assert!(!verify_welcome_tag(
            &[1; 32], 123, 457, 9, 3, 0xBEEF, &welcome
        ));
    }

    #[test]
    fn server_identity_signature_round_trip_and_tamper_rejection() {
        let seed = [7u8; 32];
        let public = identity_public_key(&seed);
        let signature = sign_server_welcome(&seed, 11, 1, 2, 3, 0, 0xCAFE);
        assert!(verify_server_welcome(
            &public, 11, 1, 2, 3, 0, 0xCAFE, &signature
        ));
        assert!(!verify_server_welcome(
            &public, 11, 1, 2, 3, 0, 0xBEEF, &signature
        ));
        assert!(!verify_server_welcome(
            &[8u8; 32], 11, 1, 2, 3, 0, 0xCAFE, &signature
        ));
    }

    #[test]
    fn derived_keys_are_directional_and_salted() {
        let a = derive_session_key(&[3; 32], 1, 2);
        let b = derive_session_key(&[3; 32], 1, 2);
        assert_eq!(a, b);
        assert_ne!(a, derive_session_key(&[3; 32], 2, 1));
    }
}
