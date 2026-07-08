//! ScootLens 守护进程（P1 起实现：gateway + kernel + 驱动组装）。

fn main() {
    println!(
        "{} {} (abi {})",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        scootlens_abi::ABI_VERSION
    );
}
