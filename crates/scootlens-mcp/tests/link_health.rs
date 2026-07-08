//! ABI 客户端链路健康集成测试：经一个可控 TCP 代理连接 gateway，
//! 模拟远程部署（反代/NAT）下的静默断链，验证：
//!
//! 1. 半开链路（字节黑洞）：在途调用在保活判死后**快速失败**，绝不无限挂起
//! 2. 判死后的下一次调用**自动重连**并成功
//!
//! 这正是 appserver 部署事故的回归测试：反代静默回收空闲 WS 后，
//! 旧实现的调用会永久挂起（MCP 客户端侧表现为 -32001 Request timed out）。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use scootlens_abi::{ApprovalMode, TokenClaims, TokenConstraints};
use scootlens_driver_mock::MockDriver;
use scootlens_gateway::{Gateway, GatewayConfig};
use scootlens_kernel::{Dispatcher, Kernel, KernelConfig};
use scootlens_mcp::{AbiClient, CallError, KeepaliveConfig};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// 起 gateway（mock 引擎，全权令牌）。
async fn start_gateway() -> (std::net::SocketAddr, String) {
    let kernel = Kernel::new(
        Arc::new(MockDriver::standard_fixture()),
        KernelConfig::default(),
    );
    let mut constraints = TokenConstraints::default();
    constraints.approval.insert("*".into(), ApprovalMode::Auto);
    let token = kernel.security().issue(&TokenClaims {
        subject: "user:test".into(),
        scopes: vec!["*".parse().expect("scope")],
        constraints,
        issued_by: "test".into(),
        issued_at: 0,
    });
    let gw = Gateway::new(Dispatcher::new(kernel), GatewayConfig::default());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { gw.serve(listener).await });
    (addr, token)
}

/// 字节级 TCP 代理。`blackhole` 置位后丢弃两个方向的一切字节且不关闭连接——
/// 模拟中间设备静默回收链路（半开：对端毫无感知）。
struct FlakyProxy {
    addr: std::net::SocketAddr,
    blackhole: Arc<AtomicBool>,
}

impl FlakyProxy {
    async fn start(upstream: std::net::SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind proxy");
        let addr = listener.local_addr().expect("proxy addr");
        let blackhole = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&blackhole);
        tokio::spawn(async move {
            while let Ok((client, _)) = listener.accept().await {
                let flag = Arc::clone(&flag);
                tokio::spawn(async move {
                    let Ok(server) = TcpStream::connect(upstream).await else {
                        return;
                    };
                    let (mut cr, mut cw) = client.into_split();
                    let (mut sr, mut sw) = server.into_split();
                    let f1 = Arc::clone(&flag);
                    let a = tokio::spawn(async move { pump(&mut cr, &mut sw, &f1).await });
                    let f2 = Arc::clone(&flag);
                    let b = tokio::spawn(async move { pump(&mut sr, &mut cw, &f2).await });
                    let _ = tokio::join!(a, b);
                });
            }
        });
        Self { addr, blackhole }
    }

    fn drop_traffic(&self) {
        self.blackhole.store(true, Ordering::SeqCst);
    }
}

/// 单向转发；blackhole 置位后只读不写（字节进黑洞），保持连接假活。
async fn pump(
    from: &mut (impl AsyncReadExt + Unpin),
    to: &mut (impl AsyncWriteExt + Unpin),
    blackhole: &AtomicBool,
) {
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = match from.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if blackhole.load(Ordering::SeqCst) {
            continue;
        }
        if to.write_all(&buf[..n]).await.is_err() {
            break;
        }
    }
}

#[tokio::test]
async fn half_open_link_fails_fast_then_reconnects() {
    let (gw_addr, token) = start_gateway().await;
    let proxy = FlakyProxy::start(gw_addr).await;
    let url = format!("ws://{}/ws?token={token}", proxy.addr);

    let client = AbiClient::connect_with_keepalive(
        &url,
        KeepaliveConfig {
            ping_interval: Duration::from_millis(50),
            idle_timeout: Duration::from_millis(200),
        },
    )
    .await
    .expect("connect via proxy");

    // 链路健康：正常往返
    let spawned = client.call("proc.spawn", json!({})).await.expect("spawn");
    assert!(spawned["pid"].is_string());

    // 反代静默断链（黑洞：不关 TCP，纯丢字节）
    proxy.drop_traffic();

    // 在途调用必须在保活判死窗口内快速失败，而不是永久挂起
    let outcome = tokio::time::timeout(Duration::from_secs(2), client.call("proc.list", json!({})))
        .await
        .expect("call must fail fast after link death, not hang");
    assert!(
        matches!(outcome, Err(CallError::Transport(_))),
        "expected transport error, got {outcome:?}"
    );

    // 下一次调用自动重连（代理接受新连接且已停止丢字节？黑洞是全局的——
    // 重连的新链路也会被丢字节，这里先恢复转发再验证重连）
    proxy.blackhole.store(false, Ordering::SeqCst);
    let listed = tokio::time::timeout(Duration::from_secs(3), async {
        // 判死与重连存在竞争窗口，容忍一次中间失败
        for _ in 0..10 {
            if let Ok(v) = client.call("proc.list", json!({})).await {
                return v;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("client never recovered");
    })
    .await
    .expect("reconnect within deadline");
    assert!(listed["procs"].is_array(), "unexpected: {listed}");
}
