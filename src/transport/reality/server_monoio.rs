// transport/reality/server_monoio.rs
// 原生 monoio Reality 服务器实现 - 不使用 compat 层

use std::sync::Arc;
use monoio::io::{AsyncReadRent, AsyncWriteRent};
use monoio_rustls_reality::server::TlsAcceptor as MonoioTlsAcceptor;
use rustls::ServerConfig;
use rustls::reality::RealityConfig;
use anyhow::{Result, anyhow, bail};
use tracing::{info, warn, debug};
use rustls_pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};
use hkdf::Hkdf;
use sha2::Sha256;
use ring::hmac;

use super::hello_parser::{self, ClientHelloInfo};

pub struct RealityServerMonoio {
    reality_config: Arc<RealityConfig>,
    server_names: Vec<String>,
}

impl Clone for RealityServerMonoio {
    fn clone(&self) -> Self {
        Self {
            reality_config: Arc::clone(&self.reality_config),
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

        let reality_config = RealityConfig::new(private_key)
            .with_verify_client(true)
            .with_short_ids(short_ids_bytes)
            .with_dest(dest.unwrap_or_else(|| "www.microsoft.com:443".to_string()));

        reality_config.validate().map_err(|e| anyhow!("Reality config validation failed: {:?}", e))?;

        Ok(Self { 
            reality_config: Arc::new(reality_config),
            server_names,
        })
    }

    /// 接受连接并执行 TLS 握手 - 原生 monoio 版本
    pub async fn accept<IO>(&self, mut stream: IO) -> Result<monoio_rustls_reality::server::TlsStream<PrefixedMonoioStream<IO>>> 
    where IO: AsyncReadRent + AsyncWriteRent + 'static {
        // 读取 ClientHello
        let mut buffer = vec![0u8; 2048];
        let (res, buf) = stream.read(buffer).await;
        let n = res?;
        buffer = buf;
        buffer.truncate(n);

        if buffer.len() < 5 {
            bail!("Connection closed early");
        }

        let needed = if buffer[0] == 0x16 { 5 + u16::from_be_bytes([buffer[3], buffer[4]]) as usize } else { buffer.len() };
        
        // 如果需要更多数据，继续读取
        while buffer.len() < needed && buffer.len() < 16384 {
            let mut chunk = vec![0u8; 1024];
            let (res, c) = stream.read(chunk).await;
            chunk = c;
            match res {
                Ok(0) => break,
                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                Err(e) => return Err(anyhow!("Read error: {}", e)),
            }
        }

        if let Ok(Some(info)) = hello_parser::parse_client_hello(&buffer) {
            // SNI 验证
            let sni_valid = if self.server_names.is_empty() {
                true
            } else if let Some(sni) = &info.server_name {
                self.server_names.iter().any(|s| s == sni)
            } else {
                false
            };

            if sni_valid {
                if let Some((offset, auth_key)) = self.verify_client_reality(&info, &buffer) {
                    let dest_str = self.reality_config.dest.as_deref().unwrap_or("www.microsoft.com");
                    let dest_host = dest_str.split(':').next().unwrap_or("www.microsoft.com");

                    info!("Reality [monoio]: Verified client (Offset {})", offset);
                    
                    let (cert, key) = self.generate_reality_cert(&auth_key, dest_host)?;

                    let mut conn_reality_config = (*self.reality_config).clone();
                    conn_reality_config.private_key = auth_key.to_vec();
                    conn_reality_config.verify_client = false;

                    let mut config = ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                        .with_safe_default_protocol_versions()?
                        .with_no_client_auth()
                        .with_single_cert(vec![cert], key)
                        .map_err(|e| anyhow!("Config build fail: {}", e))?;
                    config.reality_config = Some(Arc::new(conn_reality_config));

                    let acceptor = MonoioTlsAcceptor::from(Arc::new(config));
                    let prefixed_stream = PrefixedMonoioStream::new(stream, buffer);
                    
                    return acceptor.accept(prefixed_stream).await
                        .map_err(|e| anyhow!("TLS handshake failed: {:?}", e));
                }
            }
        }

        // Fallback 到正常 TLS 或错误处理
        bail!("Reality verification failed, fallback needed")
    }

