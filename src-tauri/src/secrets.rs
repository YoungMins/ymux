//! Backend-owned secret store: webhook URL / HMAC secret / LLM API key.
//!
//! One file, `<dir>/secrets.json`, mapping `(dest_id, field)` to a value.
//! On Windows the whole file is sealed with DPAPI (current-user scope); on
//! other platforms it is plaintext JSON created `0600`. Writes go through a
//! temp file + rename. Values are never returned over IPC (only [`status`]
//! booleans) and never appear in `Debug`.
//!
//! A file that cannot be read back (corrupt JSON, DPAPI failure) is treated
//! as empty; before the next write it is copied to `secrets.json.corrupt`
//! (older copies rotate to `.corrupt.1`, `.corrupt.2`; at most [`BACKUPS`]
//! are kept) so nothing is destroyed silently. A file that could not even be
//! *read* (an I/O error other than "not found") is never overwritten: every
//! write fails until it can be read.
//!
//! [`status`]: SecretStore::status

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::Serialize;

use crate::error::{YmuxError, YmuxResult};

/// Corrupt-file copies kept.
const BACKUPS: usize = 3;

/// The fields a destination may store.
pub const FIELDS: [&str; 3] = ["url", "secret", "api_key"];

type Map = BTreeMap<String, BTreeMap<String, String>>;

/// Which secret fields of a destination are set. No values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct SecretStatus {
    pub url: bool,
    pub secret: bool,
    pub api_key: bool,
}

pub struct SecretStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl fmt::Debug for SecretStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretStore")
            .field("path", &self.path)
            .field("values", &"<redacted>")
            .finish()
    }
}

fn check_field(field: &str) -> YmuxResult<()> {
    if FIELDS.contains(&field) {
        Ok(())
    } else {
        Err(YmuxError::Config(format!("unknown secret field: {field}")))
    }
}

