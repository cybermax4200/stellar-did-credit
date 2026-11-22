//! Groth16 Phase 2 ceremony tooling (multi-party `delta` contributions).
//!
//! A Groth16 proving/verifying key pair is circuit specific and is derived from
//! secret randomness ("toxic waste"). Anyone who learns all of it can forge
//! proofs. A multi-party ceremony removes the single point of trust: each
//! participant mixes in fresh randomness and destroys it, and the result is
//! sound if **at least one** participant was honest.
//!
//! This module implements the circuit-specific **Phase 2** of the
//! Zcash/Bowe-style ceremony: every contribution re-randomizes `delta`,
//!
//! ```text
//! delta_g1'  = d * delta_g1            vk.delta_g2' = d * vk.delta_g2
//! h_query'   = d^-1 * h_query          l_query'     = d^-1 * l_query
//! ```
//!
//! and publishes `d * gamma_g2` with a proof of knowledge of `d`. Everything else in
//! the key (`alpha`, `beta`, `gamma`, the QAP queries) is untouched. (arkworks'
//! Groth16 setup draws *random* G1/G2 bases rather than the curve generators,
//! so all pairing checks below are expressed relative to elements already in
//! the key, e.g. `e(delta_g1, beta_g2) == e(beta_g1, delta_g2)`.) Each step
//! is publicly verifiable with [`verify_contribution`] (pairing checks, no
//! secrets), and a final public random *beacon* contribution can be added with
//! [`contribute_beacon`] so that not even the last participant controls the
//! outcome.
//!
//! # What this does NOT provide
//!
//! Phase 2 only protects `delta`. The soundness of the key also depends on the
//! secrecy of the Phase 1 values (`tau`, `alpha`, `beta`): whoever knows `tau`
//! can forge proofs no matter what `delta` is. [`initial_params`] is a
//! **single-party stand-in for Phase 1** (the operator running it knows those
//! values). A production key must start from the output of a public
//! powers-of-tau ceremony instead. See `docs/zk-trusted-setup.md`.

use ark_bls12_381::{Bls12_381, Fr, G1Affine, G1Projective, G2Affine, G2Projective};
use ark_ec::pairing::Pairing;
use ark_ec::{AffineRepr, CurveGroup, VariableBaseMSM};
use ark_ff::{Field, PrimeField, UniformRand, Zero};
use ark_groth16::{ProvingKey, VerifyingKey};
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use ark_std::rand::{CryptoRng, RngCore};
use sha2::{Digest, Sha256};
use std::fmt;

/// Circuits a ceremony can be run for.
pub const CIRCUITS: [&str; 2] = ["threshold", "tier"];

/// 32-byte SHA-256 digest.
pub type Hash32 = [u8; 32];

/// Why a ceremony operation or verification failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CeremonyError {
    /// Unknown circuit name.
    UnknownCircuit(String),
    /// Contributor names are limited to `[A-Za-z0-9_.-]`, 1..=64 chars.
    InvalidName,
    /// Circuit setup failed.
    Setup(String),
    /// The proof of knowledge of the contribution's secret did not verify.
    BadProofOfKnowledge,
    /// A contribution public key was the point at infinity.
    IdentityPoint(&'static str),
    /// `delta_g1` and `delta_g2` do not share the same exponent.
    DeltaMismatch,
    /// The new `delta` is not the old `delta` times the contributed secret.
    DeltaNotUpdated,
    /// `h_query` / `l_query` were not scaled by the inverse of the secret.
    QueryNotUpdated(&'static str),
    /// A component that must not change did change (or lengths differ).
    UnexpectedChange(&'static str),
    /// A key is internally inconsistent (G1/G2 halves disagree).
    InconsistentKey(&'static str),
    /// Transcript hash chain or recorded hash does not match.
    HashChain(String),
    /// Transcript text could not be parsed.
    Parse(String),
    /// (De)serialization error.
    Serialization(String),
}

impl fmt::Display for CeremonyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use CeremonyError::*;
        match self {
            UnknownCircuit(c) => write!(f, "unknown circuit `{c}` (expected threshold or tier)"),
            InvalidName => write!(f, "name must be 1-64 chars of [A-Za-z0-9_.-]"),
            Setup(e) => write!(f, "circuit setup failed: {e}"),
            BadProofOfKnowledge => write!(f, "proof of knowledge of the contribution is invalid"),
            IdentityPoint(w) => write!(f, "{w} is the point at infinity"),
            DeltaMismatch => write!(f, "delta_g1 and delta_g2 are inconsistent"),
            DeltaNotUpdated => write!(f, "delta was not updated by the contributed secret"),
            QueryNotUpdated(q) => write!(f, "{q} was not scaled by the inverse of the secret"),
            UnexpectedChange(w) => {
                write!(f, "{w} changed, but a contribution may only touch delta")
            }
            InconsistentKey(w) => write!(f, "inconsistent key: {w}"),
            HashChain(e) => write!(f, "hash chain error: {e}"),
            Parse(e) => write!(f, "transcript parse error: {e}"),
            Serialization(e) => write!(f, "serialization error: {e}"),
        }
    }
}

impl std::error::Error for CeremonyError {}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/// Initial parameters for `circuit` (`threshold` or `tier`).
///
/// **Single-party Phase 1 stand-in**: the caller's `rng` determines all
/// secrets, so whoever runs this knows the toxic waste. Use it only to start a
/// rehearsal, or replace it with parameters derived from a public Phase 1
/// transcript for a production ceremony.
pub fn initial_params<R: RngCore + CryptoRng>(
    circuit: &str,
    rng: &mut R,
) -> Result<ProvingKey<Bls12_381>, CeremonyError> {
    let keys = match circuit {
        "threshold" => crate::generate_test_keys(rng),
        "tier" => crate::generate_tier_test_keys(rng),
        other => return Err(CeremonyError::UnknownCircuit(other.to_string())),
    };
    keys.map(|(pk, _)| pk)
        .map_err(|e| CeremonyError::Setup(e.to_string()))
}

/// SHA-256 of the canonical (compressed) serialization of the full key.
pub fn params_hash(pk: &ProvingKey<Bls12_381>) -> Hash32 {
    let mut bytes = Vec::new();
    pk.serialize_compressed(&mut bytes)
        .expect("serialization into a Vec cannot fail");
    Sha256::digest(&bytes).into()
}

/// Serialize parameters (compressed).
pub fn params_to_bytes(pk: &ProvingKey<Bls12_381>) -> Vec<u8> {
    let mut bytes = Vec::new();
    pk.serialize_compressed(&mut bytes)
        .expect("serialization into a Vec cannot fail");
    bytes
}

/// Deserialize parameters, validating every curve point.
pub fn params_from_bytes(bytes: &[u8]) -> Result<ProvingKey<Bls12_381>, CeremonyError> {
    ProvingKey::deserialize_compressed(bytes)
        .map_err(|e| CeremonyError::Serialization(e.to_string()))
}

// ---------------------------------------------------------------------------
// Contributions
// ---------------------------------------------------------------------------

/// A public record of one contribution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contribution {
    /// Participant label, `[A-Za-z0-9_.-]{1,64}`.
    pub name: String,
    /// `d * vk.gamma_g2`, the contribution's public key.
    pub delta_g2: G2Affine,
    /// Schnorr commitment `k * vk.gamma_g2` of the proof of knowledge of `d`.
    pub pok_commitment: G2Affine,
    /// Schnorr response `k + c * d`.
    pub pok_response: Fr,
    /// Hash of the parameters this contribution was applied to.
    pub prev_hash: Hash32,
    /// Hash of the resulting parameters.
    pub params_hash: Hash32,
    /// `delta_g1` of the resulting parameters (lets a transcript alone be
    /// checked for the `delta` chain without the multi-megabyte key files).
    pub new_delta_g1: G1Affine,
    /// `vk.delta_g2` of the resulting parameters.
    pub new_delta_g2: G2Affine,
}

