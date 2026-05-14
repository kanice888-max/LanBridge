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
