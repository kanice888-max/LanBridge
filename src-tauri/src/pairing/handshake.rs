use sha2::{Digest, Sha256};

/// Derive a 6-digit pairing verification code from two public keys and a nonce.
///
/// Both devices will compute the same code because:
/// 1. Public keys are sorted lexicographically (min/max)
/// 2. The same nonce is used
/// 3. SHA256 is deterministic
///
/// Code format: 6-digit zero-padded string (e.g., "004521").
pub fn derive_pairing_code(public_key_a: &[u8], public_key_b: &[u8], nonce: &[u8]) -> String {
    // Sort public keys lexicographically
    let (min_key, max_key) = if public_key_a <= public_key_b {
        (public_key_a, public_key_b)
    } else {
        (public_key_b, public_key_a)
    };

    // SHA256("lanbridge-pairing-v1" || nonce || min_public_key || max_public_key)
    let mut hasher = Sha256::new();
    hasher.update(b"lanbridge-pairing-v1");
    hasher.update(nonce);
    hasher.update(min_key);
    hasher.update(max_key);
    let hash = hasher.finalize();

    // Take first 4 bytes as u32, modulo 1_000_000
    let value = u32::from_be_bytes([hash[0], hash[1], hash[2], hash[3]]);
    let code = value % 1_000_000;

    format!("{:06}", code)
}

/// Generate a random session nonce for pairing.
pub fn generate_nonce() -> Vec<u8> {
    let mut nonce = vec![0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut nonce);
    nonce
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pairing_code_deterministic() {
        let key_a = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let key_b = vec![16, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1];
        let nonce = vec![0; 32];

        let code1 = derive_pairing_code(&key_a, &key_b, &nonce);
        let code2 = derive_pairing_code(&key_b, &key_a, &nonce);

        assert_eq!(code1, code2, "order of keys should not matter");
        assert_eq!(code1.len(), 6, "code should be 6 digits");
    }

    #[test]
    fn test_pairing_code_different_nonce() {
        let key_a = vec![1; 32];
        let key_b = vec![2; 32];
        let nonce1 = vec![0; 32];
        let nonce2 = vec![1; 32];

        let code1 = derive_pairing_code(&key_a, &key_b, &nonce1);
        let code2 = derive_pairing_code(&key_a, &key_b, &nonce2);

        assert_ne!(code1, code2, "different nonce should give different code");
    }

    #[test]
    fn test_generate_nonce_unique() {
        let nonce1 = generate_nonce();
        let nonce2 = generate_nonce();
        assert_eq!(nonce1.len(), 32);
        assert_ne!(nonce1, nonce2, "nonces should be unique");
    }
}
