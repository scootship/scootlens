//! State VFS：vault（加密、Agent 只写不读）、downloads、uploads 沙箱
//! （docs/04-kernel-design.md §4.4、docs/06-security-model.md T3）。
//!
//! 目录布局：
//!
//! ```text
//! <state_dir>/
//!   keys/signing.key      # 0600，SecurityManager
//!   journal.jsonl
//!   vault/vault.key       # 0600，ChaCha20-Poly1305 密钥
//!   vault/vault.enc       # 加密 blob：JSON {name: secret}
//!   downloads/            # 引擎下载落盘
//!   uploads/              # act.upload 允许引用的唯一来源
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use chacha20poly1305::aead::{Aead, AeadCore, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use scootlens_abi::{AbiError, ErrorCode};
use serde::{Deserialize, Serialize};

/// State VFS。`root = None` → vault 内存模式、downloads/uploads 不可用。
pub struct StateVfs {
    root: Option<PathBuf>,
    vault: Mutex<VaultStore>,
}

impl StateVfs {
    pub fn open(root: Option<&Path>) -> std::io::Result<Self> {
        let vault = match root {
            Some(dir) => {
                std::fs::create_dir_all(dir.join("downloads"))?;
                std::fs::create_dir_all(dir.join("uploads"))?;
                VaultStore::open(&dir.join("vault"))?
            }
            None => VaultStore::in_memory(),
        };
        Ok(Self {
            root: root.map(Path::to_path_buf),
            vault: Mutex::new(vault),
        })
    }

    // ---------- vault（Agent 只写不读） ----------

    pub fn vault_write(&self, name: &str, secret: &str) -> Result<(), AbiError> {
        self.lock_vault()
            .put(name, secret)
            .map_err(|e| AbiError::new(ErrorCode::Internal, format!("vault write failed: {e}")))
    }

    /// 仅列出名字，永不返回值。
    pub fn vault_names(&self) -> Vec<String> {
        self.lock_vault().names()
    }

    /// 内核内部解引用（`act.type` vault_ref）。**绝不进入任何 syscall 返回值**。
    pub(crate) fn vault_resolve(&self, name: &str) -> Option<String> {
        self.lock_vault().get(name)
    }

    // ---------- downloads / uploads ----------

    pub fn downloads_dir(&self) -> Option<PathBuf> {
        self.root.as_ref().map(|r| r.join("downloads"))
    }

    /// 列出命名空间目录内的文件名。
    pub fn list_files(&self, ns: &str) -> Result<Vec<String>, AbiError> {
        let root = self.root.as_ref().ok_or_else(no_state_dir)?;
        let dir = root.join(ns);
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .map_err(|e| AbiError::new(ErrorCode::Internal, format!("read {ns}: {e}")))?
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        Ok(names)
    }

    /// 把 `uploads/` 内的相对路径解析为绝对路径；越界/缺失 → `E_INVALID_ARG`。
    pub fn resolve_upload(&self, rel: &str) -> Result<PathBuf, AbiError> {
        let root = self.root.as_ref().ok_or_else(no_state_dir)?;
        let uploads = root
            .join("uploads")
            .canonicalize()
            .map_err(|e| AbiError::new(ErrorCode::Internal, format!("uploads dir: {e}")))?;
        let joined = uploads.join(rel);
        let resolved = joined.canonicalize().map_err(|_| {
            AbiError::new(ErrorCode::InvalidArg, format!("no such upload file: {rel}"))
        })?;
        if !resolved.starts_with(&uploads) {
            return Err(AbiError::new(
                ErrorCode::InvalidArg,
                "upload path escapes sandbox",
            ));
        }
        Ok(resolved)
    }

    fn lock_vault(&self) -> std::sync::MutexGuard<'_, VaultStore> {
        self.vault.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn no_state_dir() -> AbiError {
    AbiError::new(
        ErrorCode::Unsupported,
        "kernel is running without a state dir",
    )
}

// ---------- vault 后端 ----------

#[derive(Default, Serialize, Deserialize)]
struct VaultData {
    entries: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
struct VaultBlob {
    nonce: String,
    ct: String,
}

struct VaultStore {
    dir: Option<PathBuf>,
    cipher: Option<ChaCha20Poly1305>,
    data: VaultData,
}

impl VaultStore {
    fn in_memory() -> Self {
        Self {
            dir: None,
            cipher: None,
            data: VaultData::default(),
        }
    }

    fn open(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let key_path = dir.join("vault.key");
        let key_bytes: [u8; 32] = if key_path.exists() {
            let hex_key = std::fs::read_to_string(&key_path)?;
            hex::decode(hex_key.trim())
                .ok()
                .and_then(|v| v.try_into().ok())
                .ok_or_else(|| std::io::Error::other("vault.key must be 32 bytes hex"))?
        } else {
            let key = ChaCha20Poly1305::generate_key(&mut chacha20poly1305::aead::OsRng);
            std::fs::write(&key_path, hex::encode(key))?;
            restrict_permissions(&key_path)?;
            key.into()
        };
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&key_bytes));
        let blob_path = dir.join("vault.enc");
        let data = if blob_path.exists() {
            let text = std::fs::read_to_string(&blob_path)?;
            decrypt(&cipher, &text).map_err(std::io::Error::other)?
        } else {
            VaultData::default()
        };
        Ok(Self {
            dir: Some(dir.to_path_buf()),
            cipher: Some(cipher),
            data,
        })
    }

    fn put(&mut self, name: &str, secret: &str) -> Result<(), String> {
        self.data.entries.insert(name.to_owned(), secret.to_owned());
        self.persist()
    }

    fn get(&self, name: &str) -> Option<String> {
        self.data.entries.get(name).cloned()
    }

    fn names(&self) -> Vec<String> {
        self.data.entries.keys().cloned().collect()
    }

    fn persist(&self) -> Result<(), String> {
        let (Some(dir), Some(cipher)) = (&self.dir, &self.cipher) else {
            return Ok(());
        };
        let plain = serde_json::to_vec(&self.data).map_err(|e| e.to_string())?;
        let nonce = ChaCha20Poly1305::generate_nonce(&mut chacha20poly1305::aead::OsRng);
        let ct = cipher
            .encrypt(&nonce, plain.as_slice())
            .map_err(|e| e.to_string())?;
        let blob = VaultBlob {
            nonce: hex::encode(nonce),
            ct: hex::encode(ct),
        };
        let text = serde_json::to_string(&blob).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("vault.enc"), text).map_err(|e| e.to_string())
    }
}

