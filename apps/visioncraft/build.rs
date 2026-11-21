//! Computes `GENERATOR_FINGERPRINT`: an FNV-1a hash over the source files listed in
//! `fingerprint-inputs.txt` (the code that determines the bytes `plant_cache` and `web_cache`
//! keep). It goes into `$OUT_DIR/generator_fingerprint.rs` as one `u64` literal.

use std::path::{Path, PathBuf};

const FNV0: u64 = 0xcbf2_9ce4_8422_2325;

fn fnv(h: u64, bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(h, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

/// Every file under `path` (itself when a file), in a stable order.
fn files(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
            .map(|e| e.expect("directory entry").path())
            .collect();
        entries.sort();
        for e in entries {
            files(&e, out);
        }
    } else {
        out.push(path.to_path_buf());
    }
}

fn main() {
    let list = "fingerprint-inputs.txt";
    println!("cargo:rerun-if-changed={list}");
    println!("cargo:rerun-if-changed=build.rs");
    let text = std::fs::read_to_string(list).expect("fingerprint-inputs.txt");
    let mut all = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let p = Path::new(line);
        assert!(p.exists(), "fingerprint input {line} does not exist");
        // A folder is watched as a whole, so a new file in it counts too.
        println!("cargo:rerun-if-changed={line}");
        files(p, &mut all);
    }
    let mut h = FNV0;
    for f in &all {
        println!("cargo:rerun-if-changed={}", f.display());
        let name = f.to_string_lossy().replace('\\', "/");
        let bytes = std::fs::read(f).unwrap_or_else(|e| panic!("cannot read {name}: {e}"));
        h = fnv(h, name.as_bytes());
        h = fnv(h, &(bytes.len() as u64).to_le_bytes());
        h = fnv(h, &bytes);
    }
    // Never zero, so a missing fingerprint cannot hide.
    let h = h.max(1);
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"))
        .join("generator_fingerprint.rs");
    std::fs::write(out, format!("{h:#018x}u64")).expect("write generator_fingerprint.rs");
}
