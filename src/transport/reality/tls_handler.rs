use anyhow::{anyhow, Result};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio_rustls::TlsAcceptor;
use tracing::{debug, info};

use super::RealityConfig;

/// Reality TLS 配置构建器
pub struct RealityTlsConfig {
    config: RealityConfig,
}

impl RealityTlsConfig {
    pub fn new(config: RealityConfig) -> Self {
        Self { config }
    }

    /// 生成自签名证书和私钥
    pub fn generate_self_signed_cert() -> Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
        use rcgen::{Certificate, CertificateParams, DistinguishedName};
        
        let mut params = CertificateParams::new(vec!["localhost".to_string()]);
        
        // 设置证书信息
        let mut dn = DistinguishedName::new();
        dn.push(rcgen::DnType::CommonName, "Reality Server");
        dn.push(rcgen::DnType::OrganizationName, "Reality");
        params.distinguished_name = dn;
        
        // 生成证书
        let cert = Certificate::from_params(params)
            .map_err(|e| anyhow!("Failed to generate certificate: {}", e))?;
        
        // 获取 DER 编码的证书
        let cert_der = cert.serialize_der()
            .map_err(|e| anyhow!("Failed to serialize certificate: {}", e))?;
        
        // 获取私钥
        let key_der = cert.serialize_private_key_der();
        
        let certs = vec![CertificateDer::from(cert_der)];
        let key = PrivateKeyDer::try_from(key_der)
            .map_err(|_| anyhow!("Failed to parse private key"))?;
        
        Ok((certs, key))
    }

    /// 构建 TLS ServerConfig
    pub fn build_tls_config(&self) -> Result<Arc<ServerConfig>> {
        let (certs, key) = Self::generate_self_signed_cert()?;
        
        let config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| anyhow!("Failed to build TLS config: {}", e))?;
        
        Ok(Arc::new(config))
    }

    /// 创建 TLS Acceptor
    pub fn create_acceptor(&self) -> Result<TlsAcceptor> {
        let config = self.build_tls_config()?;
        Ok(TlsAcceptor::from(config))
    }
}

/// Reality TLS 握手处理器
pub struct RealityTlsHandler {
    config: RealityConfig,
    acceptor: TlsAcceptor,
}

impl RealityTlsHandler {
    pub fn new(config: RealityConfig) -> Result<Self> {
        let tls_config = RealityTlsConfig::new(config.clone());
        let acceptor = tls_config.create_acceptor()?;
        
        Ok(Self {
            config,
            acceptor,
        })
    }

    /// 执行 Reality TLS 握手
    pub async fn perform_handshake(&self, stream: TcpStream) -> Result<tokio_rustls::server::TlsStream<TcpStream>> {
        debug!("Starting Reality TLS handshake");
        
        // 使用 rustls 执行完整的 TLS 1.3 握手
        let tls_stream = self.acceptor.accept(stream).await
            .map_err(|e| anyhow!("TLS handshake failed: {}", e))?;
        
        info!("TLS 1.3 handshake completed successfully");
        
        Ok(tls_stream)
    }
}
