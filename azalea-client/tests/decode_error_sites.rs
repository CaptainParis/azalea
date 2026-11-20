//! Architecture test for the decode error counter
//! (`azalea_core::decode_errors`).
//!
//! Every place in the packet handlers, chunk loading and registry parsing that
//! logs an error and moves on, or turns an error into `None`, must either
//! call `record_decode_error` or be listed in [`NOT_DECODE_ERRORS`] with the
//! reason it isn't a decode error. A new silent skip in these paths makes
//! this test fail.

use std::{
    fs,
    path::{Path, PathBuf},
};

use regex::Regex;

/// Directories and files that are scanned, relative to `azalea-client`.
const SCANNED: &[&str] = &[
    "src/plugins/packet",
    "src/plugins/chunks.rs",
    "../azalea-core/src/registry_holder",
];

/// How many lines after a matched line may contain the `record_decode_error`
/// call.
const ROUTED_WITHIN_LINES: usize = 12;

/// Sites that look like a skip but aren't a decode error, as (file, the
/// trimmed text of the matched line, how many times that line appears
/// unrouted in the file, reason). The count is exact, so a new site with the
/// same text still fails the test.
const NOT_DECODE_ERRORS: &[(&str, &str, usize, &str)] = &[
    // sending packets
    (
        "src/plugins/packet/config/events.rs",
        "if let Ok((mut raw_conn, in_configuration_state)) = query.get_mut(send_config_packet.sent_by) {",
        1,
        "ECS lookup of our own connection when sending",
    ),
    (
        "src/plugins/packet/config/events.rs",
        "error!(",
        1,
        "sending a packet in the wrong state is a bug in our code",
    ),
    (
        "src/plugins/packet/config/events.rs",
        "if let Err(e) = raw_conn.write(send_config_packet.packet.clone()) {",
        1,
        "write (encode/IO) error when sending",
    ),
    (
        "src/plugins/packet/config/events.rs",
        "error!(\"Failed to send packet: {e}\");",
        1,
        "write (encode/IO) error when sending",
    ),
    (
        "src/plugins/packet/game/events.rs",
        "if let Ok((mut raw_connection, in_game_state)) = query.get_mut(event.sent_by) {",
        1,
        "ECS lookup of our own connection when sending",
    ),
    (
        "src/plugins/packet/game/events.rs",
        "error!(",
        1,
        "sending a packet in the wrong state is a bug in our code",
    ),
    (
        "src/plugins/packet/game/events.rs",
        "if let Err(e) = raw_connection.write(event.packet.clone()) {",
        1,
        "write (encode/IO) error when sending",
    ),
    (
        "src/plugins/packet/game/events.rs",
        "error!(\"Failed to send packet: {e}\");",
        1,
        "write (encode/IO) error when sending",
    ),
    (
        "src/plugins/packet/login/events.rs",
        "if let Ok((mut raw_conn, in_login_state)) = query.get_mut(event.sent_by) {",
        1,
        "ECS lookup of our own connection when sending",
    ),
    (
        "src/plugins/packet/login/events.rs",
        "error!(",
        1,
        "sending a packet in the wrong state is a bug in our code",
    ),
    (
        "src/plugins/packet/login/events.rs",
        "if let Err(e) = raw_conn.write(event.packet.clone()) {",
        1,
        "write (encode/IO) error when sending",
    ),
    (
        "src/plugins/packet/login/events.rs",
        "error!(\"Failed to send packet: {e}\");",
        1,
        "write (encode/IO) error when sending",
    ),
    // disconnects
    (
        "src/plugins/packet/config/mod.rs",
        "warn!(\"Got disconnect packet {p:?}\");",
        1,
        "the packet decoded fine, the server disconnected us",
    ),
    (
        "src/plugins/packet/game/mod.rs",
        "warn!(\"Got disconnect packet {p:?}\");",
        1,
        "the packet decoded fine, the server disconnected us",
    ),
    // ECS lookups in the game packet handlers. the packet was decoded; the
    // entity or component it refers to isn't (or isn't yet) in our world
    (
        "src/plugins/packet/game/mod.rs",
        "warn!(\"got add player packet but we haven't gotten a login packet yet\");",
        1,
        "packet order (no login yet), not a decode error",
    ),
    (
        "src/plugins/packet/game/mod.rs",
        "let Ok(mut loaded_by) = loaded_by_query.get_mut(ecs_entity) else {",
        1,
        "ECS consistency check for an entity we already know",
    ),
    (
        "src/plugins/packet/game/mod.rs",
        "error!(",
        2,
        "ECS consistency checks for an entity we already know",
    ),
    (
        "src/plugins/packet/game/mod.rs",
        "let Ok(entity_kind) = entity_kind_query.get(entity) else {",
        1,
        "ECS lookup of an entity that has no kind yet",
    ),
    (
        "src/plugins/packet/game/mod.rs",
        "warn!(\"Got teleport entity packet for unknown entity id {}\", p.id);",
        1,
        "entity isn't in our world",
    ),
    (
        "src/plugins/packet/game/mod.rs",
        "let Ok(mut loaded_by) = entity_query.get_mut(entity) else {",
        1,
        "ECS lookup of an entity we already know",
    ),
    (
        "src/plugins/packet/game/mod.rs",
        "warn!(",
        1,
        "ECS lookup of an entity we already know",
    ),
    (
        "src/plugins/packet/game/mod.rs",
        "let Ok((mut physics, mut position, mut look_direction, local_entity)) =",
        1,
        "ECS lookup of an entity",
    ),
    (
        "src/plugins/packet/game/mod.rs",
        "let Ok((mut physics, mut position, mut look_direction)) = entity_query.get_mut(entity) else {",
        1,
        "ECS lookup of an entity",
    ),
    (
        "src/plugins/packet/relative_updates.rs",
        "let Ok((minecraft_entity_id, updates_received, local_entity)) = entity_update_query.get(entity)",
        1,
        "ECS lookup of an entity",
    ),
    (
        "src/plugins/packet/relative_updates.rs",
        "warn!(\"Entity {entity:?} has both LocalEntity and UpdatesReceived\");",
        1,
        "ECS consistency check",
    ),
    // registry parsing. these return an error (or None, which the caller turns
    // into an error) to the caller instead of skipping, so the failure reaches
    // RegistryType::append_nbt, which counts it
    (
        "../azalea-core/src/registry_holder/components.rs",
        "let Ok(nbt) = nbt.list(\"CrossbowChargingSounds\") else {",
        1,
        "not a list: falls back to the single compound form, which returns its error",
    ),
    (
        "../azalea-core/src/registry_holder/components.rs",
        ".map(|s| SoundEvent::from_str(&s.to_str()).ok())",
        2,
        "collected into an Option, so one bad sound fails the whole value",
    ),
    (
        "../azalea-core/src/registry_holder/entity_effect.rs",
        "error!(\"Error deserializing EntityEffect {kind}: {nbt:?}\");",
        1,
        "logs, then returns the error to the caller",
    ),
    (
        "../azalea-core/src/registry_holder/entity_effect.rs",
        ".ok()",
        1,
        "collected into an Option, so one bad sound fails the whole value",
    ),
    (
        "../azalea-core/src/registry_holder/float_provider.rs",
        "return Self::from_compound(c).ok();",
        1,
        "FromNbtTag returns None, which the caller turns into an error",
    ),
    (
        "../azalea-core/src/registry_holder/value.rs",
        "return Self::from_compound(c).ok();",
        1,
        "FromNbtTag returns None, which the caller turns into an error",
    ),
    (
        "../azalea-core/src/registry_holder/mod.rs",
        "error!(",
        2,
        "get_for_registry called with the wrong registry name is a bug in our code",
    ),
];

