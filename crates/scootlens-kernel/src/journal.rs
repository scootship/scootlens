//! 审计 journal：append-only + SHA-256 哈希链（docs/04-kernel-design.md §4.7）。
//!
//! 每行 JSONL：`{"seq":n,"prev":"<hex>","hash":"<hex>","raw":"<entry json>"}`，
//! `hash = sha256(prev + raw)`。`raw` 保留写入时的精确字节序列，
//! 验证器不依赖 JSON 规范化。创世 `prev` 为 64 个 `0`。
//!
//! 铁律：先记后行——syscall 在 journal 记录成功之前不得产生副作用。

use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// journal 条目种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalKind {
    /// syscall 进入。
    Call,
    /// syscall 成功。
    Result,
    /// 鉴权拒绝（E_CAP_DENIED / E_QUOTA / E_APPROVAL_PENDING）。
    Deny,
    /// 审批决定。
    Approval,
}

/// journal 条目（`raw` 的内容）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub seq: u64,
    pub ts_ms: u64,
    pub kind: JournalKind,
    pub subject: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<String>,
    pub detail: serde_json::Value,
}

/// 链上一行。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalLine {
    pub seq: u64,
    pub prev: String,
    pub hash: String,
    pub raw: String,
}

enum Backend {
    File(std::fs::File),
    Memory,
}

struct Inner {
    backend: Backend,
    /// 内存副本（obs.journal 查询用；文件模式亦维护尾部窗口）。
    lines: Vec<JournalLine>,
    prev: String,
    seq: u64,
}

/// append-only journal。
pub struct Journal {
    inner: Mutex<Inner>,
}

const MEMORY_WINDOW: usize = 4096;

impl Journal {
    /// 文件模式：`dir/journal.jsonl`，已存在则接链续写。
    pub fn open(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("journal.jsonl");
        let existing = if path.exists() {
            let text = std::fs::read_to_string(&path)?;
            parse_lines(&text).map_err(std::io::Error::other)?
        } else {
            Vec::new()
        };
        let (prev, seq) = existing
            .last()
            .map_or((GENESIS.to_owned(), 0), |l| (l.hash.clone(), l.seq));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        Ok(Self {
            inner: Mutex::new(Inner {
                backend: Backend::File(file),
                lines: existing,
                prev,
                seq,
            }),
        })
    }

    /// 内存模式（无 state_dir 时）。
    pub fn in_memory() -> Self {
        Self {
            inner: Mutex::new(Inner {
                backend: Backend::Memory,
                lines: Vec::new(),
                prev: GENESIS.to_owned(),
                seq: 0,
            }),
        }
    }

    /// 追加一条记录，返回 seq。detail 必须已脱敏。
    pub fn record(
        &self,
        kind: JournalKind,
        subject: &str,
        method: &str,
        pid: Option<&str>,
        detail: serde_json::Value,
    ) -> u64 {
        let mut inner = self.lock();
        let seq = inner.seq + 1;
        let entry = JournalEntry {
            seq,
            ts_ms: crate::security::unix_now_ms(),
            kind,
            subject: subject.to_owned(),
            method: method.to_owned(),
            pid: pid.map(str::to_owned),
            detail,
        };
        let raw = serde_json::to_string(&entry).unwrap_or_else(|_| "{}".into());
        let hash = chain_hash(&inner.prev, &raw);
        let line = JournalLine {
            seq,
            prev: inner.prev.clone(),
            hash: hash.clone(),
            raw,
        };
        if let Backend::File(f) = &mut inner.backend {
            if let Ok(mut text) = serde_json::to_string(&line) {
                text.push('\n');
                if let Err(e) = f.write_all(text.as_bytes()) {
                    tracing::error!(error = %e, "journal write failed");
                }
            }
        }
        inner.lines.push(line);
        if inner.lines.len() > MEMORY_WINDOW {
            inner.lines.remove(0);
        }
        inner.prev = hash;
        inner.seq = seq;
        seq
    }

