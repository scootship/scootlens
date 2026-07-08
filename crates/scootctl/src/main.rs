//! scootctl：命令行客户端。一次连接、一次调用、打印结果。
//!
//! ```text
//! scootctl --token <t> spawn
//! scootctl --token <t> goto p-1 https://example.com
//! scootctl --token <t> snapshot p-1
//! scootctl --token <t> click p-1 s1e3
//! ```

use std::process::ExitCode;

use base64::Engine as _;
use clap::{Parser, Subcommand};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

#[derive(Parser)]
#[command(name = "scootctl", version, about = "ScootLens CLI client")]
struct Args {
    /// 服务端 WS 端点。
    #[arg(long, default_value = "ws://127.0.0.1:9910/ws", env = "SCOOTLENS_URL")]
    url: String,

    /// 访问令牌。
    #[arg(long, env = "SCOOTLENS_TOKEN")]
    token: String,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 创建 Web 进程。
    Spawn {
        #[arg(long, default_value = "default")]
        profile: String,
    },
    /// 列出进程。
    Ps,
    /// 进程详情。
    Info { pid: String },
    /// 终止进程。
    Kill { pid: String },
    /// 导航。
    Goto { pid: String, url: String },
    /// 后退 / 前进 / 重载。
    Back { pid: String },
    Forward { pid: String },
    Reload { pid: String },
    /// 语义快照（打印紧凑文本）。
    Snapshot { pid: String },
    /// 截图（PNG 写文件）。
    Screenshot {
        pid: String,
        #[arg(long, default_value = "screenshot.png")]
        out: String,
    },
    /// 点击元素。
    Click { pid: String, r#ref: String },
    /// 输入文本。
    Type {
        pid: String,
        r#ref: String,
        text: String,
    },
    /// 按键（Enter/Tab/…）。
    Press { pid: String, keys: String },
    /// 系统信息。
    Sysinfo,
}

impl Cmd {
    fn into_rpc(self) -> (&'static str, Value) {
        match self {
            Cmd::Spawn { profile } => ("proc.spawn", json!({ "profile": profile })),
            Cmd::Ps => ("proc.list", json!({})),
            Cmd::Info { pid } => ("proc.info", json!({ "pid": pid })),
            Cmd::Kill { pid } => ("proc.kill", json!({ "pid": pid })),
            Cmd::Goto { pid, url } => ("nav.goto", json!({ "pid": pid, "url": url })),
            Cmd::Back { pid } => ("nav.back", json!({ "pid": pid })),
            Cmd::Forward { pid } => ("nav.forward", json!({ "pid": pid })),
            Cmd::Reload { pid } => ("nav.reload", json!({ "pid": pid })),
            Cmd::Snapshot { pid } => ("view.snapshot", json!({ "pid": pid })),
            Cmd::Screenshot { pid, .. } => ("view.screenshot", json!({ "pid": pid })),
            Cmd::Click { pid, r#ref } => ("act.click", json!({ "pid": pid, "ref": r#ref })),
            Cmd::Type { pid, r#ref, text } => {
                ("act.type", json!({ "pid": pid, "ref": r#ref, "text": text }))
            }
            Cmd::Press { pid, keys } => ("act.press", json!({ "pid": pid, "keys": keys })),
            Cmd::Sysinfo => ("sys.info", json!({})),
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args = Args::parse();
    match run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<(), String> {
    // screenshot 的输出路径先取出（into_rpc 消费 cmd）
    let screenshot_out = match &args.cmd {
        Cmd::Screenshot { out, .. } => Some(out.clone()),
        _ => None,
    };
    let (method, params) = args.cmd.into_rpc();

    let url = format!("{}?token={}", args.url, args.token);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| format!("connect {}: {e}", args.url))?;

    let req = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    ws.send(Message::Text(req.to_string().into()))
        .await
        .map_err(|e| format!("send: {e}"))?;

    let resp = loop {
        let msg = ws
            .next()
            .await
            .ok_or("connection closed")?
            .map_err(|e| format!("recv: {e}"))?;
        if let Message::Text(t) = msg {
            let v: Value = serde_json::from_str(&t).map_err(|e| format!("bad json: {e}"))?;
            if v["id"] == json!(1) {
                break v;
            }
        }
    };
    let _ = ws.close(None).await;

    if let Some(err) = resp.get("error") {
        return Err(format!(
            "{} ({}): {}",
            err["data"]["code"].as_str().unwrap_or("?"),
            err["code"],
            err["message"].as_str().unwrap_or("")
        ));
    }
    let result = &resp["result"];

    match method {
        "view.snapshot" => {
            print!("{}", result["text"].as_str().unwrap_or(""));
            if result["truncated"].as_bool() == Some(true) {
                eprintln!("(truncated)");
            }
        }
        "view.screenshot" => {
            let out = screenshot_out.unwrap_or_else(|| "screenshot.png".into());
            let data = result["data_base64"].as_str().unwrap_or("");
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(|e| format!("bad base64: {e}"))?;
            std::fs::write(&out, &bytes).map_err(|e| format!("write {out}: {e}"))?;
            println!("wrote {out} ({} bytes)", bytes.len());
        }
        _ => println!(
            "{}",
            serde_json::to_string_pretty(result).unwrap_or_else(|_| result.to_string())
        ),
    }
    Ok(())
}
