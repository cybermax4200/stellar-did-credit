//! Groth16 Phase 2 ceremony CLI. See `docs/zk-trusted-setup.md`.
//!
//! ```text
//! ceremony init <circuit> <dir>            create params_0000.bin + transcript.txt
//! ceremony contribute <dir> <name>         add a contribution (fresh OS randomness)
//! ceremony beacon <dir> <name> <beacon>    add the final public-randomness contribution
//! ceremony verify <dir>                    fully verify every step from the key files
//! ceremony export <dir> <out-prefix>       write <prefix>.txt (Rust arrays) + <prefix>.json
//! ```
//!
//! Each participant runs `contribute` on their own machine on the latest
//! `params_NNNN.bin`, then publishes the new params file and transcript line.
//! `init` is a single-party stand-in for Phase 1: see the warning it prints.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::exit;

use ark_std::rand::rngs::{OsRng, StdRng};
use ark_std::rand::{RngCore, SeedableRng};
use zk_circuit::ceremony::{
    contribute, contribute_beacon, initial_params, params_from_bytes, params_hash, params_to_bytes,
    verify_contribution, verify_params_consistency, vk_to_json, vk_to_rust_arrays, CeremonyError,
    Transcript, CIRCUITS,
};

const USAGE: &str = "usage:
  ceremony init <circuit> <dir>
  ceremony contribute <dir> <name>
  ceremony beacon <dir> <name> <beacon>
  ceremony verify <dir>
  ceremony export <dir> <out-prefix>
circuits: threshold, tier";

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {msg}");
    exit(1);
}

fn params_path(dir: &Path, index: usize) -> PathBuf {
    dir.join(format!("params_{index:04}.bin"))
}

fn read_transcript(dir: &Path) -> Transcript {
    let text = fs::read_to_string(dir.join("transcript.txt"))
        .unwrap_or_else(|e| fail(format!("cannot read transcript.txt: {e}")));
    Transcript::from_text(&text).unwrap_or_else(|e| fail(e))
}

fn write_transcript(dir: &Path, t: &Transcript) {
    fs::write(dir.join("transcript.txt"), t.to_text())
        .unwrap_or_else(|e| fail(format!("cannot write transcript.txt: {e}")));
}

fn load_params(dir: &Path, index: usize) -> ark_groth16::ProvingKey<ark_bls12_381::Bls12_381> {
    let bytes = fs::read(params_path(dir, index)).unwrap_or_else(|e| {
        fail(format!(
            "cannot read {}: {e}",
            params_path(dir, index).display()
        ))
    });
    params_from_bytes(&bytes).unwrap_or_else(|e| fail(e))
}

/// A verifier RNG that is not derived from anything the contributor controls.
fn verifier_rng() -> StdRng {
    let mut seed = [0u8; 32];
    OsRng.fill_bytes(&mut seed);
    StdRng::from_seed(seed)
}

