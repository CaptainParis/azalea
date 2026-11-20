//! Write the hand-written deep seeds into a corpus directory, as
//! `<dir>/<target>/<name>`: `cargo run -p azalea-fuzz --example write_seeds
//! -- <corpus dir>`.

fn main() {
    let dir = std::env::args().nth(1).expect("usage: write_seeds <corpus dir>");
    let mut n = 0;
    for seed in azalea_fuzz::seeds::all() {
        let target = std::path::Path::new(&dir).join(seed.target);
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join(seed.name), &seed.data).unwrap();
        n += 1;
    }
    println!("wrote {n} seeds to {dir}");
}
