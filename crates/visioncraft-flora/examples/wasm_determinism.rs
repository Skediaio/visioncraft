//! Cross-platform determinism check. Build for `wasm32-unknown-unknown` and call the exported
//! `main` from any WebAssembly host (e.g. node): it returns normally when the meshes hash to the
//! same values as on x86_64 (see `golden_hashes` in tests/flora.rs) and traps otherwise.
//! Natively it just runs the same check.

use visioncraft_flora::{Lod, PlantMesh, generate, species};

fn fnv(m: &PlantMesh) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: u32| {
        for byte in b.to_le_bytes() {
            h ^= byte as u64;
            h = h.wrapping_mul(0x100_0000_01b3);
        }
    };
    let floats = m
        .positions
        .iter()
        .flatten()
        .chain(m.normals.iter().flatten());
    floats
        .chain(m.uvs.iter().flatten())
        .chain(m.wind.iter().flatten())
        .for_each(|v| eat(v.to_bits()));
    m.indices.iter().for_each(|&i| eat(i));
    h
}

fn main() {
    let lib = species();
    for (id, lod, want) in [
        ("quercus-robur", Lod::Lod0, 0x9750_e973_aae9_5481_u64),
        ("hosta", Lod::Lod1, 0xa334_e697_a230_f815),
        ("carpinus-hedge", Lod::Lod2, 0xc567_f698_a41b_6c32),
        ("picea-abies", Lod::Lod1, 0xdd72_8a9a_25eb_1bc2),
    ] {
        let s = lib.iter().find(|s| s.id == id).expect("species");
        assert_eq!(fnv(&generate(&s.params, 1234, lod)), want, "{id}");
    }
}