impl SecretStore {
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join("secrets.json"),
            lock: Mutex::new(()),
        }
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `(map, corrupt)`. A missing file is empty and not corrupt; content
    /// that does not parse is empty and corrupt. `Err` means the file exists
    /// but could not be read at all, so nothing may be written over it.
    fn load_strict(&self) -> YmuxResult<(Map, bool)> {
        let bytes = match std::fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((Map::new(), false)),
            Err(e) => {
                tracing::warn!("secrets: cannot read {}: {e}", self.path.display());
                return Err(e.into());
            }
        };
        match imp::unseal(&bytes).and_then(|p| serde_json::from_slice::<Map>(&p).ok()) {
            Some(m) => Ok((m, false)),
            None => {
                tracing::warn!(
                    "secrets: {} is unreadable; treating as empty",
                    self.path.display()
                );
                Ok((Map::new(), true))
            }
        }
    }

    /// For readers: anything unreadable is empty.
    fn load(&self) -> (Map, bool) {
        self.load_strict().unwrap_or((Map::new(), true))
    }

    /// Path of the `n`th corrupt-file copy (0 = newest).
    fn backup_path(&self, n: usize) -> PathBuf {
        let mut name = self.path.file_name().unwrap_or_default().to_os_string();
        name.push(".corrupt");
        if n > 0 {
            name.push(format!(".{n}"));
        }
        self.path.with_file_name(name)
    }

    /// Keeps a copy of the unreadable file, rotating older copies. The same
    /// bytes are not copied twice (a failing save retried must not push the
    /// original out).
    fn back_up_corrupt(&self) {
        let Ok(current) = std::fs::read(&self.path) else {
            return;
        };
        if std::fs::read(self.backup_path(0)).is_ok_and(|b| b == current) {
            return;
        }
        let _ = std::fs::remove_file(self.backup_path(BACKUPS - 1));
        for n in (0..BACKUPS - 1).rev() {
            let from = self.backup_path(n);
            if from.exists() {
                let _ = std::fs::rename(&from, self.backup_path(n + 1));
            }
        }
        if let Err(e) = std::fs::write(self.backup_path(0), &current) {
            tracing::warn!("secrets: cannot back up corrupt file: {e}");
        }
    }

    fn save(&self, map: &Map, corrupt: bool) -> YmuxResult<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if corrupt {
            self.back_up_corrupt();
        }
        let plain = serde_json::to_vec(map)?;
        let sealed = imp::seal(&plain)?;
        let tmp = self.path.with_extension("json.tmp");
        imp::write_private(&tmp, &sealed)?;
        if let Err(e) = std::fs::rename(&tmp, &self.path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.into());
        }
        Ok(())
    }

    /// Store a value; an empty value clears the field.
    pub fn set(&self, id: &str, field: &str, value: &str) -> YmuxResult<()> {
        check_field(field)?;
        let _g = self.guard();
        let (mut map, corrupt) = self.load_strict()?;
        if value.is_empty() {
            if let Some(e) = map.get_mut(id) {
                e.remove(field);
                if e.is_empty() {
                    map.remove(id);
                }
            }
        } else {
            map.entry(id.to_string())
                .or_default()
                .insert(field.to_string(), value.to_string());
        }
        self.save(&map, corrupt)
    }

    /// Backend-internal read. Never hand the result to the frontend.
    pub fn get(&self, id: &str, field: &str) -> Option<String> {
        if check_field(field).is_err() {
            return None;
        }
        let _g = self.guard();
        self.load().0.get(id)?.get(field).cloned()
    }

    pub fn status(&self, id: &str) -> SecretStatus {
        let _g = self.guard();
        let (map, _) = self.load();
        let has = |f: &str| map.get(id).is_some_and(|e| e.contains_key(f));
        SecretStatus {
            url: has("url"),
            secret: has("secret"),
            api_key: has("api_key"),
        }
    }

    pub fn remove_dest(&self, id: &str) -> YmuxResult<()> {
        let _g = self.guard();
        let (mut map, corrupt) = self.load_strict()?;
        if map.remove(id).is_none() {
            return Ok(());
        }
        self.save(&map, corrupt)
    }

    /// Drop every destination not in `keep_ids`.
    pub fn prune(&self, keep_ids: &[String]) -> YmuxResult<()> {
        let _g = self.guard();
        let (mut map, corrupt) = self.load_strict()?;
        let before = map.len();
        map.retain(|k, _| keep_ids.iter().any(|i| i == k));
        if map.len() == before {
            return Ok(());
        }
        self.save(&map, corrupt)
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    /// Copies a DPAPI output blob; `None` for a null pointer or zero length
    /// (`from_raw_parts` on a null pointer is undefined behaviour).
    ///
    /// # Safety
    /// A non-null `pbData` must point to `cbData` readable bytes.
    unsafe fn blob_to_vec(b: &CRYPT_INTEGER_BLOB) -> Option<Vec<u8>> {
        if b.pbData.is_null() || b.cbData == 0 {
            return None;
        }
        Some(std::slice::from_raw_parts(b.pbData, b.cbData as usize).to_vec())
    }

    pub fn seal(plain: &[u8]) -> YmuxResult<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: plain.len() as u32,
            pbData: plain.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: `input` borrows `plain` for the call; `out` is filled by
        // DPAPI with a LocalAlloc buffer that we copy and free below.
        unsafe {
            CryptProtectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
            .map_err(|e| YmuxError::Config(format!("dpapi protect: {e}")))?;
            let v = blob_to_vec(&out);
            if !out.pbData.is_null() {
                let _ = LocalFree(HLOCAL(out.pbData as *mut _));
            }
            v.ok_or_else(|| YmuxError::Config("dpapi protect: empty result".into()))
        }
    }

    pub fn unseal(sealed: &[u8]) -> Option<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: sealed.len() as u32,
            pbData: sealed.as_ptr() as *mut u8,
        };
        let mut out = CRYPT_INTEGER_BLOB::default();
        // SAFETY: as in `seal`.
        unsafe {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
            .ok()?;
            let v = blob_to_vec(&out);
            if !out.pbData.is_null() {
                let _ = LocalFree(HLOCAL(out.pbData as *mut _));
            }
            v
        }
    }

    pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        // The config dir inherits a user-only ACL; the content is DPAPI-sealed anyway.
        // Flushed before the caller renames it into place, like the unix path.
        use std::io::Write;
        let mut f = std::fs::File::create(path)?;
        f.write_all(bytes)?;
        f.sync_all()
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn seal(plain: &[u8]) -> YmuxResult<Vec<u8>> {
        Ok(plain.to_vec())
    }

    pub fn unseal(sealed: &[u8]) -> Option<Vec<u8>> {
        Some(sealed.to_vec())
    }

    pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let _ = std::fs::remove_file(path);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        // `mode` is masked by umask; make 0600 exact.
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        f.write_all(bytes)?;
        f.sync_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, SecretStore) {
        let d = tempfile::tempdir().unwrap();
        let s = SecretStore::new(d.path());
        (d, s)
    }

    #[test]
    fn set_get_status_remove() {
        let (_d, s) = store();
        assert_eq!(s.status("a"), SecretStatus::default());
        s.set("a", "url", "https://x/y").unwrap();
        s.set("a", "secret", "s3").unwrap();
        s.set("b", "api_key", "k").unwrap();
        assert_eq!(s.get("a", "url").as_deref(), Some("https://x/y"));
        assert_eq!(s.get("a", "api_key"), None);
        assert_eq!(
            s.status("a"),
            SecretStatus {
                url: true,
                secret: true,
                api_key: false
            }
        );
        s.set("a", "url", "").unwrap();
        assert!(!s.status("a").url);
        s.remove_dest("a").unwrap();
        assert_eq!(s.status("a"), SecretStatus::default());
        assert_eq!(s.get("b", "api_key").as_deref(), Some("k"));
    }

    #[test]
    fn persists_across_instances_and_prunes() {
        let (d, s) = store();
        s.set("a", "url", "u").unwrap();
        s.set("b", "url", "v").unwrap();
        let s2 = SecretStore::new(d.path());
        assert_eq!(s2.get("a", "url").as_deref(), Some("u"));
        s2.prune(&["b".to_string()]).unwrap();
        assert_eq!(s2.get("a", "url"), None);
        assert_eq!(s2.get("b", "url").as_deref(), Some("v"));
    }

    #[test]
    fn bad_field_rejected() {
        let (_d, s) = store();
        assert!(s.set("a", "password", "x").is_err());
        assert!(s.set("a", "", "x").is_err());
        assert_eq!(s.get("a", "../x"), None);
    }

    #[test]
    fn corrupt_file_is_empty_and_backed_up_once() {
        let (d, s) = store();
        let p = d.path().join("secrets.json");
        std::fs::write(&p, b"\x00not json or dpapi").unwrap();
        assert_eq!(s.get("a", "url"), None);
        assert_eq!(s.status("a"), SecretStatus::default());
        s.set("a", "url", "u").unwrap();
        let bak = d.path().join("secrets.json.corrupt");
        assert_eq!(std::fs::read(&bak).unwrap(), b"\x00not json or dpapi");
        assert_eq!(s.get("a", "url").as_deref(), Some("u"));
        // A second corruption rotates; the first copy is kept as `.1`.
        std::fs::write(&p, b"other junk").unwrap();
        s.set("a", "url", "w").unwrap();
        assert_eq!(std::fs::read(&bak).unwrap(), b"other junk");
        assert_eq!(
            std::fs::read(d.path().join("secrets.json.corrupt.1")).unwrap(),
            b"\x00not json or dpapi"
        );
    }

    #[test]
    fn corrupt_backups_rotate_and_keep_at_most_three() {
        let (d, s) = store();
        let p = d.path().join("secrets.json");
        for i in 0..5 {
            std::fs::write(&p, format!("junk {i}")).unwrap();
            s.set("a", "url", "u").unwrap();
        }
        let read = |n: &str| std::fs::read_to_string(d.path().join(n)).unwrap();
        assert_eq!(read("secrets.json.corrupt"), "junk 4");
        assert_eq!(read("secrets.json.corrupt.1"), "junk 3");
        assert_eq!(read("secrets.json.corrupt.2"), "junk 2");
        assert!(!d.path().join("secrets.json.corrupt.3").exists());
        // The same corrupt bytes are not copied twice.
        std::fs::write(&p, "junk 4").unwrap();
        s.set("a", "url", "u").unwrap();
        assert_eq!(read("secrets.json.corrupt.1"), "junk 3");
    }

    #[test]
    fn an_unreadable_file_is_never_overwritten() {
        let (d, s) = store();
        // A directory where the file should be: reading it fails with an
        // error that is not "not found".
        let p = d.path().join("secrets.json");
        std::fs::create_dir(&p).unwrap();
        assert!(s.set("a", "url", "u").is_err());
        assert!(s.remove_dest("a").is_err());
        assert!(s.prune(&[]).is_err());
        assert!(p.is_dir(), "left alone");
        assert!(!d.path().join("secrets.json.tmp").exists());
        assert_eq!(s.get("a", "url"), None);
    }

    #[test]
    fn debug_hides_values() {
        let (_d, s) = store();
        s.set("a", "secret", "TOPSECRETVALUE").unwrap();
        assert!(!format!("{s:?}").contains("TOPSECRETVALUE"));
    }

    #[cfg(unix)]
    #[test]
    fn file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let (d, s) = store();
        s.set("a", "url", "u").unwrap();
        let m = std::fs::metadata(d.path().join("secrets.json")).unwrap();
        assert_eq!(m.permissions().mode() & 0o777, 0o600);
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_round_trip_and_not_plaintext() {
        let (d, s) = store();
        s.set("a", "secret", "PLAINTEXTMARKER").unwrap();
        let raw = std::fs::read(d.path().join("secrets.json")).unwrap();
        assert!(!raw.windows(15).any(|w| w == b"PLAINTEXTMARKER"));
        assert_eq!(s.get("a", "secret").as_deref(), Some("PLAINTEXTMARKER"));
    }
}
