#![no_std]
#![no_main]

use aya_ebpf::{bindings::xdp_action, macros::xdp, programs::XdpContext};
use aya_log_ebpf::info;
use core::mem;

#[repr(C)]
pub struct EthHdr {
    pub dst: [u8; 6],
    pub src: [u8; 6],
    pub etype: u16,
}

#[repr(C)]
pub struct IpHdr {
    pub version_ihl: u8,
    pub tos: u8,
    pub tot_len: u16,
    pub id: u16,
    pub frag_off: u16,
    pub ttl: u8,
    pub protocol: u8,
    pub check: u16,
    pub saddr: u32,
    pub daddr: u32,
}

#[repr(C)]
pub struct TcpHdr {
    pub source: u16,
    pub dest: u16,
    pub seq: u32,
    pub ack_seq: u32,
    pub res1: u8,
    pub flags: u8,
    pub window: u16,
    pub check: u16,
    pub urg_ptr: u16,
}

#[xdp]
pub fn xdp_firewall(ctx: XdpContext) -> u32 {
    match try_xdp_firewall(ctx) {
        Ok(ret) => ret,
        Err(_) => xdp_action::XDP_ABORTED,
    }
}

fn try_xdp_firewall(ctx: XdpContext) -> Result<u32, ()> {
    let start = ctx.data();
    let end = ctx.data_end();

    // 1. Ethernet
    let eth_hdr = unsafe { &*(start as *const EthHdr) }; // Safe pointer cast via ref
    if start + mem::size_of::<EthHdr>() > end {
        return Ok(xdp_action::XDP_PASS);
    }
    if eth_hdr.etype != u16::to_be(0x0800) {
        return Ok(xdp_action::XDP_PASS);
    }

    // 2. IP
    let ip_start = start + mem::size_of::<EthHdr>();
    if ip_start + mem::size_of::<IpHdr>() > end {
        return Ok(xdp_action::XDP_PASS);
    }
    let ip_hdr = unsafe { &*(ip_start as *const IpHdr) };
    if ip_hdr.protocol != 6 {
        return Ok(xdp_action::XDP_PASS);
    } // TCP only

    let ihl = ip_hdr.version_ihl & 0x0F;
    let ip_len = (ihl as usize) * 4;

    // 3. TCP
    let tcp_start = ip_start + ip_len;
    if tcp_start + mem::size_of::<TcpHdr>() > end {
        return Ok(xdp_action::XDP_PASS);
    }
    let tcp_hdr = unsafe { &*(tcp_start as *const TcpHdr) };

    // Calculate TCP header length (Data Offset)
    // res1 on Little Endian struct might be tricky, let's look at raw bytes if needed
    // But struct definition assumes standard C layout
    // Bitfields in C are tricky in Rust. We need to be careful.
    // Actually, `res1` in my struct definition covers the 4 bits of Data Offset if I defined it right?
    // standard TCP header:
    // | Source | Dest |
    // | Seq |
    // | Ack |
    // | DO(4) Res(3) NS(1) | Flags | Window |
    // My struct:
    // res1: u8 ?
    // Wait, u16 port (2) + u16 port (2) + u32 (4) + u32 (4) = 12 bytes.
    // Next is Data Offset (4 bits) + Reserved (3 bits) + NS (1 bit). Total 1 byte.
    // Then Flags (1 byte).
    // So `res1` corresponds to DO+Res+NS.

    let doff = (tcp_hdr.res1 & 0xF0) >> 4;
    let tcp_len = (doff as usize) * 4;

    // 4. TLS
    let payload_start = tcp_start + tcp_len;

    // Check minimal TLS header (5 bytes)
    if payload_start + 5 > end {
        return Ok(xdp_action::XDP_PASS);
    }

    let content_type = unsafe { *(payload_start as *const u8) };
    // ContentType::Handshake is 22 (0x16)
    if content_type == 0x16 {
        // Need to check Handshake type?
        // TLS Record (5 bytes) + Handshake Header (1 byte MsgType)
        if payload_start + 6 <= end {
            let handshake_type = unsafe { *((payload_start + 5) as *const u8) };
            if handshake_type == 1 {
                // ClientHello
                info!(&ctx, "TLS ClientHello detected!");
                // This is where we would parse SNI
                // For now, pass
            }
        }
    }

    Ok(xdp_action::XDP_PASS)
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { core::hint::unreachable_unchecked() }
}
