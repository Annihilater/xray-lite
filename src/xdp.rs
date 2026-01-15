#[cfg(feature = "xdp")]
pub mod loader {
    use aya::programs::XdpFlags;
    use aya::{include_bytes_aligned, programs::Xdp, Bpf};
    use aya_log::EbpfLogger;
    use tracing::{error, info, warn};
    use tokio;

    pub fn start_xdp(iface: &str) {
        let iface = iface.to_string();

        // Must use tokio::spawn to provide Reactor context for aya::log
        tokio::spawn(async move {
            info!("正在初始化 XDP 防火墙，接口: {}", iface);

            // 加载逻辑
            // 这里的路径是相对于 User Space crate root 的 (xray-lite/)
            #[cfg(debug_assertions)]
             let program_bytes = include_bytes_aligned!(
                "../xray-lite-ebpf/target/bpfel-unknown-none/release/xray-lite-ebpf"
            );
            #[cfg(not(debug_assertions))]
            let program_bytes = include_bytes_aligned!(
                "../xray-lite-ebpf/target/bpfel-unknown-none/release/xray-lite-ebpf"
            );

            // 加载 BPF
            let mut bpf = match Bpf::load(program_bytes) {
                Ok(b) => b,
                Err(e) => {
                    error!("XDP 加载失败: {}", e);
                    return;
                }
            };

            // 初始化日志：必须在 Tokio Runtime 上下文中调用
            if let Err(e) = EbpfLogger::init(&mut bpf) {
                warn!("XDP EbpfLogger 初始化失败 (非致命): {}", e);
            }

            // 挂载 XDP 程序
            let program: &mut Xdp = match bpf.program_mut("xdp_firewall").unwrap().try_into() {
                Ok(p) => p,
                Err(e) => {
                    error!("无法获取 xdp_firewall 程序: {}", e);
                    return;
                }
            };

            if let Err(e) = program.load() {
                error!("XDP 程序加载到内核失败: {}", e);
                return;
            }

            if let Err(e) = program.attach(&iface, XdpFlags::default()) {
                error!("XDP 程序挂载到接口 {} 失败: {}", iface, e);
                return;
            }

            info!(
                "🚀 XDP 防火墙已成功挂载到 {}！高性能内核级过滤生效中。",
                iface
            );

            // 保持 Async Task 存活，防止 Bpf 对象被 Drop
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
            }
        });
    }
}
