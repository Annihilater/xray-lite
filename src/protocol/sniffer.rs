/// 尝试从数据包中嗅探 TLS SNI (Server Name Indication)
/// 这是一个高效的纯 Rust 实现，旨在最小化内存分配
pub fn sniff_tls_sni(data: &[u8]) -> Option<String> {
    if data.len() < 43 {
        // Min ClientHello size
        return None;
    }

    let mut pos = 0;

    // 1. Record Layer: ContentType Handshake (0x16)
    if data.get(pos)? != &0x16 {
        return None;
    }
    pos += 5; // Skip ContentType(1), Version(2), Length(2)

    // 2. ClientHello Layer: HandshakeType ClientHello (0x01)
    if data.get(pos)? != &0x01 {
        return None;
    }

    // Skip HandshakeType(1), Length(3), Version(2), Random(32)
    pos += 38;

    // SessionID
    let sess_id_len = *data.get(pos)? as usize;
    pos += 1 + sess_id_len;

    // Cipher Suites
    if pos + 2 > data.len() {
        return None;
    }
    let cipher_len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
    pos += 2 + cipher_len;

    // Compression Methods
    if pos + 1 > data.len() {
        return None;
    }
    let comp_len = *data.get(pos)? as usize;
    pos += 1 + comp_len;

    // Extensions
    if pos + 2 > data.len() {
        return None;
    }
    let ext_len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
    pos += 2;

    let end_ext = pos + ext_len;
    if end_ext > data.len() {
        return None;
    }

    while pos + 4 <= end_ext {
        let ext_type = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let len = u16::from_be_bytes([data[pos + 2], data[pos + 3]]) as usize;
        pos += 4;

        if pos + len > end_ext {
            break;
        }

        if ext_type == 0x0000 {
            // ServerName Extension
            if len < 2 {
                return None;
            }
            let list_len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
            let mut p2 = pos + 2;
            let end_list = p2 + list_len;

            if end_list > pos + len {
                return None;
            }

            while p2 + 3 <= end_list {
                let name_type = data[p2];
                let name_len = u16::from_be_bytes([data[p2 + 1], data[p2 + 2]]) as usize;
                p2 += 3;

                if p2 + name_len > end_list {
                    break;
                }

                if name_type == 0x00 {
                    // HostName
                    return std::str::from_utf8(&data[p2..p2 + name_len])
                        .map(|s| s.to_string())
                        .ok();
                }
                p2 += name_len;
            }
        }
        pos += len;
    }

    None
}
