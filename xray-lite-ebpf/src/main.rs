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

#[repr(C)]
pub struct UdpHdr {
    pub source: u16,
    pub dest: u16,
    pub len: u16,
    pub check: u16,
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

    // 1. Ethernet - CHECK BOUNDS FIRST
    if start + mem::size_of::<EthHdr>() > end {
        return Ok(xdp_action::XDP_PASS);
    }
    let eth_hdr = unsafe { &*(start as *const EthHdr) };
    if eth_hdr.etype != u16::to_be(0x0800) {
        return Ok(xdp_action::XDP_PASS);
    }

    // 2. IP - CHECK BOUNDS FIRST
    let ip_start = start + mem::size_of::<EthHdr>();
    if ip_start + mem::size_of::<IpHdr>() > end {
        return Ok(xdp_action::XDP_PASS);
    }
    let ip_hdr = unsafe { &*(ip_start as *const IpHdr) };

    let ihl = ip_hdr.version_ihl & 0x0F;
    let ip_len = (ihl as usize) * 4;
    let trans_start = ip_start + ip_len;

    // PROTOCOL DISPATCH
    // Check protocol: TCP=6, UDP=17
    match ip_hdr.protocol {
        // --- UDP HANDLING ---
        17 => {
            // Check bounds for UDP Header
            if trans_start + mem::size_of::<UdpHdr>() > end {
                return Ok(xdp_action::XDP_PASS);
            }
            let udp_hdr = unsafe { &*(trans_start as *const UdpHdr) };
            let dest_port = u16::from_be(udp_hdr.dest);

            // Check if port is protected
            if unsafe { ALLOWED_PORTS.get(&dest_port).is_some() } {
                // DROP all UDP traffic on protected ports (Anti-UDP Flood)
                warn!(&ctx, "⛔ Blocked UDP flood on protected port {}", dest_port);
                return Ok(xdp_action::XDP_DROP);
            }
            return Ok(xdp_action::XDP_PASS);
        }
        // --- TCP HANDLING ---
        6 => {
            // Check bounds for TCP Header
            if trans_start + mem::size_of::<TcpHdr>() > end {
                return Ok(xdp_action::XDP_PASS);
            }
            let tcp_hdr = unsafe { &*(trans_start as *const TcpHdr) };
            let dest_port = u16::from_be(tcp_hdr.dest);

            // Check if port is protected
            if unsafe { ALLOWED_PORTS.get(&dest_port).is_none() } {
                return Ok(xdp_action::XDP_PASS);
            }

            // --- Protected Port Logic (TCP) ---

            // 1. TCP Flags Check (Anti-DoS)
            let flags = tcp_hdr.flags;
            // SYN+FIN is illegal
            if (flags & 0x02 != 0) && (flags & 0x01 != 0) {
                warn!(
                    &ctx,
                    "⛔ Blocked illegal TCP flags (SYN+FIN) on port {}", dest_port
                );
                return Ok(xdp_action::XDP_DROP);
            }
            // SYN+RST is illegal
            if (flags & 0x02 != 0) && (flags & 0x04 != 0) {
                warn!(
                    &ctx,
                    "⛔ Blocked illegal TCP flags (SYN+RST) on port {}", dest_port
                );
                return Ok(xdp_action::XDP_DROP);
            }
            // No flags set (Null scan)
            if flags == 0 {
                warn!(
                    &ctx,
                    "⛔ Blocked illegal TCP flags (NULL) on port {}", dest_port
                );
                return Ok(xdp_action::XDP_DROP);
            }

            let doff = (tcp_hdr.res1 & 0xF0) >> 4;
            let tcp_len = (doff as usize) * 4;

            // 2. TLS/HTTP Deep Packet Inspection
            let payload_start = trans_start + tcp_len;

            // Check bounds for payload min size
            if payload_start + 5 > end {
                return Ok(xdp_action::XDP_PASS);
            }

            let content_type = unsafe { *(payload_start as *const u8) };

            // Log TLS ClientHello for debugging
            if content_type == 0x16 && payload_start + 6 <= end {
                let handshake_type = unsafe { *((payload_start + 5) as *const u8) };
                if handshake_type == 1 {
                    info!(&ctx, "TLS ClientHello detected on port {}", dest_port);
                }
            }

            // PASS all application layer traffic to Reality
            return Ok(xdp_action::XDP_PASS);
        }
        // --- OTHER PROTOCOLS (ICMP, etc.) ---
        _ => return Ok(xdp_action::XDP_PASS),
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    unsafe { core::hint::unreachable_unchecked() }
}