    fn verify_client_reality(&self, info: &ClientHelloInfo, buffer: &[u8]) -> Option<(usize, [u8; 32])> {
        let session_id = &info.session_id;
        if session_id.len() != 32 { return None; }

        let private_key_bytes: [u8; 32] = self.reality_config.private_key.clone().try_into().ok()?;
        let server_private = StaticSecret::from(private_key_bytes);
        let client_public_bytes: [u8; 32] = session_id[..32].try_into().ok()?;
        let client_public = X25519PublicKey::from(client_public_bytes);
        let shared_secret = server_private.diffie_hellman(&client_public);

        let hk = Hkdf::<Sha256>::new(None, shared_secret.as_bytes());
        let mut auth_key = [0u8; 32];
        hk.expand(b"REALITY Auth key", &mut auth_key).ok()?;

        for short_id in &self.reality_config.short_ids {
            let short_id_slice = if short_id.len() >= 8 { &short_id[..8] } else { short_id.as_slice() };
            
            for offset in [0, 4, 8, 12, 16] {
                if self.verify_at_offset(buffer, &auth_key, short_id_slice, offset) {
                    return Some((offset, auth_key));
                }
            }
        }
        None
    }

    fn verify_at_offset(&self, buffer: &[u8], auth_key: &[u8; 32], short_id: &[u8], offset: usize) -> bool {
        if buffer.len() < 43 + offset + 8 { return false; }
        
        let hmac_key = hmac::Key::new(hmac::HMAC_SHA256, auth_key);
        let msg = &buffer[..43 + offset];
        let tag = hmac::sign(&hmac_key, msg);
        let expected = &tag.as_ref()[..8];
        let actual = &buffer[43 + offset..43 + offset + 8];
        
        if expected != actual { return false; }
        
        // 验证 short_id
        if buffer.len() >= 43 + offset + 8 + short_id.len() {
            let actual_short_id = &buffer[43 + offset + 8..43 + offset + 8 + short_id.len()];
            actual_short_id == short_id
        } else {
            false
        }
    }

    fn generate_reality_cert(&self, auth_key: &[u8; 32], host: &str) -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>)> {
        use rcgen::{CertificateParams, KeyPair, PKCS_ED25519};
        
        let key_pair = KeyPair::generate(&PKCS_ED25519)
            .map_err(|e| anyhow!("KeyPair gen failed: {}", e))?;
        let pub_key_raw = key_pair.public_key_raw().to_vec();
        
        let mut params = CertificateParams::new(vec![host.to_string()]);
        params.alg = &PKCS_ED25519;
        params.key_pair = Some(key_pair);
        
        let cert = rcgen::Certificate::from_params(params)
            .map_err(|e| anyhow!("Cert gen failed: {}", e))?;
        let mut cert_der = cert.serialize_der()
            .map_err(|e| anyhow!("Cert serialize failed: {}", e))?;
        let priv_key_der = cert.serialize_private_key_der();
        
        // Reality Signature: HMAC-SHA512(AuthKey, RawPublicKey)
        let total_len = cert_der.len();
        if total_len < 64 {
            bail!("CERT DER too short");
        }
        let sig_pos = total_len - 64;
        let ring_key = hmac::Key::new(hmac::HMAC_SHA512, auth_key);
        let signature = hmac::sign(&ring_key, &pub_key_raw);
        let sig_bytes = signature.as_ref(); 

        // Overwrite the signature at the end of DER
        cert_der[sig_pos..].copy_from_slice(sig_bytes);
        
        Ok((
            CertificateDer::from(cert_der),
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(priv_key_der))
        ))
    }
}

/// 带有前缀数据的 monoio 流包装器
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
        // 先返回前缀数据
        if self.prefix_offset < self.prefix.len() {
            let remaining = self.prefix.len() - self.prefix_offset;
            let to_copy = remaining.min(buf.bytes_total());
            let slice = unsafe { std::slice::from_raw_parts_mut(buf.write_ptr(), to_copy) };
            slice.copy_from_slice(&self.prefix[self.prefix_offset..self.prefix_offset + to_copy]);
            self.prefix_offset += to_copy;
            unsafe { buf.set_init(to_copy) };
            return (Ok(to_copy), buf);
        }
        
        // 前缀读完后，从内部流读取
        self.inner.read(buf).await
    }

    async fn readv<T: monoio::buf::IoVecBufMut>(&mut self, buf: T) -> monoio::BufResult<usize, T> {
        // 简化实现，直接委托
        self.inner.readv(buf).await
    }
}

impl<IO: AsyncWriteRent> AsyncWriteRent for PrefixedMonoioStream<IO> {
    async fn write<T: monoio::buf::IoBuf>(&mut self, buf: T) -> monoio::BufResult<usize, T> {
        self.inner.write(buf).await
    }

    async fn writev<T: monoio::buf::IoVecBuf>(&mut self, buf_vec: T) -> monoio::BufResult<usize, T> {
        self.inner.writev(buf_vec).await
    }

    async fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush().await
    }

    async fn shutdown(&mut self) -> std::io::Result<()> {
        self.inner.shutdown().await
    }
}

// 实现 Split trait 以支持双向传输
unsafe impl<IO: monoio::io::Split> monoio::io::Split for PrefixedMonoioStream<IO> {}