fn rust_files(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let mut entries = fs::read_dir(path)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect::<Vec<_>>();
        entries.sort();
        for entry in entries {
            rust_files(&entry, out);
        }
    } else if path.extension().is_some_and(|e| e == "rs") {
        out.push(path.to_owned());
    }
}

#[test]
fn every_skip_is_counted_or_explained() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let pattern =
        Regex::new(r"\bwarn!\(|\berror!\(|\blet Ok\(|\bif let Err\(|\.ok\(\)|\bErr\([^)]*\)\s*=>")
            .unwrap();

    let mut files = Vec::new();
    for scanned in SCANNED {
        rust_files(&root.join(scanned), &mut files);
    }
    assert!(!files.is_empty());

    let mut unexplained = Vec::new();
    let mut found = vec![0; NOT_DECODE_ERRORS.len()];
    for file in &files {
        let rel = file
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let contents = fs::read_to_string(file).unwrap();
        let lines = contents.lines().collect::<Vec<_>>();
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || !pattern.is_match(line) {
                continue;
            }
            let routed = lines[i..(i + ROUTED_WITHIN_LINES + 1).min(lines.len())]
                .iter()
                .any(|l| l.contains("record_decode_error("));
            if routed {
                continue;
            }
            if let Some(index) = NOT_DECODE_ERRORS
                .iter()
                .position(|(f, text, _, _)| *f == rel && *text == trimmed)
            {
                found[index] += 1;
                if found[index] <= NOT_DECODE_ERRORS[index].2 {
                    continue;
                }
            }
            unexplained.push(format!("{rel}:{}: {trimmed}", i + 1));
        }
    }

    assert!(
        unexplained.is_empty(),
        "these sites skip data without calling record_decode_error. route them \
         through the decode error counter, or add them to NOT_DECODE_ERRORS with \
         a reason:\n{}",
        unexplained.join("\n")
    );

    let stale = NOT_DECODE_ERRORS
        .iter()
        .zip(found)
        .filter(|((_, _, count, _), found)| found < count)
        .map(|((f, text, count, _), found)| format!("{f}: {text} (expected {count}, found {found})"))
        .collect::<Vec<_>>();
    assert!(
        stale.is_empty(),
        "these NOT_DECODE_ERRORS entries match fewer sites than listed:\n{}",
        stale.join("\n")
    );
}
