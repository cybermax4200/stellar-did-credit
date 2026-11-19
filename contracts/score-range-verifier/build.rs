use ark_bls12_381::{G1Affine, G2Affine};
use ark_serialize::CanonicalDeserialize;
use std::{env, fs, path::PathBuf};

fn point(source: &str, name: &str) -> Vec<u8> {
    let marker = format!("let {name} = [");
    let bytes = source.split_once(&marker).expect("missing verification key point").1
        .split_once("];").map(|(a, _)| a)
        .expect("unterminated verification key point");
    bytes.split(',').filter_map(|value| {
        let value = value.trim();
        if value.is_empty() { None } else {
            Some(u8::from_str_radix(value.strip_prefix("0x").expect("invalid key byte"), 16).expect("invalid key byte"))
        }
    }).collect()
}

fn main() {
    println!("cargo:rerun-if-changed=src/lib.rs");
    let path = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("src/lib.rs");
    let source = fs::read_to_string(path).expect("read verification key");
    for name in ["alpha_bytes", "gamma_abc_0", "gamma_abc_1", "gamma_abc_2", "gamma_abc_3", "gamma_abc_4", "gamma_abc_5", "gamma_abc_6"] {
        let bytes = point(&source, name);
        assert_eq!(bytes.len(), 96, "wrong size for {name}");
        let _ = G1Affine::deserialize_uncompressed(bytes.as_slice()).unwrap_or_else(|_| panic!("invalid G1 point {name}"));
    }
    for name in ["beta_bytes", "gamma_bytes", "delta_bytes"] {
        let bytes = point(&source, name);
        assert_eq!(bytes.len(), 192, "wrong size for {name}");
        let _ = G2Affine::deserialize_uncompressed(bytes.as_slice()).unwrap_or_else(|_| panic!("invalid G2 point {name}"));
    }
}
