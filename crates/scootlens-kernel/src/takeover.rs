//! 人工接管（takeover，docs/07-web-console.md §关键交互）。
//!
//! Console 持有 `act:takeover` 后对单个 proc 独占输入：接管期间其他主体的
//! `act.*` 调用**挂起等待**（不丢弃、不拒绝），归还控制后按序恢复执行；
//! 等待超过上限返回 `E_TIMEOUT`。进程终止时自动清除并唤醒等待者。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use scootlens_abi::{AbiError, ErrorCode, Pid};
use tokio::sync::Notify;

struct Entry {
    holder: String,
    released: Arc<Notify>,
}

/// 接管表（per-pid 至多一个 holder）。
#[derive(Default)]
pub(crate) struct TakeoverTable {
    inner: Mutex<HashMap<Pid, Entry>>,
}

impl TakeoverTable {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Pid, Entry>> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 开始接管。幂等（同 holder 重复 start 返回 `false` = 无状态变化）；
    /// 已被他人持有 → `E_INVALID_ARG`。
    pub fn start(&self, pid: &Pid, subject: &str) -> Result<bool, AbiError> {
        let mut map = self.lock();
        match map.get(pid) {
            Some(e) if e.holder == subject => Ok(false),
            Some(e) => Err(AbiError::new(
                ErrorCode::InvalidArg,
                format!("proc {pid} already taken over by {}", e.holder),
            )),
            None => {
                map.insert(
                    pid.clone(),
                    Entry {
                        holder: subject.to_owned(),
                        released: Arc::new(Notify::new()),
                    },
                );
                Ok(true)
            }
        }
    }

    /// 结束接管：仅 holder 本人可归还；唤醒全部等待中的输入调用。
    pub fn end(&self, pid: &Pid, subject: &str) -> Result<(), AbiError> {
        let mut map = self.lock();
        match map.get(pid) {
            None => Err(AbiError::new(
                ErrorCode::InvalidArg,
                format!("proc {pid} is not taken over"),
            )),
            Some(e) if e.holder != subject => Err(AbiError::new(
                ErrorCode::CapDenied,
                format!("takeover of {pid} is held by {}", e.holder),
            )),
            Some(_) => {
                if let Some(e) = map.remove(pid) {
                    e.released.notify_waiters();
                }
                Ok(())
            }
        }
    }

    /// 进程终止清理：无 holder 校验；返回此前的 holder（若在接管中）。
    pub fn clear(&self, pid: &Pid) -> Option<String> {
        let mut map = self.lock();
        map.remove(pid).map(|e| {
            e.released.notify_waiters();
            e.holder
        })
    }

    /// 当前 holder。
    pub fn holder(&self, pid: &Pid) -> Option<String> {
        self.lock().get(pid).map(|e| e.holder.clone())
    }

    /// 输入门：holder 本人（或无接管）直接放行；其他主体挂起等待，
    /// 直到接管结束或超时（`E_TIMEOUT`）。唤醒后重新检查（可能被再次接管）。
    pub async fn gate(
        &self,
        pid: &Pid,
        subject: &str,
        hold_timeout: Duration,
    ) -> Result<(), AbiError> {
        let deadline = tokio::time::Instant::now() + hold_timeout;
        loop {
            let released = {
                let map = self.lock();
                match map.get(pid) {
                    None => return Ok(()),
                    Some(e) if e.holder == subject => return Ok(()),
                    Some(e) => Arc::clone(&e.released),
                }
            };
            let notified = released.notified();
            tokio::pin!(notified);
            // 先登记唤醒兴趣，再复查条件——避免 release 与 await 之间的竞态漏唤醒
            notified.as_mut().enable();
            {
                let map = self.lock();
                match map.get(pid) {
                    None => return Ok(()),
                    Some(e) if e.holder == subject => return Ok(()),
                    Some(_) => {}
                }
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                let holder = self.holder(pid).unwrap_or_else(|| "unknown".into());
                return Err(AbiError::new(
                    ErrorCode::Timeout,
                    format!("input to {pid} is held by takeover ({holder})"),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid() -> Pid {
        "p-t1".parse().expect("pid")
    }

    #[tokio::test]
    async fn start_is_idempotent_and_exclusive() {
        let t = TakeoverTable::default();
        assert!(t.start(&pid(), "user:a").expect("start"));
        assert!(!t.start(&pid(), "user:a").expect("re-start"), "idempotent");
        let err = t.start(&pid(), "user:b").expect_err("conflict");
        assert_eq!(err.code, ErrorCode::InvalidArg);
        assert_eq!(t.holder(&pid()).as_deref(), Some("user:a"));
    }

    #[tokio::test]
    async fn end_requires_holder() {
        let t = TakeoverTable::default();
        t.start(&pid(), "user:a").expect("start");
        let err = t.end(&pid(), "user:b").expect_err("non-holder");
        assert_eq!(err.code, ErrorCode::CapDenied);
        t.end(&pid(), "user:a").expect("holder ends");
        let err = t.end(&pid(), "user:a").expect_err("already ended");
        assert_eq!(err.code, ErrorCode::InvalidArg);
    }

    #[tokio::test]
    async fn gate_waits_until_release() {
        let t = Arc::new(TakeoverTable::default());
        t.start(&pid(), "user:a").expect("start");
        // holder 自身直接放行
        t.gate(&pid(), "user:a", Duration::from_millis(10))
            .await
            .expect("holder passes");
        let waiter = {
            let t = Arc::clone(&t);
            tokio::spawn(async move { t.gate(&pid(), "agent:x", Duration::from_secs(5)).await })
        };
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!waiter.is_finished(), "gate must hold while taken over");
        t.end(&pid(), "user:a").expect("end");
        waiter
            .await
            .expect("join")
            .expect("held call resumes after release");
    }

    #[tokio::test]
    async fn gate_times_out() {
        let t = TakeoverTable::default();
        t.start(&pid(), "user:a").expect("start");
        let err = t
            .gate(&pid(), "agent:x", Duration::from_millis(20))
            .await
            .expect_err("timeout");
        assert_eq!(err.code, ErrorCode::Timeout);
    }

    #[tokio::test]
    async fn clear_wakes_waiters() {
        let t = Arc::new(TakeoverTable::default());
        t.start(&pid(), "user:a").expect("start");
        let waiter = {
            let t = Arc::clone(&t);
            tokio::spawn(async move { t.gate(&pid(), "agent:x", Duration::from_secs(5)).await })
        };
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert_eq!(t.clear(&pid()).as_deref(), Some("user:a"));
        waiter.await.expect("join").expect("cleared gate opens");
        assert_eq!(t.clear(&pid()), None, "clear is idempotent");
    }
}