fn check_name(name: &str) -> Result<(), CeremonyError> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(CeremonyError::InvalidName)
    }
}

fn point_bytes<T: CanonicalSerialize>(p: &T) -> Vec<u8> {
    let mut v = Vec::new();
    p.serialize_compressed(&mut v)
        .expect("serialization into a Vec cannot fail");
    v
}

/// Fiat-Shamir challenge binding the proof of knowledge to the previous
/// parameters, the contributor name and both curve points.
fn pok_challenge(prev_hash: &Hash32, name: &str, delta_g2: &G2Affine, commitment: &G2Affine) -> Fr {
    let mut h = Sha256::new();
    h.update(b"stellar-did-credit/ceremony/pok/v1");
    h.update(prev_hash);
    h.update((name.len() as u32).to_be_bytes());
    h.update(name.as_bytes());
    h.update(point_bytes(delta_g2));
    h.update(point_bytes(commitment));
    Fr::from_be_bytes_mod_order(&h.finalize())
}

fn scale_g1(points: &[G1Affine], s: Fr) -> Vec<G1Affine> {
    let projective: Vec<G1Projective> = points.iter().map(|p| *p * s).collect();
    G1Projective::normalize_batch(&projective)
}

/// Apply one contribution: draws a fresh non-zero secret `d` from `rng`,
/// updates the parameters and returns them with the public record. The secret
/// is dropped when this function returns.
pub fn contribute<R: RngCore + CryptoRng>(
    pk: &ProvingKey<Bls12_381>,
    name: &str,
    rng: &mut R,
) -> Result<(ProvingKey<Bls12_381>, Contribution), CeremonyError> {
    check_name(name)?;
    let prev_hash = params_hash(pk);

    let mut d = Fr::rand(rng);
    while d.is_zero() {
        d = Fr::rand(rng);
    }
    let d_inv = d.inverse().expect("d is non-zero");

    let mut new = pk.clone();
    new.delta_g1 = (pk.delta_g1 * d).into_affine();
    new.vk.delta_g2 = (pk.vk.delta_g2 * d).into_affine();
    new.h_query = scale_g1(&pk.h_query, d_inv);
    new.l_query = scale_g1(&pk.l_query, d_inv);

    // Proof of knowledge of d (Schnorr over G2, Fiat-Shamir).
    let base = pk.vk.gamma_g2;
    let delta_pub = (base * d).into_affine();
    let mut k = Fr::rand(rng);
    while k.is_zero() {
        k = Fr::rand(rng);
    }
    let commitment = (base * k).into_affine();
    let c = pok_challenge(&prev_hash, name, &delta_pub, &commitment);
    let response = k + c * d;

    let contribution = Contribution {
        name: name.to_string(),
        delta_g2: delta_pub,
        pok_commitment: commitment,
        pok_response: response,
        prev_hash,
        params_hash: params_hash(&new),
        new_delta_g1: new.delta_g1,
        new_delta_g2: new.vk.delta_g2,
    };
    Ok((new, contribution))
}

