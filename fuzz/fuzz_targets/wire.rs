#![no_main]

use libfuzzer_sys::fuzz_target;
use moba_net::protocol::{
    decode_content_update, decode_hello, decode_input, decode_packet, decode_snapshot,
    decode_welcome,
};

fuzz_target!(|data: &[u8]| {
    let _ = decode_packet(data);
    let _ = decode_snapshot(data);
    let _ = decode_input(data);
    let _ = decode_hello(data);
    let _ = decode_welcome(data);
    let _ = decode_content_update(data);
});
