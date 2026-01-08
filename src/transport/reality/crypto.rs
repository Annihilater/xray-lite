use anyhow::{anyhow, Result};
use rand::rngs::OsRng;
use ring::{aead, digest, hkdf, hmac};
use x25519_dalek::{PublicKey, StaticSecret};

/// 计算 Transcrip Hash (SHA256)
pub fn hash_transcript(messages: &[&[u8]]) -> Vec<u8> {
    let mut ctx = digest::Context::new(&digest::SHA256);
    for msg in messages {
        ctx.update(msg);
    }
    ctx.finish().as_ref().to_vec()
}

/// Reality 加密助手
pub struct RealityCrypto {
    my_secret: StaticSecret,
}

impl RealityCrypto {
    pub fn new() -> Self {
        let my_secret = StaticSecret::random_from_rng(OsRng);
        Self { my_secret }
    }

    pub fn get_public_key(&self) -> Vec<u8> {
        let pk = PublicKey::from(&self.my_secret);
        pk.as_bytes().to_vec()
    }

    pub fn derive_shared_secret(&self, peer_public_bytes: &[u8]) -> Result<Vec<u8>> {
        if peer_public_bytes.len() != 32 {
            return Err(anyhow!("无效的公钥长度"));
        }
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(peer_public_bytes);

        let peer_pk = PublicKey::from(bytes);
        let shared = self.my_secret.diffie_hellman(&peer_pk);
        Ok(shared.as_bytes().to_vec())
    }
}

pub struct TlsKeys {
    pub client_write_key: aead::LessSafeKey,
    pub server_write_key: aead::LessSafeKey,
    pub client_iv: [u8; 12],
    pub server_iv: [u8; 12],
}

impl TlsKeys {
    /// 基于 Shared Secret 导出握手密钥 (TLS 1.3 Key Schedule)
    /// 这里的实现简化了，假设 CipherSuite 为 TLS_AES_128_GCM_SHA256
    pub fn derive_handshake_keys(
        shared_secret: &[u8],
        hello_hash: &[u8],
    ) -> Result<(Self, hkdf::Prk)> {
        let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, &[]);
        let prk = salt.extract(shared_secret);

        // 这一步其实比较复杂，TLS 1.3 完整的 Key Schedule 涉及 Early Secret -> Handshake Secret -> Master Secret
        // 为了简化，我们假设这是一个标准的 1-RTT 握手

        // 1. Derive Handshake Secret
        // Early Secret is derived from 0 (no PSK)
        let early_secret = hkdf::Salt::new(hkdf::HKDF_SHA256, &[]).extract(&[0u8; 32]);
        let derived_secret = expand_label(&early_secret, b"derived", &hash_empty(), 32)?;

        let handshake_secret =
            hkdf::Salt::new(hkdf::HKDF_SHA256, &derived_secret).extract(shared_secret);

        // 2. Derive Traffic Secrets
        let client_hs_secret = expand_label(&handshake_secret, b"c hs traffic", hello_hash, 32)?;
        let server_hs_secret = expand_label(&handshake_secret, b"s hs traffic", hello_hash, 32)?;

        // 3. Derive Keys and IVs
        let client_keys = derive_key_iv(&client_hs_secret)?;
        let server_keys = derive_key_iv(&server_hs_secret)?;

        Ok((
            TlsKeys {
                client_write_key: client_keys.0,
                server_write_key: server_keys.0,
                client_iv: client_keys.1,
                server_iv: server_keys.1,
            },
            handshake_secret,
        ))
    }

    /// 导出应用数据密钥 (Master Secret)
    pub fn derive_application_keys(
        handshake_secret: &hkdf::Prk,
        handshake_hash: &[u8],
    ) -> Result<Self> {
        // 1. Derive Master Secret
        let derived_secret = expand_label(handshake_secret, b"derived", &hash_empty(), 32)?;
        let master_secret = hkdf::Salt::new(hkdf::HKDF_SHA256, &derived_secret).extract(&[0u8; 32]);

        // 2. Derive Traffic Secrets
        let client_app_secret = expand_label(&master_secret, b"c ap traffic", handshake_hash, 32)?;
        let server_app_secret = expand_label(&master_secret, b"s ap traffic", handshake_hash, 32)?;

        // 3. Derive Keys and IVs
        let client_keys = derive_key_iv(&client_app_secret)?;
        let server_keys = derive_key_iv(&server_app_secret)?;

        Ok(TlsKeys {
            client_write_key: client_keys.0,
            server_write_key: server_keys.0,
            client_iv: client_keys.1,
            server_iv: server_keys.1,
        })
    }
}

// Helper struct for ring HKDF
struct OutputLen(usize);

