use ark_bls12_381::{G1Affine, G2Affine};
use ark_serialize::CanonicalDeserialize;
use std::{env, fs, path::PathBuf};

fn point(source: &str, name: &str) -> Vec<u8> {
    let marker = format!("let {name} = [");
    let bytes = source
        .split_once(&marker)
        .expect("missing verification key point")
        .1
        .split_once("];")
        .map(|(a, _)| a)
        .expect("unterminated verification key point");
    bytes
        .split(',')
        .filter_map(|value| {
            let value = value.trim();
            if value.is_empty() {
                None
            } else {
                Some(
                    u8::from_str_radix(value.strip_prefix("0x").expect("invalid key byte"), 16)
                        .expect("invalid key byte"),
                )
            }
        })
        .collect()
}

/// The source of the function whose signature starts with `signature`, up to
/// (but excluding) the start of `next_signature`.
fn function_source<'a>(source: &'a str, signature: &str, next_signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("missing `{signature}`"));
    let rest = &source[start..];
    let end = rest.find(next_signature).unwrap_or(rest.len());
    &rest[..end]
}

/// `NUM_PUBLIC_INPUTS + 1` points: one per public input plus the constant term.
const GAMMA_ABC_POINTS: usize = 9;

fn check_key(source: &str, which: &str) {
    for name in std::iter::once("alpha_bytes".to_string())
        .chain((0..GAMMA_ABC_POINTS).map(|i| format!("gamma_abc_{i}")))
    {
        let bytes = point(source, &name);
        assert_eq!(bytes.len(), 96, "wrong size for {which} {name}");
        let _ = G1Affine::deserialize_uncompressed(bytes.as_slice())
            .unwrap_or_else(|_| panic!("invalid G1 point {which} {name}"));
    }
    for name in ["beta_bytes", "gamma_bytes", "delta_bytes"] {
        let bytes = point(source, name);
        assert_eq!(bytes.len(), 192, "wrong size for {which} {name}");
        let _ = G2Affine::deserialize_uncompressed(bytes.as_slice())
            .unwrap_or_else(|_| panic!("invalid G2 point {which} {name}"));
    }
}

fn main() {
    println!("cargo:rerun-if-changed=src/lib.rs");
    let path = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("src/lib.rs");
    let source = fs::read_to_string(path).expect("read verification key");

    // Validate both embedded keys: the threshold circuit's and the tier-band
    // circuit's. Each must be well-formed and on the curve / in the subgroup.
    let threshold = function_source(
        &source,
        "fn embedded_verification_key(",
        "fn embedded_tier_verification_key(",
    );
    check_key(threshold, "threshold VK");
    let tier = function_source(
        &source,
        "fn embedded_tier_verification_key(",
        "/// Encode a u32",
    );
    check_key(tier, "tier VK");
}