fn cmd_init(circuit: &str, dir: &Path) {
    if !CIRCUITS.contains(&circuit) {
        fail(CeremonyError::UnknownCircuit(circuit.to_string()));
    }
    if dir.join("transcript.txt").exists() {
        fail("transcript.txt already exists in that directory");
    }
    fs::create_dir_all(dir).unwrap_or_else(|e| fail(e));
    eprintln!(
        "WARNING: `init` is a SINGLE-PARTY stand-in for Phase 1. This process knows tau, alpha and\n\
         beta, and anyone who knows tau can forge proofs regardless of the Phase 2 contributions.\n\
         Keys from this run are for rehearsal only. A production ceremony must start from a public\n\
         powers-of-tau transcript. See docs/zk-trusted-setup.md."
    );
    let pk = initial_params(circuit, &mut OsRng).unwrap_or_else(|e| fail(e));
    fs::write(params_path(dir, 0), params_to_bytes(&pk)).unwrap_or_else(|e| fail(e));
    write_transcript(dir, &Transcript::new(circuit, &pk));
    println!("initialized {circuit} ceremony in {}", dir.display());
    println!("params_0000 hash: {}", hex(&params_hash(&pk)));
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Apply a contribution produced by `make`, self-verify it, then persist it.
fn append(
    dir: &Path,
    make: impl FnOnce(
        &ark_groth16::ProvingKey<ark_bls12_381::Bls12_381>,
    ) -> Result<
        (
            ark_groth16::ProvingKey<ark_bls12_381::Bls12_381>,
            zk_circuit::ceremony::Contribution,
        ),
        CeremonyError,
    >,
) {
    let mut transcript = read_transcript(dir);
    let index = transcript.contributions.len();
    let old = load_params(dir, index);
    if params_hash(&old) != transcript.head_hash() {
        fail("latest params file does not match the transcript head");
    }
    let (new, contribution) = make(&old).unwrap_or_else(|e| fail(e));
    verify_contribution(&old, &new, &contribution, &mut verifier_rng())
        .unwrap_or_else(|e| fail(format!("self-check failed: {e}")));
    fs::write(params_path(dir, index + 1), params_to_bytes(&new)).unwrap_or_else(|e| fail(e));
    println!(
        "contribution {} by `{}` written\nparams hash: {}",
        index + 1,
        contribution.name,
        hex(&contribution.params_hash)
    );
    transcript.contributions.push(contribution);
    write_transcript(dir, &transcript);
}

fn cmd_verify(dir: &Path) {
    let transcript = read_transcript(dir);
    transcript.verify_delta_chain().unwrap_or_else(|e| fail(e));

    let mut prev = load_params(dir, 0);
    if params_hash(&prev) != transcript.initial_hash {
        fail("params_0000.bin does not match the transcript's initial hash");
    }
    verify_params_consistency(&prev).unwrap_or_else(|e| fail(e));
    let mut rng = verifier_rng();
    for (i, c) in transcript.contributions.iter().enumerate() {
        let next = load_params(dir, i + 1);
        verify_contribution(&prev, &next, c, &mut rng)
            .unwrap_or_else(|e| fail(format!("contribution {} (`{}`): {e}", i + 1, c.name)));
        println!("contribution {} (`{}`): OK", i + 1, c.name);
        prev = next;
    }
    verify_params_consistency(&prev).unwrap_or_else(|e| fail(e));
    println!(
        "ceremony verified: {} contribution(s), final params hash {}",
        transcript.contributions.len(),
        hex(&params_hash(&prev))
    );
}

fn cmd_export(dir: &Path, prefix: &str) {
    cmd_verify(dir);
    let transcript = read_transcript(dir);
    if transcript.contributions.is_empty() {
        fail("refusing to export a key with no contributions");
    }
    let pk = load_params(dir, transcript.contributions.len());
    let txt = format!(
        "// Groth16 verification key from a Phase 2 ceremony ({} circuit, {} contribution(s)).\n\
         // Final params hash: {}\n\
         // Generated by `ceremony export`; see docs/zk-trusted-setup.md for the trust assumptions.\n\n{}",
        transcript.circuit,
        transcript.contributions.len(),
        hex(&params_hash(&pk)),
        vk_to_rust_arrays(&pk.vk)
    );
    fs::write(format!("{prefix}.txt"), txt).unwrap_or_else(|e| fail(e));
    fs::write(
        format!("{prefix}.json"),
        vk_to_json(&transcript.circuit, &pk.vk),
    )
    .unwrap_or_else(|e| fail(e));
    println!("wrote {prefix}.txt and {prefix}.json");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    match a.as_slice() {
        ["init", circuit, dir] => cmd_init(circuit, Path::new(dir)),
        ["contribute", dir, name] => {
            append(Path::new(dir), |old| contribute(old, name, &mut OsRng))
        }
        ["beacon", dir, name, beacon] => append(Path::new(dir), |old| {
            contribute_beacon(old, name, beacon.as_bytes())
        }),
        ["verify", dir] => cmd_verify(Path::new(dir)),
        ["export", dir, prefix] => cmd_export(Path::new(dir), prefix),
        _ => {
            eprintln!("{USAGE}");
            exit(2);
        }
    }
}
