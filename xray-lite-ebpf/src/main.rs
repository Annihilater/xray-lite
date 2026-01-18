#![no_std]
#![no_main]

use aya_ebpf::{
    bindings::xdp_action,
    helpers::bpf_ktime_get_ns,
    macros::{map, xdp},
    maps::HashMap,
    programs::XdpContext,
};
use aya_log_ebpf::warn;
use core::mem;

// --- Map Definitions ---

// Allowed Ports (VLESS ports)
#[map]
static ALLOWED_PORTS: HashMap<u16, u8> = HashMap::with_max_entries(64, 0);

#[derive(Clone, Copy)]
#[repr(C)]
pub struct RateLimitEntry {
    pub last_time_ns: u64,
    pub count: u32,
}

// Track TCP SYN rates for source IPs
#[map]
static RATE_LIMIT_MAP: HashMap<u32, RateLimitEntry> = HashMap::with_max_entries(10240, 0);

// --- Constants ---
const ETH_P_IP: u16 = 0x0800;
const IPPROTO_TCP: u8 = 6;
const IPPROTO_UDP: u8 = 17;
// CONFIG: Max SYN packets per second per IP
const SYN_LIMIT_PER_SEC: u32 = 50;
const NANOS_PER_SEC: u64 = 1_000_000_000;

// --- Struct Definitions ---

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

// --- Logic ---

#[xdp]
pub fn xray_firewall(ctx: XdpContext) -> u32 {
    match try_xdp_firewall(ctx) {
        Ok(ret) => ret,
        Err(_) => xdp_action::XDP_ABORTED,
    }
}

fn try_xdp_firewall(ctx: XdpContext) -> Result<u32, ()> {
    let start = ctx.data();
    let end = ctx.data_end();

    // 1. Ethernet Header Check
    if start + mem::size_of::<EthHdr>() > end {
        return Ok(xdp_action::XDP_PASS);
    }
    let eth_hdr = unsafe { &*(start as *const EthHdr) };

    // Only handle IPv4
    if eth_hdr.etype != u16::to_be(ETH_P_IP) {
        return Ok(xdp_action::XDP_PASS);
    }

    // 2. IP Header Check
    let ip_start = start + mem::size_of::<EthHdr>();
    if ip_start + mem::size_of::<IpHdr>() > end {
        return Ok(xdp_action::XDP_PASS);
    }
    let ip_hdr = unsafe { &*(ip_start as *const IpHdr) };

    // Calculate IP header length to find transport header
    let ihl = ip_hdr.version_ihl & 0x0F;
    let ip_len = (ihl as usize) * 4;
    let trans_start = ip_start + ip_len;

    match ip_hdr.protocol {
        IPPROTO_UDP => {
            // UDP Header Check
            if trans_start + mem::size_of::<UdpHdr>() > end {
                return Ok(xdp_action::XDP_PASS);
            }
            let udp_hdr = unsafe { &*(trans_start as *const UdpHdr) };
            let dest_port = u16::from_be(udp_hdr.dest);

            // Check Allowed Ports Map
            if unsafe { ALLOWED_PORTS.get(&dest_port).is_some() } {
                // DROP all UDP traffic on protected ports (Anti-UDP Flood)
                return Ok(xdp_action::XDP_DROP);
            }
            return Ok(xdp_action::XDP_PASS);
        }
        IPPROTO_TCP => {
            // TCP Header Check
            if trans_start + mem::size_of::<TcpHdr>() > end {
                return Ok(xdp_action::XDP_PASS);
            }
            let tcp_hdr = unsafe { &*(trans_start as *const TcpHdr) };
            let dest_port = u16::from_be(tcp_hdr.dest);

            // Only protect ports in the ALLOWED_PORTS map
            if unsafe { ALLOWED_PORTS.get(&dest_port).is_none() } {
                return Ok(xdp_action::XDP_PASS);
            }

            // --- Logic for Protected Ports ---
            let flags = tcp_hdr.flags;

            // 1. Illegal Flags Check
            // SYN+FIN (0x02 | 0x01)
            if (flags & 0x03) == 0x03 {
                return Ok(xdp_action::XDP_DROP);
            }
            // SYN+RST (0x02 | 0x04)
            if (flags & 0x06) == 0x06 {
                return Ok(xdp_action::XDP_DROP);
            }

            // 2. SYN Rate Limit (Per Source IP)
            // Check if SYN is set (0x02) and ACK is NOT set (0x10) -> New Connection Attempt
            if (flags & 0x02 != 0) && (flags & 0x10 == 0) {
                let src_ip = u32::from_be(ip_hdr.saddr);
                let now = unsafe { bpf_ktime_get_ns() };

                // Lookup IP in Rate Limit Map
                match unsafe { RATE_LIMIT_MAP.get_ptr_mut(&src_ip) } {
                    Some(entry) => {
                        let entry = unsafe { &mut *entry };

                        // Convert nanoseconds to seconds for comparison is costly, keep naive check
                        // If more than 1 second has passed
                        if now > entry.last_time_ns + NANOS_PER_SEC {
                            // Reset window
                            entry.last_time_ns = now;
                            entry.count = 1;
                        } else {
                            // Within same second window
                            entry.count += 1;

                            if entry.count > SYN_LIMIT_PER_SEC {
                                // Log occasionally to avoid flooding trace pipe
                                if entry.count % 100 == 0 {
                                    warn!(
                                        &ctx,
                                        "⛔ RATELIMIT: Dropped SYN flood from IP {:x}", src_ip
                                    );
                                }
                                return Ok(xdp_action::XDP_DROP);
                            }
                        }
                    }
                    None => {
                        // New IP, insert entry
                        let new_entry = RateLimitEntry {
                            last_time_ns: now,
                            count: 1,
                        };
                        let _ = unsafe { RATE_LIMIT_MAP.insert(&src_ip, &new_entry, 0) };
                    }
                }
            }

            return Ok(xdp_action::XDP_PASS);
        }
        _ => return Ok(xdp_action::XDP_PASS),
    }
}
