//! ID 类型：进程 ID 与语义快照元素引用。

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// 进程 ID，格式 `p-<ascii 小写字母/数字>`，如 `p-a1b2c3`。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Pid(String);

impl Pid {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Pid {
    type Err = ParseIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let body = s.strip_prefix("p-").ok_or(ParseIdError::Pid)?;
        if body.is_empty()
            || !body
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        {
            return Err(ParseIdError::Pid);
        }
        Ok(Pid(s.to_owned()))
    }
}

impl TryFrom<String> for Pid {
    type Error = ParseIdError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<Pid> for String {
    fn from(p: Pid) -> String {
        p.0
    }
}

impl fmt::Display for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 语义快照元素引用，格式 `s<generation>e<index>`，如 `s3e17`。
///
/// `generation` 为快照代数；对过期代数的 ref 操作返回 `E_REF_STALE`。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ElementRef {
    generation: u64,
    index: u64,
}

impl ElementRef {
    pub fn new(generation: u64, index: u64) -> Self {
        Self { generation, index }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn index(&self) -> u64 {
        self.index
    }

    /// 相对当前快照代数是否已过期。
    pub fn is_stale(&self, current_generation: u64) -> bool {
        self.generation != current_generation
    }
}

impl FromStr for ElementRef {
    type Err = ParseIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let body = s.strip_prefix('s').ok_or(ParseIdError::ElementRef)?;
        let (gen_part, idx_part) = body.split_once('e').ok_or(ParseIdError::ElementRef)?;
        let is_digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
        if !is_digits(gen_part) || !is_digits(idx_part) {
            return Err(ParseIdError::ElementRef);
        }
        let generation = gen_part.parse().map_err(|_| ParseIdError::ElementRef)?;
        let index = idx_part.parse().map_err(|_| ParseIdError::ElementRef)?;
        Ok(Self { generation, index })
    }
}

impl TryFrom<String> for ElementRef {
    type Error = ParseIdError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<ElementRef> for String {
    fn from(r: ElementRef) -> String {
        r.to_string()
    }
}

impl fmt::Display for ElementRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "s{}e{}", self.generation, self.index)
    }
}

/// ID 解析错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseIdError {
    #[error("invalid pid: expected `p-<lowercase alnum>`")]
    Pid,
    #[error("invalid element ref: expected `s<gen>e<index>`")]
    ElementRef,
}
