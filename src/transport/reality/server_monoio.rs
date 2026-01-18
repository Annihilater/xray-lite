// transport/reality/server_monoio.rs
// 原生 monoio Reality 服务器实现 - 修复 AuthKey 派生 Bug

use std::sync::Arc;
use monoio::io::{AsyncReadRent, AsyncWriteRent, AsyncReadRentExt};
use monoio_rustls_reality::server::TlsAcceptor as MonoioTlsAcceptor;
use rustls::ServerConfig;
use rustls::reality::RealityConfig;
use anyhow::{Result, anyhow, bail};
use tracing::{info, warn, debug, error};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};
use hkdf::Hkdf;
use sha2::Sha256;
use aes_gcm::{Aes256Gcm, KeyInit, AeadInPlace, Nonce};
use ring::hmac;

use super::hello_parser;

pub struct RealityServerMonoio {
    private_key: Vec<u8>,
    dest: String,
    short_ids: Vec<Vec<u8>>,
    server_names: Vec<String>,
}

impl Clone for RealityServerMonoio {
    fn clone(&self) -> Self {
        Self {
            private_key: self.private_key.clone(),
            dest: self.dest.clone(),
            short_ids: self.short_ids.clone(),
            server_names: self.server_names.clone(),
        }
    }
}

impl RealityServerMonoio {
    pub fn new(private_key: Vec<u8>, dest: Option<String>, short_ids: Vec<String>, server_names: Vec<String>) -> Result<Self> {
        let mut short_ids_bytes = Vec::new();
        for id in short_ids {
            let b = hex::decode(&id).map_err(|e| anyhow!("Invalid shortId hex: {}", e))?;
            short_ids_bytes.push(b);
        }

        Ok(Self { 
            private_key,
            dest: dest.unwrap_or_else(|| "www.microsoft.com:443".to_string()),
            short_ids: short_ids_bytes,
            server_names,
        })
    }

    pub async fn accept<IO>(&self, mut stream: IO) -> Result<monoio_rustls_reality::server::TlsStream<PrefixedMonoioStream<IO>>> 
    where IO: AsyncReadRent + AsyncWriteRent + 'static {
        let mut buffer = Vec::with_capacity(2048);
        
        while buffer.len() < 5 {
            let mut chunk = vec![0u8; 1024];
            let (res, c) = stream.read(chunk).await;
            chunk = c;
            let n = res?;
            if n == 0 { bail!("Connection closed early"); }
            buffer.extend_from_slice(&chunk[..n]);
        }

        if buffer[0] != 0x16 { bail!("Not TLS"); }

        let needed = 5 + u16::from_be_bytes([buffer[3], buffer[4]]) as usize;
        while buffer.len() < needed && buffer.len() < 16384 {
            let mut chunk = vec![0u8; 1024];
            let (res, c) = stream.read(chunk).await;
            chunk = c;
            let n = res?;
            if n == 0 { break; }
            buffer.extend_from_slice(&chunk[..n]);
        }

        if let Ok(Some(info)) = hello_parser::parse_client_hello(&buffer) {
            let sni_valid = if self.server_names.is_empty() { true }
                           else if let Some(sni) = &info.server_name { self.server_names.iter().any(|s| s == sni) }
                           else { false };

            if sni_valid {
                // 1. 验证客户端并获取正确的 auth_key
                if let Some((offset, auth_key)) = self.verify_client_reality(&info, &buffer) {
                    let dest_host = self.dest.split(':').next().unwrap_or("www.microsoft.com");
                    info!("Reality [io_uring]: Auth Success (Offset: {})", offset);
                    
                    // 2. 使用这个 auth_key 生成动态证书
                    let (cert, key) = self.generate_reality_cert(&auth_key, dest_host)?;

                    let mut config = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                        .with_safe_default_protocol_versions()?
                        .with_no_client_auth()
                        .with_single_cert(vec![cert], key)
                        .map_err(|e| anyhow!("Config build fail: {}", e))?;

                    // 3. 将验证通过的 auth_key 作为后续 TLS 处理的基础密钥
                    let mut rc = RealityConfig::new(auth_key.to_vec());
                    rc.verify_client = false; // 已通过手动验证，后续握手不再验证
                    config.reality_config = Some(Arc::new(rc));

                    let acceptor = MonoioTlsAcceptor::from(Arc::new(config));
                    let prefixed_stream = PrefixedMonoioStream::new(stream, buffer);
                    
                    return acceptor.accept(prefixed_stream).await
                        .map_err(|e| anyhow!("TLS Handshake Error: {:?}", e));
                }
            }
        }

