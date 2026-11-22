# Groth16 trusted setup: Phase 2 ceremony tooling

The score circuit uses Groth16, which needs a circuit-specific proving/verifying
key derived from secret randomness ("toxic waste"). Anyone who learns that
randomness can forge proofs that the on-chain verifier accepts. The key in the
`score-range-verifier` contract today comes from `generate_vk`, whose seed is
public, so **it is a development key and must not be used in production**.

This document describes the tooling in `zk/circuit` (`src/ceremony.rs` and the
`ceremony` binary) for producing a key through a multi-party contribution
process, what it guarantees, and, just as important, what it does not.

## What is implemented, and what is not

A Groth16 ceremony has two phases:

| Phase | Produces | Circuit specific | In this repo |
|-------|----------|------------------|--------------|
| 1: powers of tau | `tau`, `alpha`, `beta` powers | no | **not implemented**: `ceremony init` is a single-party stand-in |
| 2: circuit setup | randomizes `delta` for one circuit | yes | **implemented**: multi-party contributions, public verification, random beacon |

**Phase 2 alone does not make a key trustworthy.** Whoever knows the Phase 1
secrets (`tau`, `alpha`, `beta`) can forge proofs *no matter what `delta` is*:
knowing `tau` lets them pick the quotient polynomial's evaluation to satisfy the
verification equation for any statement. The soundness of a final key therefore
needs **both**:

1. a Phase 1 where at least one participant was honest, and
2. a Phase 2 where at least one participant was honest.

`ceremony init` generates fresh parameters in one process, so **the operator
running it knows all the Phase 1 secrets**. Keys derived from `init` are
suitable for rehearsals, integration tests and tooling development only. A
production ceremony must replace `init` with parameters derived from the output
of a public powers-of-tau ceremony (importing a Phase 1 transcript is future
work and is not part of this tooling).

### The committed artifacts are a rehearsal

`zk/circuit/vk_ceremony.txt`, `vk_ceremony.json` and `ceremony/transcript.txt`
(threshold circuit) come from a **single-operator rehearsal**: one machine ran `init`, three
contributions named `participant-1..3`, and a beacon whose value
(`rehearsal-beacon-not-a-real-ledger-hash`) is a placeholder, not a real
public-randomness source. They were generated against the circuit as of the commit that added them. A
verification key is only valid for the exact circuit it was set up for, so
**these files must be regenerated whenever `zk/circuit` changes** (the same is
true of the contract's embedded key). They demonstrate that the pipeline works end to end
and give integrators a key to test against. They are **not** the product of an
independent multi-party ceremony and must not be treated as production keys.

## How a Phase 2 contribution works

A contribution draws a secret `d`, then updates the proving key:

```
delta_g1'  = d * delta_g1            vk.delta_g2' = d * vk.delta_g2
h_query'   = d^-1 * h_query          l_query'     = d^-1 * l_query
```

and publishes `d * gamma_g2` together with a Schnorr proof that it knows `d`.
Nothing else in the key changes. The key stays valid for the same circuit (a
real proof under the final key verifies against the final verification key), and
because each participant multiplies `delta` by their own secret, the final
`delta` is unknown to anyone unless **every** participant colluded or leaked.

(arkworks' Groth16 setup uses random G1/G2 bases rather than the curve
generators, so every check is expressed relative to elements already in the key,
for example `e(delta_g1, beta_g2) == e(beta_g1, delta_g2)`.)

### Random beacon

The last contribution should be a *beacon*: its secret is derived
deterministically from a public value that did not exist when the last human
contributed (for example a future Stellar ledger hash), so the final participant
could not steer the result. `ceremony beacon <dir> <name> <beacon>` hashes the
beacon string to the secret; anyone can recompute and audit it.

## Running a ceremony

```bash
cd zk/circuit

# Coordinator: create round 0 and the transcript (rehearsal only, see above)
cargo run --release --bin ceremony -- init threshold ceremony-run

# Each participant, on their own machine, in turn: take the latest
# params_NNNN.bin + transcript.txt, then
cargo run --release --bin ceremony -- contribute ceremony-run alice
# ...publish the new params_NNNN+1.bin and transcript.txt, then destroy the machine's state.

# After the last human participant: the public beacon
cargo run --release --bin ceremony -- beacon ceremony-run beacon "<ledger-hash>"

# Anyone: verify every step from the key files
cargo run --release --bin ceremony -- verify ceremony-run

# Export the verification key (refuses to run if verification fails or there are no contributions)
cargo run --release --bin ceremony -- export ceremony-run vk_ceremony
#   -> vk_ceremony.txt   Rust byte arrays, the format `generate_vk` prints
#   -> vk_ceremony.json  hex of the same encodings
```

Participant names use `[A-Za-z0-9_.-]` (1 to 64 characters). Every `contribute`
and `beacon` run self-verifies the new step before writing it. Each
`params_NNNN.bin` is about 10 MB, so the key files are published out of band;
only `transcript.txt` and the verification key are committed here.

### Participant guidance

* Run on a machine you trust, ideally offline and freshly booted; `contribute`
  uses the operating system's randomness.
* The secret exists only in process memory for the length of the run. Close the
  machine afterwards.
* Check that the published transcript line for your contribution carries the
  hash `contribute` printed.

## Verification

* **`ceremony verify <dir>`** is the full check. For every step it verifies the
  hash chain, the proof of knowledge, the `delta` update (pairing checks), that
  `h_query` and `l_query` were scaled by `d^-1` (a batched random-linear-combination
  pairing check over every entry), and that nothing else in the key changed.
* **`Transcript::verify_delta_chain`** checks the `delta` chain, every proof of
  knowledge and the hash chain from `transcript.txt` alone (no key files). It
  does *not* cover the query vectors, so it is a partial check for people who
  only have the transcript.
* `cargo test -p zk-circuit` runs a multi-party ceremony on a small circuit and
  rejects tampered queries, an unscaled `h_query`/`l_query`, a changed
  `alpha`/`a_query`/`gamma_abc`, a forged proof of knowledge, a replayed
  contribution and a broken hash chain. `cargo test --release -p zk-circuit`
  additionally runs a ceremony on the real score circuit and checks that a proof
  generated with the ceremony key verifies against the ceremony verification key
  (and against neither a wrong statement nor the pre-ceremony key).

## Using the output key

1. Replace the arrays in `embedded_verification_key` in
   `contracts/score-range-verifier/src/lib.rs` with the contents of the
   exported `vk_ceremony.txt` (same encoding and names).
2. Regenerate the proof fixtures that depend on the key (the contract's
   end-to-end test builds its proof from `generate_test_keys`, so it must be
   switched to use the ceremony proving key).
3. The proving key (`params_NNNN.bin` of the final round) is what provers need;
   it is not secret and should be published with the transcript.

## Before a production deployment

- [ ] Replace `init` with parameters derived from a public Phase 1 transcript.
- [ ] Recruit independent participants (different organizations, machines and
      networks) and record each contribution's hash in more than one place.
- [ ] Use a real public beacon fixed in advance.
- [ ] Have at least one independent party run `ceremony verify` on the published
      files.
- [ ] Rotate the embedded verification key and bump the circuit version.

## Circuits

`threshold` (the `score > threshold` circuit) and `tier` (the tier-band circuit)
are supported: `ceremony init threshold <dir>` / `ceremony init tier <dir>`.
Each circuit has its own key, so each needs its own ceremony and its own
embedded verification key in `score-range-verifier`. Adding another circuit is
one more arm in `ceremony::initial_params` and an entry in `ceremony::CIRCUITS`.
The committed rehearsal artifacts cover `threshold` only.