    /// 尾部条目（新→旧序返回最近 `limit` 条，可选 pid 过滤）。
    pub fn tail(&self, limit: usize, pid: Option<&str>) -> Vec<serde_json::Value> {
        let inner = self.lock();
        inner
            .lines
            .iter()
            .rev()
            .filter_map(|l| {
                let entry: JournalEntry = serde_json::from_str(&l.raw).ok()?;
                if let Some(want) = pid
                    && entry.pid.as_deref() != Some(want)
                {
                    return None;
                }
                serde_json::to_value(&entry).ok().map(|mut v| {
                    if let Some(obj) = v.as_object_mut() {
                        obj.insert("hash".into(), l.hash.clone().into());
                    }
                    v
                })
            })
            .take(limit)
            .collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn chain_hash(prev: &str, raw: &str) -> String {
    let mut h = Sha256::new();
    h.update(prev.as_bytes());
    h.update(raw.as_bytes());
    hex::encode(h.finalize())
}

/// 解析并验链。任何断链/篡改返回 Err。
pub fn parse_lines(text: &str) -> Result<Vec<JournalLine>, String> {
    let mut out = Vec::new();
    let mut prev = GENESIS.to_owned();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let l: JournalLine =
            serde_json::from_str(line).map_err(|e| format!("line {}: {e}", i + 1))?;
        if l.prev != prev {
            return Err(format!("line {}: chain broken (prev mismatch)", i + 1));
        }
        if chain_hash(&l.prev, &l.raw) != l.hash {
            return Err(format!("line {}: hash mismatch", i + 1));
        }
        prev = l.hash.clone();
        out.push(l);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn file_chain_parses_and_tamper_is_detected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let j = Journal::open(dir.path()).expect("open");
        j.record(
            JournalKind::Call,
            "agent:a",
            "proc.spawn",
            None,
            json!({ "k": 1 }),
        );
        j.record(
            JournalKind::Result,
            "agent:a",
            "proc.spawn",
            Some("p-1"),
            json!({ "ok": true }),
        );
        j.record(
            JournalKind::Deny,
            "agent:b",
            "js.exec",
            Some("p-1"),
            json!({ "code": "E_CAP_DENIED" }),
        );

        let text = std::fs::read_to_string(dir.path().join("journal.jsonl")).expect("read");
        assert_eq!(parse_lines(&text).expect("valid chain").len(), 3);

        // Altering any journaled field breaks the hash chain.
        let tampered = text.replacen("proc.spawn", "proc.evilx", 1);
        assert!(parse_lines(&tampered).is_err(), "tampered chain must fail");
    }

    #[test]
    fn open_resumes_existing_chain() {
        let dir = tempfile::tempdir().expect("tempdir");
        {
            let j = Journal::open(dir.path()).expect("open");
            j.record(JournalKind::Call, "s", "a", None, json!({}));
        }
        // Reopen and append; chain must stay valid across restart.
        let j = Journal::open(dir.path()).expect("reopen");
        let seq = j.record(JournalKind::Call, "s", "b", None, json!({}));
        assert_eq!(seq, 2, "seq continues after reopen");
        let text = std::fs::read_to_string(dir.path().join("journal.jsonl")).expect("read");
        assert!(parse_lines(&text).is_ok(), "resumed chain stays valid");
    }

    #[test]
    fn tail_filters_by_pid_newest_first() {
        let j = Journal::in_memory();
        j.record(JournalKind::Call, "s", "a", Some("p-1"), json!({}));
        j.record(JournalKind::Call, "s", "b", Some("p-2"), json!({}));
        j.record(JournalKind::Call, "s", "c", Some("p-1"), json!({}));
        let got = j.tail(10, Some("p-1"));
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["method"], "c", "newest first");
        assert_eq!(got[1]["method"], "a");
    }
}
