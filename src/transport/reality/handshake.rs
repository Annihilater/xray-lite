use anyhow::{anyhow, Result};
use bytes::{BytesMut, Buf};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{debug, info, warn};

use super::tls::{ClientHello, TlsRecord};
use super::RealityConfig;
use super::crypto::{RealityCrypto, TlsKeys};

#[derive(Clone)]
pub struct RealityHandshake {
    config: RealityConfig,
}

impl RealityHandshake {
    pub fn new(config: RealityConfig) -> Self {
        Self { config }
    }

    pub async fn perform(&self, mut client_stream: TcpStream) -> Result<super::stream::TlsStream<TcpStream>> {
        // 1. Read ClientHello
        let (client_hello, client_hello_payload) = self.read_client_hello(&mut client_stream).await?;
        debug!("ClientHello received");

        // 2. Extract Client Key Share
        let client_key_share = match client_hello.get_key_share() {
            Some(key) => key,
            None => return Err(anyhow!("No X25519 key share")),
        };

        // 3. Key Exchange
        let crypto = RealityCrypto::new();
        let my_public_key = crypto.get_public_key();
        let shared_secret = crypto.derive_shared_secret(&client_key_share)?;

        // 4. ServerHello
        use rand::RngCore;
        let mut server_random = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut server_random);

        let mut server_hello = super::tls::ServerHello::new_reality(
            &client_hello.session_id,
            server_random,
            &my_public_key
        )?;
        server_hello.modify_for_reality(&self.config.private_key, &client_hello.random)?;

        // 5. Send ServerHello & CCS
        client_stream.write_all(&server_hello.encode()).await?;
        client_stream.write_all(&[0x14, 0x03, 0x03, 0x00, 0x01, 0x01]).await?;
        debug!("ServerHello & CCS sent");

        // 6. Derive Handshake Keys
        let transcript0 = vec![client_hello_payload.as_slice(), server_hello.handshake_payload()];
        let (mut hs_keys, handshake_secret) = TlsKeys::derive_handshake_keys(
            &shared_secret, 
            &super::crypto::hash_transcript(&transcript0)
        )?;
        
        // 7. Build and send encrypted handshake messages
        
        // 7.1 EncryptedExtensions (empty)
        let ee_msg = vec![8, 0, 0, 2, 0, 0]; // Type: EE, Len: 2, ExtLen: 0
        
        // 7.2 Certificate (with dummy cert)
        let cert_msg = super::cert_gen::generate_dummy_certificate();
        
        // 7.3 CertificateVerify
        let transcript_cv = vec![
            client_hello_payload.as_slice(),
            server_hello.handshake_payload(),
            &ee_msg,
            &cert_msg
        ];
        let hash_cv = super::crypto::hash_transcript(&transcript_cv);
        let cv_msg = super::cert_gen::generate_certificate_verify(&hash_cv);
        
        // 7.4 Finished
        let transcript_fin = vec![
            client_hello_payload.as_slice(),
            server_hello.handshake_payload(),
            &ee_msg,
            &cert_msg,
            &cv_msg
        ];
        let hash_fin = super::crypto::hash_transcript(&transcript_fin);
        let verify_data = TlsKeys::calculate_verify_data(&hs_keys.server_traffic_secret, &hash_fin)?;
        
        let mut fin_msg = BytesMut::new();
        fin_msg.extend_from_slice(&[20]); // Type: Finished
        let fin_len_bytes = (verify_data.len() as u32).to_be_bytes();
        fin_msg.extend_from_slice(&fin_len_bytes[1..4]);
        fin_msg.extend_from_slice(&verify_data);
        
        // 8. Send all encrypted messages separately
        let ee_record = hs_keys.encrypt_server_record(0, &ee_msg, 22)?;
        client_stream.write_all(&ee_record).await?;
        debug!("EncryptedExtensions sent (seq=0)");
        
        let cert_record = hs_keys.encrypt_server_record(1, &cert_msg, 22)?;
        client_stream.write_all(&cert_record).await?;
        debug!("Certificate sent (seq=1)");
        
        let cv_record = hs_keys.encrypt_server_record(2, &cv_msg, 22)?;
        client_stream.write_all(&cv_record).await?;
        debug!("CertificateVerify sent (seq=2)");
        
        let fin_record = hs_keys.encrypt_server_record(3, &fin_msg, 22)?;
        client_stream.write_all(&fin_record).await?;
        debug!("Finished sent (seq=3)");
        
        info!("Server handshake complete, waiting for client Finished...");

        // 9. Read Client Finished
        let mut buf = BytesMut::with_capacity(4096);
        
        loop {
            if buf.len() < 5 {
                let n = client_stream.read_buf(&mut buf).await?;
                if n == 0 { return Err(anyhow!("Connection closed")); }
                if buf.len() < 5 { continue; }
            }
            
            let ctype = buf[0];
            let rlen = u16::from_be_bytes([buf[3], buf[4]]) as usize;
            
            if buf.len() < 5 + rlen {
                let n = client_stream.read_buf(&mut buf).await?;
                if n == 0 { return Err(anyhow!("EOF")); }
                continue;
            }
            
            let mut record_data = buf.split_to(5 + rlen);
            
            if ctype == 20 { continue; } // Skip CCS
            
            if ctype == 23 {
                let mut header = [0u8; 5];
                header.copy_from_slice(&record_data[..5]);
                let (inner_type, plen) = hs_keys.decrypt_client_record(0, &header, &mut record_data[5..])?;
                
                if inner_type == 21 {
                    warn!("Client Alert: {}/{}", record_data[5], if plen > 1 { record_data[6]} else { 0 });
                    return Err(anyhow!("Client sent Alert"));
                }
                
                if inner_type == 22 && plen > 0 && record_data[5] == 20 {
                    info!("Client Finished received!");
                    break;
                }
            }
        }
        
        // 10. Derive Application Keys
        let transcript_app = vec![
            client_hello_payload.as_slice(),
            server_hello.handshake_payload(),
            &ee_msg,
            &cert_msg,
            &cv_msg,
            &fin_msg
        ];
        let app_keys = TlsKeys::derive_application_keys(&handshake_secret, &super::crypto::hash_transcript(&transcript_app))?;
        
        info!("🎉 Reality handshake successful! Tunnel established.");
        Ok(super::stream::TlsStream::new_with_buffer(client_stream, app_keys, buf))
    }

    async fn read_client_hello(&self, stream: &mut TcpStream) -> Result<(ClientHello, Vec<u8>)> {
        let mut buf = BytesMut::with_capacity(4096);
        loop {
            let n = stream.read_buf(&mut buf).await?;
            if n == 0 { return Err(anyhow!("EOF reading CH")); }
            let mut parse_buf = buf.clone();
            if let Some(record) = TlsRecord::parse(&mut parse_buf)? {
                if record.content_type == super::tls::ContentType::Handshake {
                     let ch = ClientHello::parse(&record.payload)?;
                     return Ok((ch, record.payload));
                }
            }
        }
    }
}
