//! 出口消毒：vault 凭据一经注入，其明文值不得出现在任何 syscall
//! 返回值、journal、trace 中（docs/06-security-model.md T3）。

use std::sync::{Mutex, PoisonError};

use serde_json::Value;

const MASK: &str = "[REDACTED]";

/// 参与**子串**替换的最小秘密长度（字节）。更短的串（如 `demo`、`aaa`、`1234`）
/// 在任意文本里大量自然出现，盲目子串替换会把无关数据（凭据名、URL、页面文本）
/// 误伤成 [`MASK`]。短秘密照常登记，但只做**整值精确匹配**遮蔽——典型泄漏形态
/// （值字段原样回流）仍被拦住；journal 里 vault 写入的 value 另有结构化遮蔽兜底
/// （见 dispatch）。
pub const SUBSTRING_MIN_LEN: usize = 6;

/// journal 等处的遮蔽字面量。
pub(crate) fn mask() -> String {
    MASK.to_owned()
}

/// 秘密值登记表 + 递归替换。
#[derive(Default)]
pub struct Redactor {
    secrets: Mutex<Vec<String>>,
}

impl Redactor {
    /// 登记一个已注入的秘密值（长度不限；空串忽略——会把一切遮蔽掉）。
    pub fn add(&self, secret: &str) {
        if secret.is_empty() {
            return;
        }
        let mut s = self.lock();
        if !s.iter().any(|x| x == secret) {
            s.push(secret.to_owned());
        }
    }

    /// 递归消毒 JSON 值（字符串内容替换，含 key 对应的值与数组元素）。
    pub fn sanitize(&self, v: &mut Value) {
        let secrets = self.lock().clone();
        if secrets.is_empty() {
            return;
        }
        sanitize_value(v, &secrets);
    }

    pub fn sanitize_str(&self, s: &str) -> String {
        let secrets = self.lock().clone();
        let mut out = s.to_owned();
        for sec in &secrets {
            if sec.len() >= SUBSTRING_MIN_LEN {
                if out.contains(sec.as_str()) {
                    out = out.replace(sec.as_str(), MASK);
                }
            } else if out == *sec {
                out = MASK.to_owned();
            }
        }
        out
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<String>> {
        self.secrets.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn sanitize_value(v: &mut Value, secrets: &[String]) {
    match v {
        Value::String(s) => {
            for sec in secrets {
                // 长秘密：子串替换；短秘密：仅整值精确匹配（防误伤无关数据）
                if sec.len() >= SUBSTRING_MIN_LEN {
                    if s.contains(sec.as_str()) {
                        *s = s.replace(sec.as_str(), MASK);
                    }
                } else if s == sec {
                    *s = MASK.to_owned();
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                sanitize_value(item, secrets);
            }
        }
        Value::Object(map) => {
            for (_, val) in map.iter_mut() {
                sanitize_value(val, secrets);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn masks_secrets_recursively() {
        let r = Redactor::default();
        r.add("SECRET-A");
        r.add("SECRET-B");
        let mut v = json!({
            "a": "value SECRET-A here",
            "nested": { "b": ["x", "SECRET-B", 3] },
            "clean": "nothing",
        });
        r.sanitize(&mut v);
        let dump = v.to_string();
        assert!(!dump.contains("SECRET-A"));
        assert!(!dump.contains("SECRET-B"));
        assert!(dump.contains("[REDACTED]"));
        assert_eq!(v["clean"], "nothing");
    }

    #[test]
    fn sanitize_str_masks_and_empty_secret_ignored() {
        let r = Redactor::default();
        r.add("");
        r.add("tok-123");
        assert_eq!(r.sanitize_str("bearer tok-123!"), "bearer [REDACTED]!");
        // Empty registration is a no-op (never masks everything).
        assert_eq!(r.sanitize_str("plain"), "plain");
    }

    #[test]
    fn short_secrets_mask_exact_values_but_never_substrings() {
        let r = Redactor::default();
        r.add("demo");
        r.add("aaa");
        // 子串不误伤：凭据名照常展示（用户场景回归）
        assert_eq!(r.sanitize_str("demo-user"), "demo-user");
        let mut v = json!({ "names": ["demo-user", "demo-aaa"], "echo": "demo", "arr": ["aaa"] });
        r.sanitize(&mut v);
        assert_eq!(v["names"], json!(["demo-user", "demo-aaa"]));
        // 整值精确匹配仍遮蔽（值字段原样回流的典型泄漏形态）
        assert_eq!(v["echo"], "[REDACTED]");
        assert_eq!(v["arr"], json!(["[REDACTED]"]));
        assert_eq!(r.sanitize_str("aaa"), "[REDACTED]");
        // 达到下限的长秘密仍做子串替换
        r.add("demo66");
        assert_eq!(r.sanitize_str("x demo66 y"), "x [REDACTED] y");
    }

    #[test]
    fn no_secrets_is_noop() {
        let r = Redactor::default();
        let mut v = json!({ "a": "b" });
        r.sanitize(&mut v);
        assert_eq!(v, json!({ "a": "b" }));
    }
}