fn decrypt(cipher: &ChaCha20Poly1305, text: &str) -> Result<VaultData, String> {
    let blob: VaultBlob = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let nonce_bytes = hex::decode(&blob.nonce).map_err(|e| e.to_string())?;
    let ct = hex::decode(&blob.ct).map_err(|e| e.to_string())?;
    let plain = cipher
        .decrypt(Nonce::from_slice(&nonce_bytes), ct.as_slice())
        .map_err(|_| "vault decrypt failed (wrong key or corrupt blob)".to_owned())?;
    serde_json::from_slice(&plain).map_err(|e| e.to_string())
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vault_encrypts_at_rest_and_persists_across_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let secret = "p@ss-DEADBEEF-1234";
        {
            let vfs = StateVfs::open(Some(dir.path())).expect("open");
            vfs.vault_write("pw", secret).expect("write");
            assert_eq!(
                vfs.vault_resolve("pw").as_deref(),
                Some(secret),
                "roundtrip"
            );
            assert_eq!(vfs.vault_names(), vec!["pw".to_string()]);
        }
        // Ciphertext at rest must not contain the plaintext secret.
        let blob = std::fs::read(dir.path().join("vault/vault.enc")).expect("blob");
        assert!(
            !String::from_utf8_lossy(&blob).contains(secret),
            "plaintext secret found in vault blob"
        );
        // A fresh handle on the same dir decrypts the persisted value.
        let vfs2 = StateVfs::open(Some(dir.path())).expect("reopen");
        assert_eq!(vfs2.vault_resolve("pw").as_deref(), Some(secret));
    }

    #[test]
    fn memory_vault_roundtrips_without_state_dir() {
        let vfs = StateVfs::open(None).expect("open");
        vfs.vault_write("k", "v").expect("write");
        assert_eq!(vfs.vault_resolve("k").as_deref(), Some("v"));
    }

    #[test]
    fn upload_path_traversal_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vfs = StateVfs::open(Some(dir.path())).expect("open");
        std::fs::write(dir.path().join("uploads/ok.txt"), b"hi").expect("seed");
        assert!(
            vfs.resolve_upload("ok.txt").is_ok(),
            "in-sandbox file resolves"
        );
        // Escapes and non-existent files must fail.
        assert!(vfs.resolve_upload("../../etc/passwd").is_err());
        assert!(vfs.resolve_upload("nope.txt").is_err());
    }

    #[test]
    fn file_features_unsupported_without_state_dir() {
        let vfs = StateVfs::open(None).expect("open");
        match vfs.resolve_upload("x") {
            Err(e) => assert_eq!(e.code, ErrorCode::Unsupported),
            Ok(_) => panic!("must be unsupported without state dir"),
        }
        assert!(vfs.downloads_dir().is_none());
    }
}
