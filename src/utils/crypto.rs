use base64::{engine::general_purpose, Engine as _};
use rand::rngs::OsRng;
use x25519_dalek::{EphemeralSecret, PublicKey};

/// X25519 密钥对
pub struct X25519KeyPair {
    pub private_key: EphemeralSecret,
    pub public_key: PublicKey,
}

#[cfg(test)]
mod tests {
    use super::*;
}
