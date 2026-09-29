
use ark_serialize::CanonicalSerialize;
use ark_std::rand::SeedableRng;
use ark_std::rand::rngs::StdRng;
use zk_circuit::generate_test_keys;

fn print_array(name: &str, bytes: &[u8]) {
    print!("let {} = [\n    ", name);
    for (i, b) in bytes.iter().enumerate() {
        print!("0x{:02X}, ", b);
        if i % 16 == 15 {
            print!("\n    ");
        }
    }
    println!("];\n");
}

fn main() {
    let mut rng = StdRng::from_seed([0u8; 32]);
    let (_, vk) = generate_test_keys(&mut rng).unwrap();
    
    let mut alpha_bytes = vec![0u8; 96];
    vk.alpha_g1.serialize_uncompressed(&mut alpha_bytes[..]).unwrap();
    print_array("alpha_bytes", &alpha_bytes);
    
    let mut beta_bytes = vec![0u8; 192];
    vk.beta_g2.serialize_uncompressed(&mut beta_bytes[..]).unwrap();
    print_array("beta_bytes", &beta_bytes);
    
    let mut gamma_bytes = vec![0u8; 192];
    vk.gamma_g2.serialize_uncompressed(&mut gamma_bytes[..]).unwrap();
    print_array("gamma_bytes", &gamma_bytes);
    
    let mut delta_bytes = vec![0u8; 192];
    vk.delta_g2.serialize_uncompressed(&mut delta_bytes[..]).unwrap();
    print_array("delta_bytes", &delta_bytes);
    
    for (i, g) in vk.gamma_abc_g1.iter().enumerate() {
        let mut g_bytes = vec![0u8; 96];
        g.serialize_uncompressed(&mut g_bytes[..]).unwrap();
        print_array(&format!("gamma_abc_{}", i), &g_bytes);
    }
}
