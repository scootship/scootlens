//! ScootLens CLI 客户端（P1 起实现）。

fn main() {
    println!(
        "{} {} (abi {})",
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        scootlens_abi::ABI_VERSION
    );
}
