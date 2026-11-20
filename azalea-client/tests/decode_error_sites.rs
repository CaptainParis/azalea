//! Architecture test for the decode error counter
//! (`azalea_core::decode_errors`).
//!
//! Every place in the packet handlers, chunk loading and registry parsing that
//! logs an error and moves on, or turns an error into `None`, must either
//! call `record_decode_error` in the same block (at that block's own depth,
//! not in a nested block) or be listed in [`NOT_DECODE_ERRORS`] with the
//! reason it isn't a decode error. A new silent skip in these paths makes
//! this test fail.
//!
//! Sites: `warn!`/`error!`, `let Ok(..)`, `if let Err(..)`, `.ok()`,
//! `Err(..) =>`, `let Some(..) = .. else`, and `debug!`/`trace!` in a block
//! that then returns or continues. The block of a site is the one its
//! statement opens (`let .. else {`, `if let Err(..) {`, `Err(..) => {`), or
//! else the block around it.
//!
//! Only these categories may be listed (goldfish sign-off, 2026-11-19): send
//! side, disconnect warnings, add_player before login, second clients and
//! despawn races, relative_updates diagnostics, and registry sites whose error
//! reaches `RegistryType::append_nbt`, which counts it.

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
    // add_player before login: packet order, the packet itself decoded fine
    (
        "src/plugins/packet/game/mod.rs",
        "let Some(world_name) = world_name else {",
        1,
        "add_entity before any login packet: packet order, not a decode error",
    ),
    (
        "src/plugins/packet/game/mod.rs",
        "warn!(\"got add player packet but we haven't gotten a login packet yet\");",
        1,
        "add_entity before any login packet: packet order, not a decode error",
    ),
    // second clients and despawn races: several local clients share one world
    (
        "src/plugins/packet/game/mod.rs",
        "debug!(\"added to LoadedBy of entity {ecs_entity:?} with id {entity_id:?}\");",
        1,
        "another local client already spawned this entity; this client is added to LoadedBy, nothing is dropped",
    ),
    (
        "src/plugins/packet/game/mod.rs",
        "let Ok(entity_kind) = entity_kind_query.get(entity) else {",
        1,
        "indexed entity without EntityKind: a second local client that just disconnected (despawn race)",
    ),
    (
        "src/plugins/packet/game/mod.rs",
        "debug!(",
        1,
        "indexed entity without EntityKind: a second local client that just disconnected (despawn race)",
    ),
    (
        "src/plugins/chunks.rs",
        "trace!(\"Skipping parsing chunk {pos:?} because we already know about it\");",
        1,
        "another local client already parsed this chunk into the shared world; it is reused, nothing is dropped",
    ),
    // relative_updates diagnostics: the relative update guard for swarms
    (
        "src/plugins/packet/relative_updates.rs",
        "let Ok((minecraft_entity_id, updates_received, local_entity)) = entity_update_query.get(entity)",
        1,
        "entity despawned in the same Update as a relative update for it (despawn race)",
    ),
    (
        "src/plugins/packet/relative_updates.rs",
        "debug!(\"called should_apply_entity_update on an entity with missing components {entity:?}\");",
        1,
        "entity despawned in the same Update as a relative update for it (despawn race)",
    ),
    (
        "src/plugins/packet/relative_updates.rs",
        "warn!(\"Entity {entity:?} has both LocalEntity and UpdatesReceived\");",
        1,
        "diagnostic of our own bookkeeping; the update is still applied or refused by the same rules",
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

/// The source with comments and the contents of string and char literals
/// replaced by spaces (newlines kept), so braces in format strings don't count
/// and byte offsets stay the same.
fn blank_non_code(src: &str) -> Vec<u8> {
    let b = src.as_bytes();
    let mut out = b.to_vec();
    let mut i = 0;
    let blank = |out: &mut Vec<u8>, from: usize, to: usize| {
        for c in &mut out[from..to] {
            if *c != b'\n' {
                *c = b' ';
            }
        }
    };
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                let end = b[i..].iter().position(|&c| c == b'\n').map_or(b.len(), |n| i + n);
                blank(&mut out, i, end);
                i = end;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = src[i + 2..].find("*/").map_or(b.len(), |n| i + 2 + n + 2);
                blank(&mut out, i, end);
                i = end;
            }
            b'r' if b.get(i + 1) == Some(&b'#') || (b.get(i + 1) == Some(&b'"') && (i == 0 || !b[i - 1].is_ascii_alphanumeric() && b[i - 1] != b'_')) => {
                // raw string r"..." or r#"..."#
                let mut j = i + 1;
                let mut hashes = 0;
                while b.get(j) == Some(&b'#') {
                    hashes += 1;
                    j += 1;
                }
                if b.get(j) != Some(&b'"') {
                    i += 1;
                    continue;
                }
                let close = format!("\"{}", "#".repeat(hashes));
                let end = src[j + 1..].find(&close).map_or(b.len(), |n| j + 1 + n + close.len());
                blank(&mut out, j + 1, end.saturating_sub(close.len()).max(j + 1));
                i = end;
            }
            b'"' => {
                let mut j = i + 1;
                while j < b.len() && b[j] != b'"' {
                    j += if b[j] == b'\\' { 2 } else { 1 };
                }
                blank(&mut out, i + 1, j.min(b.len()));
                i = j + 1;
            }
            b'\'' => {
                // char literal like '{' or '\''; lifetimes are left alone
                if b.get(i + 2) == Some(&b'\'') {
                    blank(&mut out, i + 1, i + 2);
                    i += 3;
                } else if b.get(i + 1) == Some(&b'\\') && b.get(i + 3) == Some(&b'\'') {
                    blank(&mut out, i + 1, i + 3);
                    i += 4;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out
}

/// The byte range (inclusive of both braces) of the block a site belongs to.
///
/// If the statement or expression starting at `at` opens a block before it
/// ends (`let .. else {`, `if let Err(..) = .. {`, `Err(..) => {`), that block;
/// otherwise the innermost block around `at`.
fn site_block(code: &[u8], at: usize) -> (usize, usize) {
    let matching_close = |open: usize| {
        let mut depth = 0usize;
        for (j, &c) in code.iter().enumerate().skip(open) {
            match c {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return j;
                    }
                }
                _ => {}
            }
        }
        code.len() - 1
    };
    let mut parens = 0i32;
    for (j, &c) in code.iter().enumerate().skip(at) {
        match c {
            b'(' | b'[' => parens += 1,
            b')' | b']' => parens -= 1,
            b'{' if parens <= 0 => {
                // a struct pattern or literal inside the expression (`Foo { .. }`)
                // is not the site's block: those only appear inside parens here
                return (j, matching_close(j));
            }
            b';' | b'}' if parens <= 0 => break,
            b',' if parens <= 0 => break,
            _ => {}
        }
        if parens < 0 {
            break;
        }
    }
    // innermost enclosing block
    let mut depth = 0usize;
    let mut open = 0;
    for j in (0..at).rev() {
        match code[j] {
            b'}' => depth += 1,
            b'{' => {
                if depth == 0 {
                    open = j;
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    (open, matching_close(open))
}

/// The code of a block body with every nested block left out.
fn own_depth(body: &[u8]) -> String {
    let mut depth = 0i32;
    let mut flat = String::new();
    for &c in body {
        match c {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ if depth == 0 => flat.push(c as char),
            _ => {}
        }
    }
    flat
}

#[test]
fn every_skip_is_counted_or_explained() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    // skips: log-and-continue macros, `let Ok(..)`/`if let Err(..)`, `.ok()` on
    // results, `Err(..) =>` arms and `let Some(..) = .. else` (let-else only;
    // `if let Some` is ordinary control flow)
    let pattern = Regex::new(
        r"\bwarn!\(|\berror!\(|\blet Ok\(|\bif let Err\(|\.ok\(\)|\bErr\([^)]*\)\s*=>|(?:^|[^\w\s]|[^f]\s|^\s*)\blet Some\(",
    )
    .unwrap();
    let if_let_some = Regex::new(r"\b(?:if|while)\s+let Some\(").unwrap();
    // debug!/trace! count only where the block returns or continues after them
    let quiet = Regex::new(r"\b(?:debug|trace)!\(").unwrap();
    let leaves = Regex::new(r"\b(?:return|continue)\b").unwrap();

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
        let code = blank_non_code(&contents);
        let code_str = String::from_utf8_lossy(&code).into_owned();
        let mut line_start = 0;
        for (i, line) in contents.split('\n').enumerate() {
            let code_line = &code_str[line_start..line_start + line.len()];
            let this_start = line_start;
            line_start += line.len() + 1;
            let trimmed = line.trim();
            if trimmed.starts_with("//") {
                continue;
            }
            let hit = if let Some(m) = pattern.find(code_line) {
                let is_let_some = code_line[m.start()..].contains("let Some(")
                    && !code_line[m.start()..].contains("let Ok(")
                    && !pattern
                        .find_iter(code_line)
                        .any(|m| !m.as_str().contains("let Some("));
                if is_let_some {
                    // let-else only
                    let at = this_start + code_line.find("let Some(").unwrap();
                    let (open, _) = site_block(&code, at);
                    let stmt = &code_str[at..open];
                    (!if_let_some.is_match(&code_str[this_start..at + 9])
                        && open > at
                        && stmt.trim_end().ends_with("else"))
                    .then_some(at)
                } else {
                    Some(this_start + m.start())
                }
            } else if let Some(m) = quiet.find(code_line) {
                let at = this_start + m.start();
                let (_, close) = site_block(&code, at);
                // `return`/`continue` after it at the block's own depth, not in
                // a nested block or closure further down
                leaves.is_match(&own_depth(&code[at..close])).then_some(at)
            } else {
                None
            };
            let Some(at) = hit else { continue };
            let (open, close) = site_block(&code, at);
            // routed: the call is in the same block as the site, at that
            // block's own depth (not in a nested block)
            let routed = own_depth(&code[open + 1..close]).contains("record_decode_error(");
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
        "these sites skip data without calling record_decode_error in the same \
         block. route them through the decode error counter, or add them to \
         NOT_DECODE_ERRORS with a reason:\n{}",
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
