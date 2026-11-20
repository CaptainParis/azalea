#![no_main]

use azalea_protocol::packets::status::ClientboundStatusPacket;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    azalea_fuzz::targets::roundtrip::<ClientboundStatusPacket>(data);
});
