#![no_main]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use moba_net::protocol::{
    WireCatalog, decode_content_update, decode_hello, decode_input, decode_packet, decode_snapshot,
    decode_welcome, encode_content_update, encode_snapshot,
};

/// A valid `ContentUpdate` for the built-in catalogue, encoded once.
///
/// The fuzzer mutates a copy of these bytes with its input, so `decode_content_update` sees a
/// realistic catalogue — including every hashed `RulesSpec` field carried since v38 and the
/// hero model / map visuals names carried since v40 — rather than only random noise. Building
/// the catalogue once keeps the per-input cost to decoding.
fn fixture() -> &'static Vec<u8> {
    static FIXTURE: OnceLock<Vec<u8>> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let mut content = moba_game::GameContent::default();
        content.heroes[0].model = Some(String::from("sentinel"));
        content.map.visuals = Some(String::from("map-lava"));
        let catalog = WireCatalog::from(&content);
        let content_hash = catalog.content_hash().expect("built-in catalogue hashes");
        let mut content_update = Vec::with_capacity(8_192);
        encode_content_update(&catalog, content_hash, &mut content_update);
        content_update
    })
}

fuzz_target!(|data: &[u8]| {
    let _ = decode_packet(data);
    if let Ok(snapshot) = decode_snapshot(data) {
        // Whatever decodes re-encodes to the same snapshot, including the compact
        // `EVENT_ATTACK` records (v41) mixed with full-size events.
        let mut payload = Vec::new();
        encode_snapshot(&snapshot, &mut payload).expect("a decoded snapshot re-encodes");
        assert_eq!(decode_snapshot(&payload), Ok(snapshot));
    }
    let _ = decode_input(data);
    let _ = decode_hello(data);
    let _ = decode_welcome(data);
    let _ = decode_content_update(data);

    let mut mutated = fixture().clone();
    for (slot, byte) in mutated.iter_mut().zip(data) {
        *slot ^= *byte;
    }
    let _ = decode_content_update(&mutated);
});