impl hkdf::KeyType for OutputLen {
    fn len(&self) -> usize {
        self.0
    }
}

    pub fn encrypt_server_record(&self, seq: u64, plaintext: &[u8], content_type: u8) -> Result<Vec<u8>> {
        let mut nonce_bytes = [0u8; 12];
        let mut padded_seq = [0u8; 12];
        padded_seq[4..].copy_from_slice(&seq.to_be_bytes());
        
        for i in 0..12 {
            nonce_bytes[i] = self.server_iv[i] ^ padded_seq[i];
        }
        
        let nonce = aead::Nonce::try_assume_unique_for_key(&nonce_bytes)
            .map_err(|_| anyhow!("Nonce creation failed"))?;
            
        let mut buffer = plaintext.to_vec();
        buffer.push(content_type); // Inner Content Type
        
        // TLS 1.3 Record Header for AAD
        // ContentType: Application Data (23)
        // Version: TLS 1.2 (0x0303)
        // Length: ciphertext length (plaintext + 1 + tag_len)
        let tag_len = self.server_write_key.algorithm().tag_len();
        let encrypted_len = buffer.len() + tag_len;
        
        let mut header = [23u8, 0x03, 0x03, 0, 0];
        let len_bytes = (encrypted_len as u16).to_be_bytes();
        header[3] = len_bytes[0];
        header[4] = len_bytes[1];
        
        let aad = aead::Aad::from(header);
        
        self.server_write_key.seal_in_place_append_tag(nonce, aad, &mut buffer)
            .map_err(|_| anyhow!("Encryption failed"))?;
            
        let mut record = Vec::with_capacity(5 + buffer.len());
        record.extend_from_slice(&header);
        record.extend_from_slice(&buffer);
        
        Ok(record)
    }

    pub fn calculate_verify_data(handshake_secret: &hkdf::Prk, handshake_hash: &[u8]) -> Result<Vec<u8>> {
        let finished_key = expand_label(handshake_secret, b"finished", &[], 32)?;
        let key = hmac::Key::new(hmac::HMAC_SHA256, &finished_key);
        let tag = hmac::sign(&key, handshake_hash);
        Ok(tag.as_ref().to_vec())
    }

    /// 解密客户端发送的 TLS 记录
    pub fn decrypt_client_record(&self, seq: u64, header: &[u8; 5], ciphertext: &mut [u8]) -> Result<(u8, usize)> {
        // 1. Build Nonce
        let mut nonce_bytes = [0u8; 12];
        let mut padded_seq = [0u8; 12];
        padded_seq[4..].copy_from_slice(&seq.to_be_bytes());
        for i in 0..12 {
            nonce_bytes[i] = self.client_iv[i] ^ padded_seq[i];
        }
        let nonce = aead::Nonce::try_assume_unique_for_key(&nonce_bytes)
            .map_err(|_| anyhow!("Nonce err"))?;

        // 2. AAD is the Record Header
        let aad = aead::Aad::from(&header[..]);

        // 3. Decrypt in place
        // ciphertext includes tag. transform to plaintext.
        // open_in_place returns plaintext length (excluding tag)
        let plaintext_len = self.client_write_key.open_in_place(nonce, aad, ciphertext)
            .map_err(|_| anyhow!("Decryption failed"))?
            .len();

        if plaintext_len == 0 {
            return Err(anyhow!("Empty plaintext"));
        }

        // 4. Remove padding and get content type (TLS 1.3 InnerPlaintext structure)
        // Structure: content[...], type(1byte), zeros[...]
        // We need to scan from end to find non-zero byte
        let mut scan_idx = plaintext_len - 1;
        while scan_idx > 0 && ciphertext[scan_idx] == 0 {
            scan_idx -= 1;
        }
        
        let content_type = ciphertext[scan_idx];
        let real_content_len = scan_idx; // Length excluding type byte and padding

        Ok((content_type, real_content_len))
    }
}


fn expand_label(prk: &hkdf::Prk, label: &[u8], context: &[u8], len: usize) -> Result<Vec<u8>> {
    let mut info = Vec::new();
    // HkdfLabel structure:
    // uint16 length
    info.extend_from_slice(&(len as u16).to_be_bytes());
    // opaque label<7..255> = "tls13 " + label
    let full_label = [b"tls13 ", label].concat();
    info.push(full_label.len() as u8);
    info.extend_from_slice(&full_label);
    // opaque context<0..255>
    info.push(context.len() as u8);
    info.extend_from_slice(context);

    let output_len = OutputLen(len);
    let okm = prk
        .expand(&[&info], output_len)
        .map_err(|_| anyhow!("HKDF expand failed"))?;

    let mut out = vec![0u8; len];
    okm.fill(&mut out)
        .map_err(|_| anyhow!("HKDF fill failed"))?;

    Ok(out)
}
// Ring's HKDF API is a bit specific. Let's fix expand_label implementation.

fn derive_key_iv(secret: &[u8]) -> Result<(aead::LessSafeKey, [u8; 12])> {
    let secret_prk = hkdf::Salt::new(hkdf::HKDF_SHA256, &[]).extract(secret);

    // key: expand("key", "", 16) for AES-128
    let key_bytes = expand_label(&secret_prk, b"key", &[], 16)?;
    let unbound_key = aead::UnboundKey::new(&aead::AES_128_GCM, &key_bytes)
        .map_err(|_| anyhow!("Failed to create unbound key"))?;
    let key = aead::LessSafeKey::new(unbound_key);

    // iv: expand("iv", "", 12)
    let iv_bytes = expand_label(&secret_prk, b"iv", &[], 12)?;
    let mut iv = [0u8; 12];
    iv.copy_from_slice(&iv_bytes);

    Ok((key, iv))
}

fn hash_empty() -> Vec<u8> {
    // SHA256 of empty string
    vec![
        0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f, 0xb9,
        0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b, 0x78, 0x52,
        0xb8, 0x55,
    ]
}
