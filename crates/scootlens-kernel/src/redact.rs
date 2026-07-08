//! 出口消毒：vault 凭据一经注入，其明文值不得出现在任何 syscall
//! 返回值、journal、trace 中（docs/06-security-model.md T3）。

use std::sync::{Mutex, PoisonError};

use serde_json::Value;

const MASK: &str = "[REDACTED]";

/// 秘密值登记表 + 递归替换。
#[derive(Default)]
pub struct Redactor {
    secrets: Mutex<Vec<String>>,
}

impl Redactor {
    /// 登记一个已注入的秘密值。
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
            if out.contains(sec.as_str()) {
                out = out.replace(sec.as_str(), MASK);
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
                if s.contains(sec.as_str()) {
                    *s = s.replace(sec.as_str(), MASK);
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
    fn no_secrets_is_noop() {
        let r = Redactor::default();
        let mut v = json!({ "a": "b" });
        r.sanitize(&mut v);
        assert_eq!(v, json!({ "a": "b" }));
    }
}
