use soroban_sdk::{Env, Vec, BytesN};

pub fn test(env: &Env) {
    let mut v: Vec<u32> = Vec::new(env);
    v.push_back(1);
}
