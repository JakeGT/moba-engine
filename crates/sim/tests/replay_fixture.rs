//! Checked-in replay fixture verifies on every supported platform.

use moba_sim::Replay;

#[test]
fn checked_in_v7_replay_fixture_verifies() {
    let bytes = include_bytes!("fixtures/seed-v7.mbr");
    let replay = Replay::decode(bytes).expect("decode fixture");
    assert_eq!(replay.frames.len(), 300);
    let result = replay.play().expect("fixture replay");
    assert_eq!(result.ticks, 300);
    assert_eq!(result.state_hash, 0xf2ae_4e0a_8ab9_5859);
}
