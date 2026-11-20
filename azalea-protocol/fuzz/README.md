Fuzzing for `azalea-protocol`.

Note that while Azalea doesn't try particularly hard to avoid crash exploits (because Azalea is generally used as a client), panics/crashes in the protocol library are still considered to be bugs that should be fixed.

Feel free to run this fuzzer for however long as you'd like to find crashes in `azalea-protocol`! It may take a very long time to find complex ones. Also, the input length is limited to 4 KiB by default.

Additionally, you should be aware that this fuzzer only targets `azalea-protocol`'s packet deserialization logic. There may be other crash bugs lurking outside of that.

## Usage

```sh
cargo install cargo-fuzz

cargo fuzz run clientbound_game -s none -- -rss_limit_mb=4096 -malloc_limit_mb=1024
# other valid targets:
# {clientbound,serverbound}_{config,game,handshake,login,status}

# the `-s none` is there for increased performance, but at the cost of catching
# less bugs. feel free to remove it.

# also see https://appsec.guide/docs/fuzzing/rust/cargo-fuzz/
```


## Hunt targets (WP2)

Besides the ten per-state decode targets:

- `roundtrip_<state>_<clientbound|serverbound>`: decode, encode, decode the
  result, encode again; the two encodings must be the same bytes.
- `frame`: the frame splitter, decompression and their size limits.
- `nbt_network`, `chat_component`, `paletted_container`, `chunk_section`.
- `handler_sequence`: the networkless `Simulation` client in play, fed a list
  of clientbound packets (`src/input.rs`); `config_to_play`: a fuzzed
  configuration phase, then seed play packets.

The bodies live in `src/targets.rs`, shared with the seed tests. Seeds are
checked in by the hunt repository (`fuzz/corpus/<target>/`, split from its
recordings); pass that directory as a second corpus so new inputs go to the
first one:

```sh
cargo fuzz build -a   # debug assertions and overflow checks on
cargo fuzz run frame /tmp/frame-work ../../fuzz/corpus/frame -- -rss_limit_mb=2048 -timeout=5
```

The hand-written deep seeds (`src/seeds.rs`, every recursive type at its
deepest) are tested by `tests/deep_seeds.rs` (1 s cap on a 16 MB thread) and
written with `cargo run -p azalea-fuzz --example write_seeds -- <corpus dir>`.
