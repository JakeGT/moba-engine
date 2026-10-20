//! Known-answer tests for the wire cryptography.
//!
//! The expected bytes were produced with the previous RustCrypto/dalek releases
//! (chacha20poly1305 0.10, hmac/hkdf 0.12, sha2 0.10, ed25519-dalek 2). The tests lock the
//! handshake tags, session key derivation, AEAD framing and every signature, so dependency
//! upgrades cannot silently change what goes on the wire or invalidate signed packs.

use moba_net::SessionCrypto;
use moba_net::crypto::{
    derive_session_key, hello_tag, identity_public_key, is_valid_identity_key, sign_client_hello,
    sign_pack, sign_server_welcome, verify_client_hello, verify_hello_tag, verify_pack,
    verify_server_welcome, verify_welcome_tag, welcome_tag,
};
use moba_net::protocol::{Header, PacketKind};

const PSK: [u8; 32] = [0x42; 32];
const SEED: [u8; 32] = [0x17; 32];

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

#[test]
fn handshake_tags_are_stable() {
    assert_eq!(
        hex(&hello_tag(&PSK, 36, 0x0102_0304_0506_0708, 9, true)),
        "e07eb5ebce27f47342d164033e4b3aa964bd11c4b034f3f0267f8dc6f7f2e905"
    );
    assert_eq!(
        hex(&welcome_tag(&PSK, 11, 22, 33, 4, 0xDEAD_BEEF)),
        "4f98d9d18e447b091d41a3f51d03020326ca0dcc2ce5c87838cb928c990880e5"
    );
}

#[test]
fn handshake_tags_verify_and_reject_tampering() {
    let hello = hello_tag(&PSK, 36, 7, 9, true);
    assert!(verify_hello_tag(&PSK, 36, 7, 9, true, &hello));
    assert!(!verify_hello_tag(&PSK, 36, 7, 9, false, &hello));
    let welcome = welcome_tag(&PSK, 11, 22, 33, 4, 5);
    assert!(verify_welcome_tag(&PSK, 11, 22, 33, 4, 5, &welcome));
    let mut tampered = welcome;
    tampered[0] ^= 1;
    assert!(!verify_welcome_tag(&PSK, 11, 22, 33, 4, 5, &tampered));
}

#[test]
fn weak_and_malformed_identity_keys_are_rejected() {
    assert!(is_valid_identity_key(&identity_public_key(&SEED)));
    // The identity point (encoded y = 1) and y = 0 are small-order, so "weak".
    let mut identity = [0u8; 32];
    identity[0] = 1;
    assert!(!is_valid_identity_key(&identity));
    assert!(!is_valid_identity_key(&[0u8; 32]));
    // Not a valid prime-order point either way: rejected.
    let mut off_curve = [0u8; 32];
    off_curve[0] = 2;
    assert!(!is_valid_identity_key(&off_curve));
}

#[test]
fn session_key_and_sealed_frame_are_stable() {
    let key = derive_session_key(&PSK, 11, 22);
    assert_eq!(
        hex(&key),
        "137a62b152a7665bf003ad1124cc7da8e62028ef9e1e6bf8cd6d641b1bed6225"
    );
    let header = Header {
        kind: PacketKind::Snapshot,
        flags: 0,
        session: 33,
        seq: 5,
        ack: 4,
        ack_bits: 3,
    };
    let sealed = SessionCrypto::new(key, 33, 1)
        .seal(&header, b"moba")
        .expect("seal");
    // 4-byte counter prefix, then ciphertext and Poly1305 tag.
    assert_eq!(
        hex(&sealed),
        "00000000553d8e98abc2013e45535f19eafdcb384fdd9afc"
    );
    let opened = SessionCrypto::new(key, 33, 0)
        .open(&header, &sealed)
        .expect("open");
    assert_eq!(opened, b"moba");
}

#[test]
fn identity_signatures_are_stable() {
    let public = identity_public_key(&SEED);
    assert_eq!(
        hex(&public),
        "31debe55d37c722768b137131caa6087080b2e0b60b94bd785d14575cfa498bc"
    );
    let server = sign_server_welcome(&SEED, 36, 11, 22, 33, 4, 0xDEAD_BEEF);
    assert_eq!(
        hex(&server),
        "56e5ff6d3f7f857e9cf9241099e63f1a9405cd7fdd57d214158ac24981f86890\
         2e1c20fa5ecd546f1d0ee60e7a0149044d0ed58b487ef972fe9a45e620358403"
    );
    assert!(verify_server_welcome(
        &public,
        36,
        11,
        22,
        33,
        4,
        0xDEAD_BEEF,
        &server
    ));
    let client = sign_client_hello(&SEED, 36, 11, 99, 0, false);
    assert_eq!(
        hex(&client),
        "f223bff8466191968d8d7b92fdbd176f55b4821b3b96cd8cf7ef461ac3583906\
         b72e5c2f24e2b15ca966001bd32f5aeacb5e345d2b4d4f563cf6898e9ca4e90e"
    );
    assert!(verify_client_hello(&public, 36, 11, 99, 0, false, &client));
}

#[test]
fn pack_signature_is_stable() {
    let mut content = moba_game::GameContent::default();
    let attestation = sign_pack(&content, &SEED);
    assert_eq!(
        hex(&attestation.signature),
        "110654bb9ff3381714a8ece0ff29b375fd760a5fb114b86e970eec238de849d7\
         7c100c2178cdd81510356de15b82e653a31d39ec638f3c6c06536fb8f1fe910c"
    );
    content.attestation = Some(attestation);
    assert!(verify_pack(&content));
}
