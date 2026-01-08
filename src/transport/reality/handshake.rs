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

        // 3. 生成服务器密钥对和共享密钥
        let crypto = RealityCrypto::new();
        let my_public_key = crypto.get_public_key();
        let shared_secret = crypto.derive_shared_secret(&client_key_share)?;

        // 4. 构造 ServerHello (带随机熵)
        use rand::RngCore;
        let mut server_random = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut server_random);

        let mut server_hello = super::tls::ServerHello::new_reality(
            &client_hello.session_id,
            server_random,
            &my_public_key
        )?;
        
        // 5. 注入 Reality Auth (HMAC-SHA256)
        server_hello.modify_for_reality(&self.config.private_key, &client_hello.random)?;

        // 6. 发送 ServerHello & CCS
        let server_hello_record = server_hello.encode();
        client_stream.write_all(&server_hello_record).await?;
        client_stream.write_all(&[0x14, 0x03, 0x03, 0x00, 0x01, 0x01]).await?;
        debug!("ServerHello & CCS sent");

        // 7. Derive Handshake Keys
        let transcript1 = vec![client_hello_payload.as_slice(), server_hello.handshake_payload()];
        let (mut hs_keys, handshake_secret) = TlsKeys::derive_handshake_keys(
            &shared_secret, 
            &super::crypto::hash_transcript(&transcript1)
        )?;
        
        // 8. 准备加密握手消息
        
        // 8.1 EncryptedExtensions (EE)
        let mut ee_content = BytesMut::new();
        ee_content.put_u16(16); // Type ALPN
        let mut alpn_list = BytesMut::new();
        alpn_val(&mut alpn_list, b"h2"); // v0.1.16: 仅返回 h2
        
        ee_content.put_u16((alpn_list.len() + 2) as u16); 
        ee_content.put_u16(alpn_list.len() as u16);      
        ee_content.put_slice(&alpn_list);

        let mut ee_msg = BytesMut::new();
        ee_msg.put_u8(8); // Type EE
        let ee_len = (ee_content.len() + 2) as u32;
        ee_msg.put_slice(&ee_len.to_be_bytes()[1..4]);
        ee_msg.put_u16(ee_content.len() as u16); 
        ee_msg.put_slice(&ee_content);
        
        // 8.2 Certificate (Empty list for now)
        let mut cert_msg = BytesMut::new();
        cert_msg.put_u8(11); // Type Certificate
        cert_msg.put_u8(0); cert_msg.put_u8(0); cert_msg.put_u8(4);
        cert_msg.put_u8(0); // Context Len
        cert_msg.put_u8(0); cert_msg.put_u8(0); cert_msg.put_u8(0); // List Len (Empty)
        
        // 8.3 Finished (Fin)
        let transcript2 = vec![
            client_hello_payload.as_slice(), 
            server_hello.handshake_payload(),
            &ee_msg,
            &cert_msg
        ];
        let hash2 = super::crypto::hash_transcript(&transcript2);
        let verify_data = TlsKeys::calculate_verify_data(&hs_keys.server_traffic_secret, &hash2)?;
        
        let mut fin_msg = BytesMut::new();
        fin_msg.put_u8(20); // Type Finished
        let fin_len = verify_data.len() as u32;
        fin_msg.put_slice(&fin_len.to_be_bytes()[1..4]);
        fin_msg.put_slice(&verify_data);
        
        // 9. 将 EE, Cert, Fin 打包成一个加密记录发送 (v0.1.16 修订)
        let mut bundled_handshake = BytesMut::new();
        bundled_handshake.put_slice(&ee_msg);
        bundled_handshake.put_slice(&cert_msg);
        bundled_handshake.put_slice(&fin_msg);
        
        let hs_cipher = hs_keys.encrypt_server_record(0, &bundled_handshake, 22)?;
        client_stream.write_all(&hs_cipher).await?;
        info!("Handshake bundled messages sent, waiting for client...");

        // 10. 读取客户端响应
        let mut buf = BytesMut::with_capacity(4096);
        let mut client_finished_payload = Vec::new();
        let mut client_seq = 0;
        
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
            
            if content_type == 21 { // Unencrypted Alert
                return Err(anyhow!("Received Unencrypted Alert: {}/{}", record_data[5], record_data[6]));
            }
            
            if content_type == 23 { // App Data
                let mut header = [0u8; 5];
                header.copy_from_slice(&record_data[..5]);
                let ciphertext = &mut record_data[5..];
                
                let (ctype, plen) = hs_keys.decrypt_client_record(client_seq, &header, ciphertext)?;
                client_seq += 1;
                
                if ctype == 21 { // Encrypted Alert
                    let level = if plen > 0 { ciphertext[0] } else { 0 };
                    let desc = if plen > 1 { ciphertext[1] } else { 0 };
                    warn!("Received Client TLS Alert: level={}, description={}", level, desc);
                    return Err(anyhow!("Handshake failed: Client sent TLS Alert {}/{}", level, desc));
                }

                if ctype == 22 && plen > 0 && ciphertext[0] == 20 { // Finished
                    client_finished_payload = ciphertext[..plen].to_vec();
                    debug!("Client Finished decrypted successfully");
                    break;
                }
                
                // 忽略其他握手消息或继续读取
                continue;
            }
            return Err(anyhow!("Unexpected Record Type: {}", content_type));
        }
        
        // 11. Derive Application Keys
        // Transcript Hash for app keys: CH...ServerFinished
        let transcript_final = vec![
            client_hello_payload.as_slice(), 
            server_hello.handshake_payload(),
            &ee_msg,
            &cert_msg,
            &fin_msg
        ];
        let hash_final = super::crypto::hash_transcript(&transcript_final);
        let app_keys = TlsKeys::derive_application_keys(&handshake_secret, &hash_final)?;
        
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