/// Final contribution whose randomness is a public *beacon* (for example the
/// hash of a Stellar ledger or Bitcoin block that did not exist when the last
/// human contribution was made). The secret is derived deterministically from
/// `beacon`, so anyone can recompute and audit it; its purpose is to ensure the
/// last human participant could not steer the final parameters.
pub fn contribute_beacon(
    pk: &ProvingKey<Bls12_381>,
    name: &str,
    beacon: &[u8],
) -> Result<(ProvingKey<Bls12_381>, Contribution), CeremonyError> {
    use ark_std::rand::rngs::StdRng;
    use ark_std::rand::SeedableRng;
    let mut h = Sha256::new();
    h.update(b"stellar-did-credit/ceremony/beacon/v1");
    h.update(beacon);
    let mut rng = StdRng::from_seed(h.finalize().into());
    contribute(pk, name, &mut rng)
}

fn msm_g1(points: &[G1Affine], scalars: &[Fr]) -> G1Projective {
    if points.is_empty() {
        return G1Projective::zero();
    }
    G1Projective::msm(points, scalars).expect("equal lengths")
}

/// Publicly verify that `new` is `old` with exactly the contribution `c`
/// applied. Uses only public data. `rng` supplies the verifier's own
/// randomness for batched checks of the query vectors (it need not be secret
/// from anyone but must not be predictable by the contributor).
pub fn verify_contribution<R: RngCore + CryptoRng>(
    old: &ProvingKey<Bls12_381>,
    new: &ProvingKey<Bls12_381>,
    c: &Contribution,
    rng: &mut R,
) -> Result<(), CeremonyError> {
    check_name(&c.name)?;
    if c.delta_g2.is_zero() {
        return Err(CeremonyError::IdentityPoint("contribution public key"));
    }
    if new.delta_g1.is_zero() || new.vk.delta_g2.is_zero() {
        return Err(CeremonyError::IdentityPoint("delta"));
    }

    // 0. Hash chain.
    if c.prev_hash != params_hash(old) {
        return Err(CeremonyError::HashChain(
            "contribution does not extend the supplied previous parameters".into(),
        ));
    }
    if c.params_hash != params_hash(new) {
        return Err(CeremonyError::HashChain(
            "recorded hash does not match the resulting parameters".into(),
        ));
    }

    // 1. Proof of knowledge of d (base: the key's own gamma_g2):
    //    gamma_g2 * s == R + c * (d * gamma_g2).
    let base = old.vk.gamma_g2;
    let ch = pok_challenge(&c.prev_hash, &c.name, &c.delta_g2, &c.pok_commitment);
    let lhs = base * c.pok_response;
    let rhs = G2Projective::from(c.pok_commitment) + c.delta_g2 * ch;
    if lhs != rhs {
        return Err(CeremonyError::BadProofOfKnowledge);
    }

    // 2. delta_g1 and delta_g2 carry the same exponent:
    //    e(delta_g1, beta_g2) == e(beta_g1, delta_g2). (beta is unchanged, see 5.)
    let e = Bls12_381::pairing;
    if e(new.delta_g1, old.vk.beta_g2) != e(old.beta_g1, new.vk.delta_g2) {
        return Err(CeremonyError::DeltaMismatch);
    }
    // 3. That exponent is (old delta) * d:
    //    e(old_delta_g1, d * gamma_g2) == e(new_delta_g1, gamma_g2).
    if e(old.delta_g1, c.delta_g2) != e(new.delta_g1, base) {
        return Err(CeremonyError::DeltaNotUpdated);
    }

    // 4. h_query and l_query were scaled by d^-1. With delta_new = d * delta_old,
    //    e(q_new, delta_new_g2) == e(q_old, delta_old_g2) holds iff q_new = d^-1 q_old.
    //    Checked on a random linear combination of all entries.
    for (label, old_q, new_q) in [
        ("h_query", &old.h_query, &new.h_query),
        ("l_query", &old.l_query, &new.l_query),
    ] {
        if old_q.len() != new_q.len() {
            return Err(CeremonyError::UnexpectedChange(label));
        }
        let r: Vec<Fr> = (0..old_q.len()).map(|_| Fr::rand(rng)).collect();
        let a = msm_g1(old_q, &r).into_affine();
        let b = msm_g1(new_q, &r).into_affine();
        if e(b, new.vk.delta_g2) != e(a, old.vk.delta_g2) {
            return Err(CeremonyError::QueryNotUpdated(label));
        }
    }

    // 5. Nothing else may change.
    macro_rules! same {
        ($label:literal, $a:expr, $b:expr) => {
            if $a != $b {
                return Err(CeremonyError::UnexpectedChange($label));
            }
        };
    }
    same!("vk.alpha_g1", old.vk.alpha_g1, new.vk.alpha_g1);
    same!("vk.beta_g2", old.vk.beta_g2, new.vk.beta_g2);
    same!("vk.gamma_g2", old.vk.gamma_g2, new.vk.gamma_g2);
    same!("vk.gamma_abc_g1", old.vk.gamma_abc_g1, new.vk.gamma_abc_g1);
    same!("beta_g1", old.beta_g1, new.beta_g1);
    same!("a_query", old.a_query, new.a_query);
    same!("b_g1_query", old.b_g1_query, new.b_g1_query);
    same!("b_g2_query", old.b_g2_query, new.b_g2_query);

    // 6. The record's cached deltas match the parameters.
    if c.new_delta_g1 != new.delta_g1 || c.new_delta_g2 != new.vk.delta_g2 {
        return Err(CeremonyError::UnexpectedChange("recorded delta"));
    }
    Ok(())
}

