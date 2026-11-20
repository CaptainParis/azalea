//! Every hand-written deep seed (`azalea_fuzz::seeds`) decodes, and its fuzz
//! target runs it on a bot thread's 16 MB stack in under 1 s.

use std::{
    io::Cursor,
    time::{Duration, Instant},
};

use azalea_buf::AzBuf;
use azalea_chat::FormattedText;
use azalea_fuzz::{input::packet_list, seeds, targets};
use azalea_protocol::{packets::game::ClientboundGamePacket, read::deserialize_packet};

const BOT_STACK: usize = 16 * 1024 * 1024;
const CAP: Duration = Duration::from_secs(1);

fn run(target: &str, data: &[u8]) {
    match target {
        "clientbound_game" => targets::decode::<ClientboundGamePacket>(data),
        "roundtrip_game_clientbound" => targets::roundtrip::<ClientboundGamePacket>(data),
        "handler_sequence" => targets::handler_sequence(data),
        "chat_component" => targets::chat_component(data),
        "nbt_network" => targets::nbt_network(data),
        other => panic!("no runner for {other}"),
    }
}

/// The seed is valid input (it decodes), so the fuzzer starts deep.
fn check_valid(target: &str, name: &str, data: &[u8]) {
    let packet_ok = |p: &[u8]| {
        deserialize_packet::<ClientboundGamePacket>(&mut Cursor::new(p))
            .unwrap_or_else(|e| panic!("{name} doesn't decode: {e}"));
    };
    match target {
        "clientbound_game" | "roundtrip_game_clientbound" => packet_ok(data),
        "handler_sequence" => packet_list(data).into_iter().for_each(packet_ok),
        "chat_component" => {
            FormattedText::azalea_read(&mut Cursor::new(data))
                .unwrap_or_else(|e| panic!("{name} doesn't decode: {e}"));
        }
        "nbt_network" => {
            simdnbt::owned::NbtTag::azalea_read(&mut Cursor::new(data))
                .unwrap_or_else(|e| panic!("{name} doesn't decode: {e}"));
        }
        _ => {}
    }
}

#[test]
fn deep_seeds_run_on_a_bot_thread_in_under_a_second() {
    let all = seeds::all();
    assert!(all.len() >= 30, "{}", all.len());
    std::thread::Builder::new()
        .stack_size(BOT_STACK)
        .spawn(move || {
            for seed in all {
                check_valid(seed.target, seed.name, &seed.data);
                let start = Instant::now();
                run(seed.target, &seed.data);
                let took = start.elapsed();
                println!("{} {}: {took:?}", seed.target, seed.name);
                assert!(took < CAP, "{} {} took {took:?}", seed.target, seed.name);
            }
        })
        .unwrap()
        .join()
        .unwrap();
}
