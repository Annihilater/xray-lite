#![no_std]
#![no_main]

use aya_ebpf::{
    bindings::xdp_action,
    macros::{map, xdp},
    maps::HashMap,
    programs::XdpContext,
};
use aya_log_ebpf::{info, warn};
use core::mem;

#[map]
static ALLOWED_PORTS: HashMap<u16, u32> = HashMap::with_max_entries(1024, 0);

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
    let eth_hdr = unsafe { &*(start as *const EthHdr) };
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
    }

    let ihl = ip_hdr.version_ihl & 0x0F;
    let ip_len = (ihl as usize) * 4;

    // 3. TCP
    let tcp_start = ip_start + ip_len;
    if tcp_start + mem::size_of::<TcpHdr>() > end {
        return Ok(xdp_action::XDP_PASS);
    }
    let tcp_hdr = unsafe { &*(tcp_start as *const TcpHdr) };

    // Check if port is protected
    let dest_port = u16::from_be(tcp_hdr.dest);

    // Lookup in HashMap. If key exists, it returns Some(&value).
    if unsafe { ALLOWED_PORTS.get(&dest_port).is_none() } {
        // Port not in protection list -> PASS
        return Ok(xdp_action::XDP_PASS);
    }

    // --- Protected Port Logic Below ---

    let doff = (tcp_hdr.res1 & 0xF0) >> 4;
    let tcp_len = (doff as usize) * 4;

    // 4. TLS Deep Packet Inspection
    let payload_start = tcp_start + tcp_len;

    // Check minimal TLS header (5 bytes)
    if payload_start + 5 > end {
        // TCP packet without payload (ACK, SYN, FIN) -> PASS
        // We must allow TCP handshake packets!
        return Ok(xdp_action::XDP_PASS);
    }

    let content_type = unsafe { *(payload_start as *const u8) };

    // TLS Record Types:
    // 0x14 = ChangeCipherSpec
    // 0x15 = Alert
    // 0x16 = Handshake
    // 0x17 = ApplicationData
    // 0x18 = Heartbeat
    // If any of these, it's TLS -> PASS
    if content_type >= 0x14 && content_type <= 0x18 {
        // Valid TLS traffic -> PASS
        // Only log ClientHello for debugging
        if content_type == 0x16 && payload_start + 6 <= end {
            let handshake_type = unsafe { *((payload_start + 5) as *const u8) };
            if handshake_type == 1 {
                info!(&ctx, "TLS ClientHello passed on port {}", dest_port);
            }
        }
        return Ok(xdp_action::XDP_PASS);
    }

    // Conservative approach: Only DROP obvious HTTP probes
    // HTTP methods start with: G(ET), P(OST/UT/ATCH), H(EAD), D(ELETE), O(PTIONS), C(ONNECT), T(RACE)
    // ASCII: G=0x47, P=0x50, H=0x48, D=0x44, O=0x4F, C=0x43, T=0x54
    let is_http_probe = content_type == 0x47  // G
        || content_type == 0x50  // P
        || content_type == 0x48  // H
        || content_type == 0x44  // D
        || content_type == 0x4F  // O
        || content_type == 0x43  // C
        || content_type == 0x54; // T

    if is_http_probe {
        warn!(&ctx, "⛔ Blocked HTTP probe on port {}", dest_port);
        return Ok(xdp_action::XDP_DROP);
    }

    // For any other unknown traffic, PASS through to let application layer handle it
    // This is safer than aggressive DROP which can break legitimate connections
    Ok(xdp_action::XDP_PASS)
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { core::hint::unreachable_unchecked() }
}
