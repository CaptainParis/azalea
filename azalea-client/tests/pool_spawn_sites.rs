//! Every task Azalea puts on Bevy's shared pools goes through
//! `task_pool::owner::{spawn_io, spawn_async_compute}`, which carries the bot
//! id and reports panics (WP2 section 1). Fails on any other use of the pools
//! in the non-test code of the Azalea crates. The list of spawn sites and
//! their time caps is in the hunt repo's docs/backport/panic-audit.md.

use std::path::{Path, PathBuf};

const POOLS: &[&str] = &["IoTaskPool", "AsyncComputeTaskPool", "ComputeTaskPool"];
/// Where the pools may be named: their setup and the owned spawn functions.
const ALLOWED: &[&str] = &["azalea-client/src/plugins/task_pool.rs"];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Lines of `code` that name a pool, outside `#[cfg(test)]` modules (a
/// rough cut: everything after the first `#[cfg(test)]` is test code).
fn pool_uses(code: &str) -> Vec<(usize, String)> {
    code.lines()
        .enumerate()
        .take_while(|(_, l)| !l.trim_start().starts_with("#[cfg(test)]"))
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
        .filter(|(_, l)| POOLS.iter().any(|p| l.contains(p)))
        .map(|(i, l)| (i + 1, l.trim().to_string()))
        .collect()
}

#[test]
fn pool_spawns_go_through_the_owned_wrapper() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    for e in std::fs::read_dir(&root).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name == "azalea" || name.starts_with("azalea-") {
            let src = e.path().join("src");
            if src.is_dir() {
                sources(&src, &mut files);
            }
        }
    }
    assert!(files.len() > 100, "found only {} source files", files.len());
    let mut bad = Vec::new();
    for f in files {
        let rel = f
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if ALLOWED.contains(&rel.as_str()) {
            continue;
        }
        for (line, text) in pool_uses(&std::fs::read_to_string(&f).unwrap()) {
            bad.push(format!("{rel}:{line}: {text}"));
        }
    }
    assert!(
        bad.is_empty(),
        "use task_pool::owner::spawn_io / spawn_async_compute (bot id, panic report, and a time cap \
         listed in panic-audit.md):\n{}",
        bad.join("\n")
    );
}

#[test]
fn the_check_sees_a_direct_spawn() {
    let code = "fn f() {\n    IoTaskPool::get().spawn(async {}).detach();\n}\n#[cfg(test)]\nmod t { use AsyncComputeTaskPool; }\n";
    assert_eq!(pool_uses(code).len(), 1);
}
