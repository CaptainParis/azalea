//! Architecture test for the WP2 panic audit
//! (`docs/backport/panic-audit.md` in azalea-hunt).
//!
//! Every place in the non-test code of the crates that decode or handle
//! network data that can panic must carry a `// network-safe: <reason>`
//! comment, saying why network input can't make it panic (for example
//! "length checked two lines above"), or be rewritten to return an error.
//! The comment goes on the site's line or in the comment lines just above
//! the statement the site is in. A new unannotated site makes this test
//! fail.
//!
//! Sites: `.unwrap()`, `.expect(..)`, `unwrap_unchecked`, `panic!`,
//! `unreachable!`, `todo!`, `unimplemented!`, `assert!`, `assert_eq!`,
//! `assert_ne!` (not `debug_assert!`: debug and fuzz builds only; overflow
//! checks are on there too, and the audit covers arithmetic by hand), and
//! indexing or slicing (`x[..]`).
//!
//! Test code (`#[cfg(test)]` items, `tests.rs` files) isn't scanned.

use std::{
    fs,
    path::{Path, PathBuf},
};

use regex::Regex;

/// Directories that are scanned, relative to `azalea-client`.
const SCANNED: &[&str] = &[
    "../azalea-protocol/src",
    "../azalea-buf/src",
    "../azalea-core/src",
    "../azalea-chat/src",
    "../azalea-world/src",
    "../azalea-entity/src",
    "../azalea-inventory/src",
    "src/plugins/packet",
    // the bad packet policy runs on every packet that failed to decode
    "src/plugins/bad_packet.rs",
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
    } else if path.extension().is_some_and(|e| e == "rs")
        && path.file_name().is_some_and(|n| n != "tests.rs")
    {
        out.push(path.to_owned());
    }
}

/// The source with comments and the contents of string and char literals
/// replaced by spaces (newlines kept), so byte offsets stay the same.
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
                let end = b[i..]
                    .iter()
                    .position(|&c| c == b'\n')
                    .map_or(b.len(), |n| i + n);
                blank(&mut out, i, end);
                i = end;
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = src[i + 2..].find("*/").map_or(b.len(), |n| i + 2 + n + 2);
                blank(&mut out, i, end);
                i = end;
            }
            b'r' if (b.get(i + 1) == Some(&b'#') || b.get(i + 1) == Some(&b'"'))
                && (i == 0 || !b[i - 1].is_ascii_alphanumeric() && b[i - 1] != b'_') =>
            {
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
                let end = src[j + 1..]
                    .find(&close)
                    .map_or(b.len(), |n| j + 1 + n + close.len());
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
                // char literal like '[' or '\''; lifetimes are left alone
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

/// Blank out every `#[cfg(test)]` item (the attribute through the end of
/// the item: its closing brace, or its semicolon for `mod tests;`).
fn blank_test_items(code: &mut [u8]) {
    let text = String::from_utf8_lossy(code).into_owned();
    let attribute = Regex::new(r"#\[cfg\((?:all\()?test\b[^\]]*\]").unwrap();
    let mut from = 0;
    while let Some(found) = attribute.find_at(&text, from) {
        let start = found.start();
        let mut depth = 0usize;
        let mut end = code.len();
        for (j, &c) in code.iter().enumerate().skip(found.end()) {
            match c {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = j + 1;
                        break;
                    }
                }
                b';' if depth == 0 => {
                    end = j + 1;
                    break;
                }
                _ => {}
            }
        }
        for c in &mut code[start..end] {
            if *c != b'\n' {
                *c = b' ';
            }
        }
        from = end;
    }
}

/// The panic sites in a file: (line number, line text).
fn panic_sites(contents: &str) -> Vec<(usize, String)> {
    let sites = Regex::new(
        r"\.unwrap\(\)|\.expect\(|\bunwrap_unchecked\b|\b(?:panic|unreachable|todo|unimplemented|assert|assert_eq|assert_ne)!|[\w)\]]\[",
    )
    .unwrap();
    let mut code = blank_non_code(contents);
    blank_test_items(&mut code);
    let code = String::from_utf8_lossy(&code).into_owned();
    let lines = contents.split('\n').collect::<Vec<_>>();
    let code_lines = code.split('\n').collect::<Vec<_>>();
    let annotated = |i: usize| {
        let note = |l: &str| {
            l.find("network-safe:")
                .is_some_and(|n| !l[n + "network-safe:".len()..].trim().is_empty())
        };
        if note(lines[i]) {
            return true;
        }
        // the comment lines above the statement the site is in
        let mut j = i;
        while j > 0 && i - j < 20 {
            j -= 1;
            let code_part = code_lines[j].trim_end();
            if lines[j].trim_start().starts_with("//") {
                if note(lines[j]) {
                    return true;
                }
                continue;
            }
            if code_part.ends_with(';') || code_part.ends_with('{') || code_part.ends_with('}') {
                return false;
            }
            if note(lines[j]) {
                return true;
            }
        }
        false
    };
    let mut out = Vec::new();
    for (i, code_line) in code_lines.iter().enumerate() {
        // attributes (`#[..]`) and macro brackets (`vec![..]`) aren't indexing
        let hit = sites.find_iter(code_line).any(|m| {
            let s = m.as_str();
            if s.ends_with('[') {
                let at = m.start();
                let before = &code_line[..at + 1];
                !before.trim_start().starts_with('#') && !before.ends_with('!')
            } else {
                true
            }
        });
        if hit && !annotated(i) {
            out.push((i + 1, lines[i].trim().to_owned()));
        }
    }
    out
}

#[test]
fn every_panic_site_is_network_safe() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for scanned in SCANNED {
        rust_files(&root.join(scanned), &mut files);
    }
    assert!(files.len() > 50);

    let mut unannotated = Vec::new();
    for file in &files {
        let rel = file
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let contents = fs::read_to_string(file).unwrap();
        for (line, text) in panic_sites(&contents) {
            unannotated.push(format!("{rel}:{line}: {text}"));
        }
    }
    assert!(
        unannotated.is_empty(),
        "{} panic sites without a `// network-safe: <reason>` comment. make \
         them return an error, or say why network input can't reach them:\n{}",
        unannotated.len(),
        unannotated.join("\n")
    );
}

/// The scanner itself: what counts as a site and what annotates one.
#[test]
fn scanner_finds_and_accepts_the_right_sites() {
    let src = r#"
fn a(v: &[u8], o: Option<u8>) -> u8 {
    let x = o.unwrap();
    // network-safe: checked by the caller
    let y = o.expect("y");
    let z = v[0]; // network-safe: v is never empty here
    let w = &v[1..];
    let s = "o.unwrap() in a string";
    debug_assert!(x > 0);
    let q = foo(
        // network-safe: two lines up
        o.unwrap(),
    );
    #[allow(unused)]
    let arr = vec![1, 2];
    // network-safe:
    panic!("empty reason doesn't count");
    x
}
#[cfg(test)]
mod tests {
    fn t() { None::<u8>.unwrap(); }
}
#[cfg(all(test, feature = "x"))]
mod more_tests {
    fn t() { None::<u8>.unwrap(); }
}
"#;
    let lines = panic_sites(src)
        .into_iter()
        .map(|(l, _)| l)
        .collect::<Vec<_>>();
    assert_eq!(lines, vec![3, 7, 17]);
}
