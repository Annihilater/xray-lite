use anyhow::{anyhow, Result};
use bytes::{BytesMut, Buf, BufMut};
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

    /// 执行 Reality TLS 握手
    pub async fn perform(&self, mut client_stream: TcpStream) -> Result<super::stream::TlsStream<TcpStream>> {
        // 1. 读取 ClientHello
        let (client_hello, client_hello_payload) = self.read_client_hello(&mut client_stream).await?;
        debug!("ClientHello received, SNI: {:?}", client_hello.get_sni());

        // 2. 提取 Client Key Share
        let client_key_share = match client_hello.get_key_share() {
            Some(key) => {
                info!("Got Client Key Share, len: {}", key.len());
                key
            },
            None => return Err(anyhow!("客户端未使用 X25519 Key Share")),
        };

        // 3. 生成我们的密钥对
        let crypto = RealityCrypto::new();
        let my_public_key = crypto.get_public_key();

        // 4. 计算 Shared Secret (ECDH)
        let shared_secret = crypto.derive_shared_secret(&client_key_share)?;
        debug!("ECDH Shared Secret derived");

        // 5. 构造 ServerHello
        let temp_random = [0u8; 32];
        let mut server_hello = super::tls::ServerHello::new_reality(
            &client_hello.session_id,
            temp_random,
            &my_public_key
        )?;
        
        // 6. 注入 Reality Auth
        server_hello.modify_for_reality(&self.config.private_key, &client_hello.random)?;

        // 7. 发送 ServerHello
        let server_hello_record = server_hello.encode();
        client_stream.write_all(&server_hello_record).await?;
        debug!("ServerHello sent");

        // 7.5 发送虚假 CCS (ChangeCipherSpec) 以满足 Middlebox 兼容模式
        client_stream.write_all(&[0x14, 0x03, 0x03, 0x00, 0x01, 0x01]).await?;
        debug!("Dummy CCS sent");
        
        // 8. Derive Handshake Keys
        let transcript1 = vec![client_hello_payload.as_slice(), server_hello.handshake_payload()];
        let (mut keys, handshake_secret) = TlsKeys::derive_handshake_keys(
            &shared_secret, 
            &super::crypto::hash_transcript(&transcript1)
        )?;
        
        // 9. Send EncryptedExtensions (Seq = 0)
        let mut ee_content = BytesMut::new();
        // Extension: ALPN (0x0010)
        ee_content.put_u16(16); // Type ALPN
        let mut alpn_list = BytesMut::new();
        alpn_val(&mut alpn_list, b"h2");
        alpn_val(&mut alpn_list, b"http/1.1");
        
        ee_content.put_u16((alpn_list.len() + 2) as u16); // Extension Data Length
        ee_content.put_u16(alpn_list.len() as u16);      // Protocol Name List Length
        ee_content.put_slice(&alpn_list);

        let mut ee_msg = BytesMut::new();
        ee_msg.put_u8(8); // Type EncryptedExtensions
        let ee_payload_len = ee_content.len() + 2;
        // Handshake Length (3 bytes)
        let len_bytes = (ee_payload_len as u32).to_be_bytes();
        ee_msg.put_u8(len_bytes[1]); ee_msg.put_u8(len_bytes[2]); ee_msg.put_u8(len_bytes[3]);
        // Extensions Length (2 bytes)
        ee_msg.put_u16(ee_content.len() as u16); 
        ee_msg.put_slice(&ee_content);
        
        let ee_cipher = keys.encrypt_server_record(0, &ee_msg, 22)?;
        client_stream.write_all(&ee_cipher).await?;

        // 9.5 Send Certificate (Empty) (Seq = 1)
        let mut cert_msg = BytesMut::new();
        cert_msg.put_u8(11); // Type Certificate
        cert_msg.put_u8(0); cert_msg.put_u8(0); cert_msg.put_u8(4);
        cert_msg.put_u8(0); // Context Len
        cert_msg.put_u8(0); cert_msg.put_u8(0); cert_msg.put_u8(0); // List Len
        
        let cert_cipher = keys.encrypt_server_record(1, &cert_msg, 22)?;
        client_stream.write_all(&cert_cipher).await?;
        
        // 10. Send Finished (Seq = 2)
        let transcript2 = vec![
            client_hello_payload.as_slice(), 
            server_hello.handshake_payload(),
            &ee_msg,
            &cert_msg
        ];
        let hash2 = super::crypto::hash_transcript(&transcript2);
        let verify_data = TlsKeys::calculate_verify_data(&keys.server_traffic_secret, &hash2)?;
        
        let mut fin_msg = BytesMut::new();
        fin_msg.put_u8(20); // Type Finished
        let fin_len_bytes = (verify_data.len() as u32).to_be_bytes();
        fin_msg.put_u8(fin_len_bytes[1]); fin_msg.put_u8(fin_len_bytes[2]); fin_msg.put_u8(fin_len_bytes[3]);
        fin_msg.put_slice(&verify_data);
        
        let fin_cipher = keys.encrypt_server_record(2, &fin_msg, 22)?;
        client_stream.write_all(&fin_cipher).await?;
        
        info!("Handshake sent, waiting for client response...");

        // 11. Read Client Finished
        let mut buf = BytesMut::with_capacity(4096);
        let mut client_finished_payload = Vec::new();
        
        loop {
            if buf.len() < 5 {
                let n = client_stream.read_buf(&mut buf).await?;
                if n == 0 { return Err(anyhow!("Connection closed by client")); }
                if buf.len() < 5 { continue; }
            }
            
            let content_type = buf[0];
            let len = u16::from_be_bytes([buf[3], buf[4]]) as usize;
            if buf.len() < 5 + len {
                let n = client_stream.read_buf(&mut buf).await?;
                if n == 0 { return Err(anyhow!("EOF reading record body")); }
                continue;
            }
            
            let mut record_data = buf.split_to(5 + len);
            
            if content_type == 20 { // CCS
                debug!("Skipping CCS record");
                continue;
            }
            
            if content_type == 21 { // Alert
                let mut header = [0u8; 5];
                header.copy_from_slice(&record_data[..5]);
                let ciphertext = &mut record_data[5..];
                
                if let Ok((ctype, plen)) = keys.decrypt_client_record(0, &header, ciphertext) {
                    if ctype == 21 && plen >= 2 {
                        warn!("Received Client Alert: level={}, description={}", ciphertext[0], ciphertext[1]);
                    }
                }
                return Err(anyhow!("Handshake failed: Received Alert(21) from client"));
            }
            
            if content_type == 23 { // App Data
                let mut header = [0u8; 5];
                header.copy_from_slice(&record_data[..5]);
                let ciphertext = &mut record_data[5..];
                
                let (ctype, plen) = keys.decrypt_client_record(0, &header, ciphertext)?;
                if ctype != 22 { return Err(anyhow!("Expected Handshake(22), got {}", ctype)); }
                
                if plen > 0 && ciphertext[0] == 20 {
                    client_finished_payload = ciphertext[..plen].to_vec();
                    debug!("Client Finished decrypted successfully");
                    break;
                }
            }
            return Err(anyhow!("Unexpected Record Type: {}", content_type));
        }
        
        // 12. Derive Application Keys
        let transcript3 = vec![
            client_hello_payload.as_slice(), 
            server_hello.handshake_payload(),
            &ee_msg,
            &cert_msg,
            &fin_msg, 
            &client_finished_payload
        ];
        let hash3 = super::crypto::hash_transcript(&transcript3);
        let app_keys = TlsKeys::derive_application_keys(&handshake_secret, &hash3)?;
        
        info!("Reality handshake successful! Tunnel established.");
        Ok(super::stream::TlsStream::new_with_buffer(client_stream, app_keys, buf))
    }

    async fn read_client_hello(&self, stream: &mut TcpStream) -> Result<(ClientHello, Vec<u8>)> {
        let mut buf = BytesMut::with_capacity(4096);
        loop {
            let n = stream.read_buf(&mut buf).await?;
            if n == 0 { return Err(anyhow!("EOF reading ClientHello")); }
            let mut parse_buf = buf.clone();
            if let Some(record) = TlsRecord::parse(&mut parse_buf)? {
                if record.content_type == super::tls::ContentType::Handshake {
                     let client_hello = ClientHello::parse(&record.payload)?;
                     return Ok((client_hello, record.payload));
                }
            }
        }
    }
}

fn alpn_val(buf: &mut BytesMut, name: &[u8]) {
    buf.put_u8(name.len() as u8);
    buf.put_slice(name);
}