/// Internal consistency checks on a single key (no history needed): no key
/// point is the identity and the G1 and G2 copies of `delta` carry the same
/// exponent, `e(delta_g1, beta_g2) == e(beta_g1, delta_g2)`.
pub fn verify_params_consistency(pk: &ProvingKey<Bls12_381>) -> Result<(), CeremonyError> {
    if pk.vk.alpha_g1.is_zero() || pk.vk.beta_g2.is_zero() || pk.vk.gamma_g2.is_zero() {
        return Err(CeremonyError::IdentityPoint("alpha/beta/gamma"));
    }
    if pk.delta_g1.is_zero() || pk.vk.delta_g2.is_zero() {
        return Err(CeremonyError::IdentityPoint("delta"));
    }
    let e = Bls12_381::pairing;
    if e(pk.delta_g1, pk.vk.beta_g2) != e(pk.beta_g1, pk.vk.delta_g2) {
        return Err(CeremonyError::InconsistentKey("delta_g1 vs delta_g2"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Transcript
// ---------------------------------------------------------------------------

/// The public, append-only record of a ceremony.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transcript {
    /// Circuit name (see [`CIRCUITS`]).
    pub circuit: String,
    /// Hash of the initial parameters.
    pub initial_hash: Hash32,
    /// `delta_g1` of the initial parameters.
    pub initial_delta_g1: G1Affine,
    /// `vk.delta_g2` of the initial parameters.
    pub initial_delta_g2: G2Affine,
    /// `beta_g1` (unchanged by contributions; base for the delta checks).
    pub beta_g1: G1Affine,
    /// `vk.beta_g2` (unchanged by contributions).
    pub beta_g2: G2Affine,
    /// `vk.gamma_g2` (unchanged; base for contribution public keys).
    pub gamma_g2: G2Affine,
    /// Contributions in order. The last one may be the beacon.
    pub contributions: Vec<Contribution>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Result<Vec<u8>, CeremonyError> {
    if s.len() & 1 == 1 {
        return Err(CeremonyError::Parse("odd-length hex".into()));
    }
    (0..s.len() / 2)
        .map(|i| {
            u8::from_str_radix(&s[2 * i..2 * i + 2], 16)
                .map_err(|_| CeremonyError::Parse("invalid hex".into()))
        })
        .collect()
}

fn de<T: CanonicalDeserialize>(s: &str) -> Result<T, CeremonyError> {
    let bytes = unhex(s)?;
    T::deserialize_compressed(&bytes[..]).map_err(|e| CeremonyError::Parse(e.to_string()))
}

fn hash_from(s: &str) -> Result<Hash32, CeremonyError> {
    unhex(s)?
        .try_into()
        .map_err(|_| CeremonyError::Parse("hash must be 32 bytes".into()))
}

impl Transcript {
    /// Start a transcript for freshly generated initial parameters.
    pub fn new(circuit: &str, initial: &ProvingKey<Bls12_381>) -> Self {
        Self {
            circuit: circuit.to_string(),
            initial_hash: params_hash(initial),
            initial_delta_g1: initial.delta_g1,
            initial_delta_g2: initial.vk.delta_g2,
            beta_g1: initial.beta_g1,
            beta_g2: initial.vk.beta_g2,
            gamma_g2: initial.vk.gamma_g2,
            contributions: Vec::new(),
        }
    }

    /// Hash of the latest parameters according to the transcript.
    pub fn head_hash(&self) -> Hash32 {
        self.contributions
            .last()
            .map(|c| c.params_hash)
            .unwrap_or(self.initial_hash)
    }

    /// Plain-text, line-oriented encoding (no external serialization crate).
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str("# stellar-did-credit Groth16 Phase 2 ceremony transcript v1\n");
        out.push_str(&format!("circuit={}\n", self.circuit));
        out.push_str(&format!("initial_hash={}\n", hex(&self.initial_hash)));
        out.push_str(&format!(
            "initial_delta_g1={}\n",
            hex(&point_bytes(&self.initial_delta_g1))
        ));
        out.push_str(&format!(
            "initial_delta_g2={}\n",
            hex(&point_bytes(&self.initial_delta_g2))
        ));
        out.push_str(&format!("beta_g1={}\n", hex(&point_bytes(&self.beta_g1))));
        out.push_str(&format!("beta_g2={}\n", hex(&point_bytes(&self.beta_g2))));
        out.push_str(&format!("gamma_g2={}\n", hex(&point_bytes(&self.gamma_g2))));
        for c in &self.contributions {
            out.push_str(&format!(
                "contribution name={} delta_g2={} pok_r={} pok_s={} prev_hash={} params_hash={} new_delta_g1={} new_delta_g2={}\n",
                c.name,
                hex(&point_bytes(&c.delta_g2)),
                hex(&point_bytes(&c.pok_commitment)),
                hex(&point_bytes(&c.pok_response)),
                hex(&c.prev_hash),
                hex(&c.params_hash),
                hex(&point_bytes(&c.new_delta_g1)),
                hex(&point_bytes(&c.new_delta_g2)),
            ));
        }
        out
    }

    /// Parse the output of [`Transcript::to_text`].
    pub fn from_text(text: &str) -> Result<Self, CeremonyError> {
        let mut circuit = None;
        let mut initial_hash = None;
        let mut d1 = None;
        let mut d2 = None;
        let (mut b1, mut b2, mut gm2) = (None, None, None);
        let mut contributions = Vec::new();
        for line in text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            if let Some(rest) = line.strip_prefix("contribution ") {
                let mut f = std::collections::HashMap::new();
                for kv in rest.split_whitespace() {
                    let (k, v) = kv
                        .split_once('=')
                        .ok_or_else(|| CeremonyError::Parse(format!("bad field `{kv}`")))?;
                    f.insert(k, v);
                }
                let get = |k: &str| {
                    f.get(k)
                        .copied()
                        .ok_or_else(|| CeremonyError::Parse(format!("missing `{k}`")))
                };
                let name = get("name")?.to_string();
                check_name(&name)?;
                contributions.push(Contribution {
                    name,
                    delta_g2: de(get("delta_g2")?)?,
                    pok_commitment: de(get("pok_r")?)?,
                    pok_response: de(get("pok_s")?)?,
                    prev_hash: hash_from(get("prev_hash")?)?,
                    params_hash: hash_from(get("params_hash")?)?,
                    new_delta_g1: de(get("new_delta_g1")?)?,
                    new_delta_g2: de(get("new_delta_g2")?)?,
                });
            } else if let Some((k, v)) = line.split_once('=') {
                match k {
                    "circuit" => circuit = Some(v.to_string()),
                    "initial_hash" => initial_hash = Some(hash_from(v)?),
                    "initial_delta_g1" => d1 = Some(de(v)?),
                    "initial_delta_g2" => d2 = Some(de(v)?),
                    "beta_g1" => b1 = Some(de(v)?),
                    "beta_g2" => b2 = Some(de(v)?),
                    "gamma_g2" => gm2 = Some(de(v)?),
                    other => return Err(CeremonyError::Parse(format!("unknown key `{other}`"))),
                }
            } else {
                return Err(CeremonyError::Parse(format!("unrecognized line `{line}`")));
            }
        }
        let missing = |w: &str| CeremonyError::Parse(format!("missing `{w}`"));
        Ok(Self {
            circuit: circuit.ok_or_else(|| missing("circuit"))?,
            initial_hash: initial_hash.ok_or_else(|| missing("initial_hash"))?,
            initial_delta_g1: d1.ok_or_else(|| missing("initial_delta_g1"))?,
            initial_delta_g2: d2.ok_or_else(|| missing("initial_delta_g2"))?,
            beta_g1: b1.ok_or_else(|| missing("beta_g1"))?,
            beta_g2: b2.ok_or_else(|| missing("beta_g2"))?,
            gamma_g2: gm2.ok_or_else(|| missing("gamma_g2"))?,
            contributions,
        })
    }

    /// Verify the `delta` chain using **only the transcript** (no key files):
    /// hash chain continuity, every proof of knowledge, and the pairing checks
    /// that each step multiplied `delta` by the contributed secret. This does
    /// not cover the query vectors; use [`verify_contribution`] with the key
    /// files for the full check.
    pub fn verify_delta_chain(&self) -> Result<(), CeremonyError> {
        let e = Bls12_381::pairing;
        let consistent =
            |d1: &G1Affine, d2: &G2Affine| e(*d1, self.beta_g2) == e(self.beta_g1, *d2);
        if !consistent(&self.initial_delta_g1, &self.initial_delta_g2) {
            return Err(CeremonyError::DeltaMismatch);
        }
        let mut prev_hash = self.initial_hash;
        let mut delta_g1 = self.initial_delta_g1;
        for (i, c) in self.contributions.iter().enumerate() {
            if c.prev_hash != prev_hash {
                return Err(CeremonyError::HashChain(format!(
                    "contribution {} does not extend contribution {}",
                    i + 1,
                    i
                )));
            }
            if c.delta_g2.is_zero() {
                return Err(CeremonyError::IdentityPoint("contribution public key"));
            }
            let ch = pok_challenge(&c.prev_hash, &c.name, &c.delta_g2, &c.pok_commitment);
            if self.gamma_g2 * c.pok_response
                != G2Projective::from(c.pok_commitment) + c.delta_g2 * ch
            {
                return Err(CeremonyError::BadProofOfKnowledge);
            }
            if !consistent(&c.new_delta_g1, &c.new_delta_g2) {
                return Err(CeremonyError::DeltaMismatch);
            }
            if e(delta_g1, c.delta_g2) != e(c.new_delta_g1, self.gamma_g2) {
                return Err(CeremonyError::DeltaNotUpdated);
            }
            prev_hash = c.params_hash;
            delta_g1 = c.new_delta_g1;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Verification-key export
// ---------------------------------------------------------------------------

fn rust_array(name: &str, bytes: &[u8]) -> String {
    let mut s = format!("let {name} = [\n    ");
    for (i, b) in bytes.iter().enumerate() {
        s.push_str(&format!("0x{b:02X}, "));
        if i % 16 == 15 {
            s.push_str("\n    ");
        }
    }
    s.push_str("];\n\n");
    s
}

fn uncompressed<T: CanonicalSerialize>(p: &T) -> Vec<u8> {
    let mut v = Vec::new();
    p.serialize_uncompressed(&mut v)
        .expect("serialization into a Vec cannot fail");
    v
}

/// The verification key as Rust byte arrays, in exactly the format the
/// `generate_vk` binary prints and `score-range-verifier` embeds
/// (arkworks uncompressed encoding: G1 = 96 bytes, G2 = 192 bytes).
pub fn vk_to_rust_arrays(vk: &VerifyingKey<Bls12_381>) -> String {
    let mut out = String::new();
    out.push_str(&rust_array("alpha_bytes", &uncompressed(&vk.alpha_g1)));
    out.push_str(&rust_array("beta_bytes", &uncompressed(&vk.beta_g2)));
    out.push_str(&rust_array("gamma_bytes", &uncompressed(&vk.gamma_g2)));
    out.push_str(&rust_array("delta_bytes", &uncompressed(&vk.delta_g2)));
    for (i, g) in vk.gamma_abc_g1.iter().enumerate() {
        out.push_str(&rust_array(&format!("gamma_abc_{i}"), &uncompressed(g)));
    }
    out
}

/// The verification key as JSON (hex of the same uncompressed encodings).
pub fn vk_to_json(circuit: &str, vk: &VerifyingKey<Bls12_381>) -> String {
    let abc: Vec<String> = vk
        .gamma_abc_g1
        .iter()
        .map(|g| format!("    \"{}\"", hex(&uncompressed(g))))
        .collect();
    format!(
        "{{\n  \"circuit\": \"{circuit}\",\n  \"curve\": \"BLS12-381\",\n  \"encoding\": \"arkworks uncompressed, hex\",\n  \"alpha_g1\": \"{}\",\n  \"beta_g2\": \"{}\",\n  \"gamma_g2\": \"{}\",\n  \"delta_g2\": \"{}\",\n  \"gamma_abc_g1\": [\n{}\n  ]\n}}\n",
        hex(&uncompressed(&vk.alpha_g1)),
        hex(&uncompressed(&vk.beta_g2)),
        hex(&uncompressed(&vk.gamma_g2)),
        hex(&uncompressed(&vk.delta_g2)),
        abc.join(",\n"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_ff::One;
    use ark_groth16::Groth16;
    use ark_r1cs_std::alloc::AllocVar;
    use ark_r1cs_std::eq::EqGadget;
    use ark_r1cs_std::fields::fp::FpVar;
    use ark_relations::r1cs::{ConstraintSynthesizer, ConstraintSystemRef, SynthesisError};
    use ark_snark::SNARK;
    use ark_std::rand::rngs::StdRng;
    use ark_std::rand::SeedableRng;

    /// `z = x * y * x` with `z` public: small, but has private intermediates so
    /// both `h_query` and `l_query` are non-empty.
    #[derive(Clone)]
    struct MulCircuit {
        x: Option<Fr>,
        y: Option<Fr>,
        z: Option<Fr>,
    }

    impl ConstraintSynthesizer<Fr> for MulCircuit {
        fn generate_constraints(self, cs: ConstraintSystemRef<Fr>) -> Result<(), SynthesisError> {
            let z = FpVar::new_input(cs.clone(), || {
                self.z.ok_or(SynthesisError::AssignmentMissing)
            })?;
            let x = FpVar::new_witness(cs.clone(), || {
                self.x.ok_or(SynthesisError::AssignmentMissing)
            })?;
            let y = FpVar::new_witness(cs, || self.y.ok_or(SynthesisError::AssignmentMissing))?;
            ((&x * &y) * &x).enforce_equal(&z)
        }
    }

    fn rng(seed: u8) -> StdRng {
        StdRng::from_seed([seed; 32])
    }

    fn tiny_initial() -> ProvingKey<Bls12_381> {
        let c = MulCircuit {
            x: None,
            y: None,
            z: None,
        };
        Groth16::<Bls12_381>::circuit_specific_setup(c, &mut rng(1))
            .unwrap()
            .0
    }

    /// Run `n` contributions + a beacon; returns every intermediate key and the transcript.
    fn run_ceremony(n: usize) -> (Vec<ProvingKey<Bls12_381>>, Transcript) {
        let initial = tiny_initial();
        let mut transcript = Transcript::new("threshold", &initial);
        let mut keys = vec![initial];
        for i in 0..n {
            let (next, c) = contribute(
                keys.last().unwrap(),
                &format!("participant-{}", i + 1),
                &mut rng(10 + i as u8),
            )
            .unwrap();
            transcript.contributions.push(c);
            keys.push(next);
        }
        let (next, c) =
            contribute_beacon(keys.last().unwrap(), "beacon", b"ledger-hash-0xabc").unwrap();
        transcript.contributions.push(c);
        keys.push(next);
        (keys, transcript)
    }

    #[test]
    fn multi_party_ceremony_verifies_and_yields_a_working_key() {
        let (keys, transcript) = run_ceremony(3);
        assert_eq!(transcript.contributions.len(), 4, "3 participants + beacon");

        // Every step verifies publicly.
        for (i, c) in transcript.contributions.iter().enumerate() {
            verify_contribution(&keys[i], &keys[i + 1], c, &mut rng(99)).unwrap();
        }
        // The transcript alone proves the delta chain.
        transcript.verify_delta_chain().unwrap();
        for k in &keys {
            verify_params_consistency(k).unwrap();
        }
        // delta changed at every step; nothing else did.
        for w in keys.windows(2) {
            assert_ne!(w[0].delta_g1, w[1].delta_g1);
            assert_eq!(w[0].vk.alpha_g1, w[1].vk.alpha_g1);
            assert_eq!(w[0].a_query, w[1].a_query);
        }
        assert_eq!(transcript.head_hash(), params_hash(keys.last().unwrap()));

        // Proof generation with the ceremony key verifies against the ceremony vk.
        let (x, y) = (Fr::from(3u32), Fr::from(5u32));
        let z = x * y * x;
        let final_pk = keys.last().unwrap();
        let proof = Groth16::<Bls12_381>::prove(
            final_pk,
            MulCircuit {
                x: Some(x),
                y: Some(y),
                z: Some(z),
            },
            &mut rng(7),
        )
        .unwrap();
        assert!(Groth16::<Bls12_381>::verify(&final_pk.vk, &[z], &proof).unwrap());
        // Wrong public input is rejected, and the *initial* vk (different delta) rejects it too.
        assert!(!Groth16::<Bls12_381>::verify(&final_pk.vk, &[z + Fr::one()], &proof).unwrap());
        assert!(!Groth16::<Bls12_381>::verify(&keys[0].vk, &[z], &proof).unwrap());
    }

    #[test]
    fn beacon_is_deterministic_and_input_dependent() {
        let pk = tiny_initial();
        let (a, ca) = contribute_beacon(&pk, "beacon", b"block-1").unwrap();
        let (b, cb) = contribute_beacon(&pk, "beacon", b"block-1").unwrap();
        let (c, _) = contribute_beacon(&pk, "beacon", b"block-2").unwrap();
        assert_eq!(ca, cb);
        assert_eq!(params_hash(&a), params_hash(&b));
        assert_ne!(params_hash(&a), params_hash(&c));
    }

    #[test]
    fn verification_rejects_tampered_queries_and_keys() {
        let (keys, transcript) = run_ceremony(1);
        let (old, new, c) = (&keys[0], &keys[1], &transcript.contributions[0]);
        let g1 = G1Affine::generator();
        let check = |bad: &ProvingKey<Bls12_381>, c: &Contribution| {
            verify_contribution(old, bad, c, &mut rng(5))
        };
        // Re-record the hash so the hash-chain check passes and the *content* check is exercised.
        let rehash = |bad: &ProvingKey<Bls12_381>| {
            let mut c2 = c.clone();
            c2.params_hash = params_hash(bad);
            c2.new_delta_g1 = bad.delta_g1;
            c2.new_delta_g2 = bad.vk.delta_g2;
            c2
        };
        assert!(check(new, c).is_ok());

        // h_query / l_query tampered.
        let mut bad = new.clone();
        bad.h_query[0] = (bad.h_query[0] + g1).into_affine();
        assert_eq!(
            check(&bad, &rehash(&bad)),
            Err(CeremonyError::QueryNotUpdated("h_query"))
        );
        let mut bad = new.clone();
        let last = bad.l_query.len() - 1;
        bad.l_query[last] = (bad.l_query[last] + g1).into_affine();
        assert_eq!(
            check(&bad, &rehash(&bad)),
            Err(CeremonyError::QueryNotUpdated("l_query"))
        );

        // Queries left unscaled (contributor "forgot" the d^-1 step).
        let mut bad = new.clone();
        bad.h_query = old.h_query.clone();
        assert_eq!(
            check(&bad, &rehash(&bad)),
            Err(CeremonyError::QueryNotUpdated("h_query"))
        );
        let mut bad = new.clone();
        bad.l_query = old.l_query.clone();
        assert_eq!(
            check(&bad, &rehash(&bad)),
            Err(CeremonyError::QueryNotUpdated("l_query"))
        );

        // Fields a contribution must not touch.
        let mut bad = new.clone();
        bad.vk.alpha_g1 = (bad.vk.alpha_g1 + g1).into_affine();
        assert_eq!(
            check(&bad, &rehash(&bad)),
            Err(CeremonyError::UnexpectedChange("vk.alpha_g1"))
        );
        let mut bad = new.clone();
        bad.a_query[0] = (bad.a_query[0] + g1).into_affine();
        assert_eq!(
            check(&bad, &rehash(&bad)),
            Err(CeremonyError::UnexpectedChange("a_query"))
        );
        let mut bad = new.clone();
        bad.vk.gamma_abc_g1[0] = (bad.vk.gamma_abc_g1[0] + g1).into_affine();
        assert_eq!(
            check(&bad, &rehash(&bad)),
            Err(CeremonyError::UnexpectedChange("vk.gamma_abc_g1"))
        );

        // Hash chain: tampered content without re-hashing is caught first.
        let mut bad = new.clone();
        bad.h_query[0] = (bad.h_query[0] + g1).into_affine();
        assert!(matches!(check(&bad, c), Err(CeremonyError::HashChain(_))));
    }

    #[test]
    fn verification_rejects_bad_delta_and_proof_of_knowledge() {
        let (keys, transcript) = run_ceremony(1);
        let (old, new, c) = (&keys[0], &keys[1], &transcript.contributions[0]);
        let rehash = |bad: &ProvingKey<Bls12_381>, c: &Contribution| {
            let mut c2 = c.clone();
            c2.params_hash = params_hash(bad);
            c2.new_delta_g1 = bad.delta_g1;
            c2.new_delta_g2 = bad.vk.delta_g2;
            c2
        };

        // delta_g1 and delta_g2 with different exponents.
        let mut bad = new.clone();
        bad.delta_g1 = (bad.delta_g1 + G1Affine::generator()).into_affine();
        assert_eq!(
            verify_contribution(old, &bad, &rehash(&bad, c), &mut rng(5)),
            Err(CeremonyError::DeltaMismatch)
        );

        // delta not moved at all (secret d != 1 is claimed, so the update check fails).
        let mut bad = new.clone();
        bad.delta_g1 = old.delta_g1;
        bad.vk.delta_g2 = old.vk.delta_g2;
        assert_eq!(
            verify_contribution(old, &bad, &rehash(&bad, c), &mut rng(5)),
            Err(CeremonyError::DeltaNotUpdated)
        );

        // Proof of knowledge tampered / for a different name / different prev hash.
        let mut bad_c = c.clone();
        bad_c.pok_response += Fr::one();
        assert_eq!(
            verify_contribution(old, new, &bad_c, &mut rng(5)),
            Err(CeremonyError::BadProofOfKnowledge)
        );
        let mut bad_c = c.clone();
        bad_c.name = "someone-else".into();
        assert_eq!(
            verify_contribution(old, new, &bad_c, &mut rng(5)),
            Err(CeremonyError::BadProofOfKnowledge)
        );
        // Contributor claims a public key they do not know the secret for.
        let mut bad_c = c.clone();
        bad_c.delta_g2 = (bad_c.delta_g2 + G2Affine::generator()).into_affine();
        assert!(verify_contribution(old, new, &bad_c, &mut rng(5)).is_err());

        // Replaying a record against the wrong predecessor.
        let (keys2, t2) = run_ceremony(2);
        assert!(matches!(
            verify_contribution(
                &keys2[1],
                &keys2[2],
                &transcript.contributions[0],
                &mut rng(5)
            ),
            Err(CeremonyError::HashChain(_))
        ));
        let _ = t2;

        // Identity public key.
        let mut bad_c = c.clone();
        bad_c.delta_g2 = G2Affine::zero();
        assert_eq!(
            verify_contribution(old, new, &bad_c, &mut rng(5)),
            Err(CeremonyError::IdentityPoint("contribution public key"))
        );
    }

    #[test]
    fn transcript_round_trips_and_rejects_tampering() {
        let (_, transcript) = run_ceremony(2);
        let text = transcript.to_text();
        let parsed = Transcript::from_text(&text).unwrap();
        assert_eq!(parsed, transcript);
        parsed.verify_delta_chain().unwrap();

        // Reordered contributions break the hash chain.
        let mut t = transcript.clone();
        t.contributions.swap(0, 1);
        assert!(matches!(
            t.verify_delta_chain(),
            Err(CeremonyError::HashChain(_))
        ));
        // Dropping a contribution breaks it too.
        let mut t = transcript.clone();
        t.contributions.remove(0);
        assert!(matches!(
            t.verify_delta_chain(),
            Err(CeremonyError::HashChain(_))
        ));
        // A forged delta in the record is caught.
        let mut t = transcript.clone();
        t.contributions[1].new_delta_g1 =
            (t.contributions[1].new_delta_g1 + G1Affine::generator()).into_affine();
        assert!(t.verify_delta_chain().is_err());

        // Malformed text.
        assert!(Transcript::from_text("garbage").is_err());
        assert!(Transcript::from_text("circuit=threshold\n").is_err());
        assert!(Transcript::from_text(&text.replace("pok_s=", "pok_s=zz")).is_err());
        assert!(Transcript::from_text(&format!("{text}\nbogus_key=1\n")).is_err());
    }

    #[test]
    fn names_are_validated() {
        let pk = tiny_initial();
        for bad in ["", "has space", "semi;colon", "naïve", &"x".repeat(65)] {
            assert_eq!(
                contribute(&pk, bad, &mut rng(1)).unwrap_err(),
                CeremonyError::InvalidName,
                "{bad:?}"
            );
        }
        assert!(contribute(&pk, "Alice_01.test-2", &mut rng(1)).is_ok());
    }

    #[test]
    fn params_round_trip_and_unknown_circuit_is_rejected() {
        let pk = tiny_initial();
        let bytes = params_to_bytes(&pk);
        let back = params_from_bytes(&bytes).unwrap();
        assert_eq!(params_hash(&pk), params_hash(&back));
        assert!(params_from_bytes(&bytes[..bytes.len() - 5]).is_err());
        assert!(matches!(
            initial_params("nope", &mut rng(1)),
            Err(CeremonyError::UnknownCircuit(_))
        ));
    }

    #[test]
    fn vk_export_formats() {
        let pk = tiny_initial();
        let txt = vk_to_rust_arrays(&pk.vk);
        for name in [
            "alpha_bytes",
            "beta_bytes",
            "gamma_bytes",
            "delta_bytes",
            "gamma_abc_0",
            "gamma_abc_1",
        ] {
            assert!(txt.contains(&format!("let {name} = [")), "{name}");
        }
        assert_eq!(
            txt.matches("let gamma_abc_").count(),
            pk.vk.gamma_abc_g1.len()
        );
        let json = vk_to_json("threshold", &pk.vk);
        assert!(json.contains("\"circuit\": \"threshold\"") && json.contains("\"gamma_abc_g1\""));
        assert_eq!(json.matches("\"0x").count(), 0);
    }

    /// The committed rehearsal transcript must stay internally valid and match
    /// the committed verification key export.
    #[test]
    fn committed_ceremony_artifacts_are_consistent() {
        let transcript = Transcript::from_text(include_str!("../ceremony/transcript.txt")).unwrap();
        assert_eq!(transcript.circuit, "threshold");
        assert_eq!(transcript.contributions.len(), 4, "3 participants + beacon");
        assert_eq!(transcript.contributions.last().unwrap().name, "beacon");
        transcript.verify_delta_chain().unwrap();
        // Round-trips byte for byte.
        assert_eq!(
            transcript.to_text().trim(),
            include_str!("../ceremony/transcript.txt").trim()
        );

        let vk_txt = include_str!("../vk_ceremony.txt");
        assert!(
            vk_txt.contains(&hex(&transcript.head_hash())),
            "vk_ceremony.txt must record the transcript's final params hash"
        );
        assert_eq!(vk_txt.matches("let gamma_abc_").count(), 9);
        let json = include_str!("../vk_ceremony.json");
        assert!(json.contains("\"circuit\": \"threshold\""));
        // The exported delta_g2 is the transcript's final delta_g2.
        let final_delta = hex(&uncompressed(
            &transcript.contributions.last().unwrap().new_delta_g2,
        ));
        assert!(json.contains(&final_delta));
    }
}
