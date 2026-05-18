use anyhow::Result;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Persistent device identity using Ed25519.
#[derive(Clone)]
pub struct DeviceIdentity {
    signing_key: SigningKey,
}

/// Public identity that can be shared with peers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicIdentity {
    pub device_id: String,
    pub public_key: Vec<u8>,
}

impl DeviceIdentity {
    /// Generate a new random identity.
    pub fn generate() -> Self {
        let signing_key = SigningKey::generate(&mut OsRng);
        Self { signing_key }
    }

    /// Load identity from a file, or generate and save a new one.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            let bytes = std::fs::read(path)?;
            if bytes.len() == 32 {
                let signing_key = SigningKey::from_bytes(
                    bytes
                        .as_slice()
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("invalid key length"))?,
                );
                return Ok(Self { signing_key });
            }
        }

        let identity = Self::generate();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, identity.signing_key.as_bytes())?;
        Ok(identity)
    }

    /// Get the public identity to share with peers.
    pub fn public(&self) -> PublicIdentity {
        PublicIdentity {
            device_id: hex::encode(self.signing_key.verifying_key().as_bytes()),
            public_key: self.signing_key.verifying_key().as_bytes().to_vec(),
        }
    }

    /// Sign a message.
    pub fn sign(&self, message: &[u8]) -> Signature {
        self.signing_key.sign(message)
    }

    /// Verify a signature against a public key.
    pub fn verify(public_key: &[u8], message: &[u8], signature: &Signature) -> Result<()> {
        let verifying_key = VerifyingKey::from_bytes(
            public_key
                .try_into()
                .map_err(|_| anyhow::anyhow!("invalid public key length"))?,
        )?;
        verifying_key.verify(message, signature)?;
        Ok(())
    }

    /// Get the raw signing key bytes (for serialization).
    pub fn key_bytes(&self) -> [u8; 32] {
        self.signing_key.to_bytes()
    }
}

/// Simple hex encoding (avoiding extra dependency).
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn generate_produces_valid_identity() {
        let id = DeviceIdentity::generate();
        let public = id.public();
        assert_eq!(
            public.device_id.len(),
            64,
            "device_id should be 32-byte hex"
        );
        assert_eq!(public.public_key.len(), 32, "public_key should be 32 bytes");
    }

    #[test]
    fn generate_produces_different_identities() {
        let a = DeviceIdentity::generate();
        let b = DeviceIdentity::generate();
        assert_ne!(a.public().device_id, b.public().device_id);
        assert_ne!(a.public().public_key, b.public().public_key);
    }

    #[test]
    fn sign_and_verify_round_trip() {
        let id = DeviceIdentity::generate();
        let message = b"lanbridge test message";
        let signature = id.sign(message);

        DeviceIdentity::verify(&id.public().public_key, message, &signature)
            .expect("verification should succeed for matching key");
    }

    #[test]
    fn verify_fails_with_wrong_key() {
        let id1 = DeviceIdentity::generate();
        let id2 = DeviceIdentity::generate();
        let message = b"lanbridge test message";
        let signature = id1.sign(message);

        let result = DeviceIdentity::verify(&id2.public().public_key, message, &signature);
        assert!(result.is_err(), "verification should fail with wrong key");
    }

    #[test]
    fn verify_fails_with_tampered_message() {
        let id = DeviceIdentity::generate();
        let message = b"original message";
        let signature = id.sign(message);

        let result =
            DeviceIdentity::verify(&id.public().public_key, b"tampered message", &signature);
        assert!(
            result.is_err(),
            "verification should fail with tampered message"
        );
    }

    #[test]
    fn load_or_create_creates_key_when_missing() {
        let dir = TempDir::new().unwrap();
        let key_path = dir.path().join("identity.key");

        let id = DeviceIdentity::load_or_create(&key_path).unwrap();
        let public = id.public();

        assert!(key_path.exists(), "key file should be created");
        let stored = std::fs::read(&key_path).unwrap();
        assert_eq!(stored.len(), 32, "stored key should be 32 bytes");
        assert_eq!(stored, id.key_bytes());
    }

    #[test]
    fn load_or_create_loads_existing_key() {
        let dir = TempDir::new().unwrap();
        let key_path = dir.path().join("identity.key");

        // Create first identity
        let id1 = DeviceIdentity::load_or_create(&key_path).unwrap();
        let public1 = id1.public();

        // Load same key file again
        let id2 = DeviceIdentity::load_or_create(&key_path).unwrap();
        let public2 = id2.public();

        assert_eq!(public1.device_id, public2.device_id);
        assert_eq!(public1.public_key, public2.public_key);
    }

    #[test]
    fn load_or_create_creates_parent_dir() {
        let dir = TempDir::new().unwrap();
        let key_path = dir.path().join("sub").join("nested").join("identity.key");

        let id = DeviceIdentity::load_or_create(&key_path).unwrap();
        assert!(key_path.exists());
        let _ = id; // keep identity alive for the assertion
    }
}