        bail!("Reality Authentication Failed")
    }

    fn verify_client_reality(&self, info: &super::hello_parser::ClientHelloInfo, full_hello: &[u8]) -> Option<(usize, [u8; 32])> {
        if info.session_id.len() != 32 || info.public_key.is_none() { return None; }
        
        let mut server_priv = [0u8; 32];
        server_priv.copy_from_slice(&self.private_key);
        let client_pub: [u8; 32] = info.public_key.as_ref()?.as_slice().try_into().ok()?;
        
        let shared = StaticSecret::from(server_priv).diffie_hellman(&X25519PublicKey::from(client_pub));
        
        let hk = Hkdf::<Sha256>::new(Some(&info.client_random[0..20]), shared.as_bytes());
        let mut auth_key = [0u8; 32];
        // 关键修复：这里的 info string 必须是 "REALITY"，且全局必须统一
        if hk.expand(b"REALITY", &mut auth_key).is_err() { return None; }

        let cipher = Aes256Gcm::new(aes_gcm::Key::<Aes256Gcm>::from_slice(&auth_key));
        let nonce = Nonce::from_slice(&info.client_random[20..32]);

        let handshake_msg = if full_hello[0] == 0x16 { &full_hello[5..] } else { full_hello };
        let mut aad = handshake_msg.to_vec();
        
        if let Some(pos) = hex::encode(&aad).find(&hex::encode(&info.session_id)).map(|p| p/2) {
            for i in 0..32 { if pos + i < aad.len() { aad[pos + i] = 0; } }
        }

        let mut buf = info.session_id.clone();
        if cipher.decrypt_in_place(nonce, &aad, &mut buf).is_err() { return None; }
        if buf.len() < 16 { return None; }

        for sid in &self.short_ids {
            if sid == &buf[4..12] { return Some((4, auth_key)); }
            if sid == &buf[8..16] { return Some((8, auth_key)); }
        }
        None
    }

    fn generate_reality_cert(&self, auth_key: &[u8; 32], host: &str) -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>)> {
        use rcgen::{CertificateParams, KeyPair, PKCS_ED25519};
        let key_pair = KeyPair::generate(&PKCS_ED25519).map_err(|e| anyhow!("KeyPair gen failed: {}", e))?;
        let pub_key_raw = key_pair.public_key_raw().to_vec();
        let mut params = CertificateParams::new(vec![host.to_string()]);
        params.alg = &PKCS_ED25519;
        params.key_pair = Some(key_pair);
        let cert = rcgen::Certificate::from_params(params).map_err(|e| anyhow!("Cert gen failed: {}", e))?;
        let mut cert_der = cert.serialize_der().map_err(|e| anyhow!("Cert serialize failed: {}", e))?;
        let priv_key_der = cert.serialize_private_key_der();
        
        let sig_pos = cert_der.len() - 64;
        let ring_key = hmac::Key::new(hmac::HMAC_SHA512, auth_key);
        let signature = hmac::sign(&ring_key, &pub_key_raw);
        cert_der[sig_pos..].copy_from_slice(signature.as_ref());
        
        Ok((CertificateDer::from(cert_der), PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(priv_key_der))))
    }
}

pub struct PrefixedMonoioStream<IO> {
    inner: IO,
    prefix: Vec<u8>,
    prefix_offset: usize,
}

impl<IO> PrefixedMonoioStream<IO> {
    pub fn new(inner: IO, prefix: Vec<u8>) -> Self {
        Self { inner, prefix, prefix_offset: 0 }
    }
}

impl<IO: AsyncReadRent> AsyncReadRent for PrefixedMonoioStream<IO> {
    async fn read<T: monoio::buf::IoBufMut>(&mut self, mut buf: T) -> monoio::BufResult<usize, T> {
        if self.prefix_offset < self.prefix.len() {
            let remaining = self.prefix.len() - self.prefix_offset;
            let to_copy = remaining.min(buf.bytes_total());
            unsafe {
                let slice = std::slice::from_raw_parts_mut(buf.write_ptr(), to_copy);
                slice.copy_from_slice(&self.prefix[self.prefix_offset..self.prefix_offset + to_copy]);
                buf.set_init(to_copy);
            }
            self.prefix_offset += to_copy;
            return (Ok(to_copy), buf);
        }
        self.inner.read(buf).await
    }
    async fn readv<T: monoio::buf::IoVecBufMut>(&mut self, buf: T) -> monoio::BufResult<usize, T> { self.inner.readv(buf).await }
}

impl<IO: AsyncWriteRent> AsyncWriteRent for PrefixedMonoioStream<IO> {
    async fn write<T: monoio::buf::IoBuf>(&mut self, buf: T) -> monoio::BufResult<usize, T> { self.inner.write(buf).await }
    async fn writev<T: monoio::buf::IoVecBuf>(&mut self, buf_vec: T) -> monoio::BufResult<usize, T> { self.inner.writev(buf_vec).await }
    async fn flush(&mut self) -> std::io::Result<()> { self.inner.flush().await }
    async fn shutdown(&mut self) -> std::io::Result<()> { self.inner.shutdown().await }
}

unsafe impl<IO: monoio::io::Split> monoio::io::Split for PrefixedMonoioStream<IO> {}
