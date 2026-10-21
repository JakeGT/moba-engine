#![no_main]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use moba_net::protocol::{
    Welcome, WireCatalog, decode_content_update, decode_hello, decode_input, decode_packet,
    decode_snapshot, decode_welcome, encode_content_update, encode_welcome,
};

/// Verifies once that the built-in catalogue — every hashed `RulesSpec` field included — and a
/// Welcome carrying it round-trip exactly. Doing this in `OnceLock` keeps the fuzzer's per-input
/// cost to decoding, and a decoder regression still fails the first iteration.
fn verify_fixtures() {
    static FIXTURES: OnceLock<()> = OnceLock::new();
    FIXTURES.get_or_init(|| {
        let catalog = WireCatalog::from(&moba_game::GameContent::default());
        let content_hash = catalog.content_hash().expect("built-in catalogue hashes");
        let mut encoded = Vec::with_capacity(2_048);
        encode_content_update(&catalog, content_hash, &mut encoded);
        let (decoded, decoded_hash) =
            decode_content_update(&encoded).expect("freshly encoded catalogue must decode");
        assert_eq!(decoded, catalog, "catalogue encode/decode round trip");
        assert_eq!(decoded_hash, content_hash, "catalogue content hash");

        let welcome = Welcome {
            slot: 3,
            tick_rate: 30,
            snapshot_interval: 1,
            tick: 7,
            seed: 9,
            hero_pick: 1,
            nonce: 11,
            server_nonce: 13,
            auth: [0; 32],
            spectator: false,
            catalog,
            content_hash,
            identity: false,
            server_public_key: [0; 32],
            server_signature: [0; 64],
        };
        let encoded = encode_welcome(&welcome);
        assert_eq!(
            decode_welcome(&encoded).expect("freshly encoded welcome must decode"),
            welcome,
            "welcome encode/decode round trip"
        );
    });
}

fuzz_target!(|data: &[u8]| {
    let _ = decode_packet(data);
    let _ = decode_snapshot(data);
    let _ = decode_input(data);
    let _ = decode_hello(data);
    let _ = decode_welcome(data);
    let _ = decode_content_update(data);

    verify_fixtures();
});
