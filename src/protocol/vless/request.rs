use anyhow::{anyhow, Result};
use bytes::{Buf, BufMut, BytesMut};
use uuid::Uuid;

use super::Address;

/// VLESS 协议版本
pub const VLESS_VERSION: u8 = 0;

/// VLESS 命令类型
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    /// TCP 连接
    Tcp = 0x01,
    /// UDP 连接
    Udp = 0x02,
    /// Mux (多路复用)
    Mux = 0x03,
}

impl Command {
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            0x01 => Ok(Command::Tcp),
            0x02 => Ok(Command::Udp),
            0x03 => Ok(Command::Mux),
            _ => Err(anyhow!("未知的命令类型: {}", value)),
        }
    }
}

/// VLESS 请求
#[derive(Debug, Clone)]
pub struct VlessRequest {
    /// 协议版本
    pub version: u8,
    /// 客户端 UUID
    pub uuid: Uuid,
    /// 命令类型
    pub command: Command,
    /// 目标地址
    pub address: Address,
    /// 附加数据长度
    pub addon_length: u8,
}

impl VlessRequest {
    /// 从字节流解码请求
    pub fn decode(buf: &mut BytesMut, allowed_uuids: &[Uuid]) -> Result<Self> {
        // 检查最小长度: version(1) + uuid(16) + addon_length(1) + command(1) + port(2) + addr_type(1)
        if buf.remaining() < 22 {
            return Err(anyhow!("缓冲区太小，无法解码 VLESS 请求"));
        }

        // 读取版本
        let version = buf.get_u8();
        eprintln!(
            "🔍 VLESS decode step 1: version byte = 0x{:02x} ({})",
            version, version
        );
        if version != VLESS_VERSION {
            return Err(anyhow!("不支持的 VLESS 版本: {}", version));
        }

        // 读取 UUID (16 字节)
        let mut uuid_bytes = [0u8; 16];
        buf.copy_to_slice(&mut uuid_bytes);
        let uuid = Uuid::from_bytes(uuid_bytes);

        // 验证 UUID
        if !allowed_uuids.contains(&uuid) {
            return Err(anyhow!("未授权的 UUID: {}", uuid));
        }

        // 读取附加数据长度
        let addon_length = buf.get_u8();
        eprintln!("🔍 VLESS decode step 3: addon_length = {}", addon_length);

        // 跳过附加数据
        if buf.remaining() < addon_length as usize {
            return Err(anyhow!("缓冲区太小，无法跳过附加数据"));
        }
        buf.advance(addon_length as usize);

        // 读取命令
        if buf.remaining() < 1 {
            return Err(anyhow!("缓冲区太小，无法读取命令"));
        }
        let cmd_byte = buf.get_u8();
        eprintln!("🔍 VLESS decode step 4: command byte = 0x{:02x}", cmd_byte);
        let command = Command::from_u8(cmd_byte)?;

        // 读取目标地址
        eprintln!(
            "🔍 VLESS decode step 5: starting address decode, remaining bytes = {}",
            buf.remaining()
        );
        let address = Address::decode(buf)?;

        Ok(VlessRequest {
            version,
            uuid,
            command,
            address,
            addon_length,
        })
    }

    /// 将请求编码为字节流
    pub fn encode(&self) -> Result<BytesMut> {
        let mut buf = BytesMut::new();

        // 写入版本
        buf.put_u8(self.version);

        // 写入 UUID
        buf.put_slice(self.uuid.as_bytes());

        // 写入附加数据长度 (暂时为 0)
        buf.put_u8(0);

        // 写入命令
        buf.put_u8(self.command as u8);

        // 写入地址
        self.address.encode(&mut buf);

        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn test_request_encode_decode() {
        let uuid = Uuid::parse_str("b831381d-6324-4d53-ad4f-8cda48b30811").unwrap();
        let request = VlessRequest {
            version: VLESS_VERSION,
            uuid,
            command: Command::Tcp,
            address: Address::Ipv4(Ipv4Addr::new(1, 1, 1, 1), 443),
            addon_length: 0,
        };

        let mut buf = request.encode().unwrap();
        let decoded = VlessRequest::decode(&mut buf, &[uuid]).unwrap();

        assert_eq!(request.version, decoded.version);
        assert_eq!(request.uuid, decoded.uuid);
        assert_eq!(request.command, decoded.command);
        assert_eq!(request.address, decoded.address);
    }

    #[test]
    fn test_unauthorized_uuid() {
        let uuid1 = Uuid::parse_str("b831381d-6324-4d53-ad4f-8cda48b30811").unwrap();
        let uuid2 = Uuid::parse_str("a831381d-6324-4d53-ad4f-8cda48b30812").unwrap();

        let request = VlessRequest {
            version: VLESS_VERSION,
            uuid: uuid1,
            command: Command::Tcp,
            address: Address::Ipv4(Ipv4Addr::new(1, 1, 1, 1), 443),
            addon_length: 0,
        };

        let mut buf = request.encode().unwrap();

        // 使用不同的 UUID 列表进行验证
        let result = VlessRequest::decode(&mut buf, &[uuid2]);
        assert!(result.is_err());
    }
}
