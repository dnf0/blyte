//! Per-repository record of git trees that have passed a repository's check.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60;

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PassedRecord {
    tree: String,
    key: String,
    ts: u64,
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn check_key(gate: &str, env: &BTreeMap<String, String>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(&(gate.len() as u64).to_be_bytes());
    hasher.update(gate.as_bytes());
    hasher.update(&(env.len() as u64).to_be_bytes());
    for (k, v) in env {
        hasher.update(&(k.len() as u64).to_be_bytes());
        hasher.update(k.as_bytes());
        hasher.update(&(v.len() as u64).to_be_bytes());
        hasher.update(v.as_bytes());
    }
    hex_lower(&hasher.finalize())
}

pub fn passed_dir(root: &Path) -> PathBuf {
    let main = crate::config::main_checkout(root).unwrap_or_else(|| root.to_path_buf());
    main.join(".wt").join("passed")
}

pub fn record_path(root: &Path, tree: &str, key: &str) -> PathBuf {
    passed_dir(root).join(format!("{tree}.{key}.json"))
}

pub fn record_passed(root: &Path, tree: &str, key: &str) -> Result<(), String> {
    record_passed_at(root, tree, key, now_secs())
}

pub fn record_passed_in_main(
    main_checkout: &Path,
    common_dir: Option<&Path>,
    tree: &str,
    key: &str,
) -> Result<(), String> {
    record_passed_at_in_main(main_checkout, common_dir, tree, key, now_secs())
}

pub fn record_passed_at(root: &Path, tree: &str, key: &str, ts: u64) -> Result<(), String> {
    let main = crate::config::main_checkout(root).unwrap_or_else(|| root.to_path_buf());
    ensure_wt_excluded(root);
    record_passed_in_dir(&main.join(".wt").join("passed"), tree, key, ts)
}

fn record_passed_at_in_main(
    main_checkout: &Path,
    common_dir: Option<&Path>,
    tree: &str,
    key: &str,
    ts: u64,
) -> Result<(), String> {
    if let Some(cd) = common_dir {
        ensure_wt_excluded_at(cd);
    }
    record_passed_in_dir(&main_checkout.join(".wt").join("passed"), tree, key, ts)
}

fn record_passed_in_dir(dir: &Path, tree: &str, key: &str, ts: u64) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("failed to create {}: {e}", dir.display()))?;

    let final_path = dir.join(format!("{tree}.{key}.json"));
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_path = dir.join(format!(".{tree}.{key}.{}.{seq}.tmp", std::process::id()));

    let record = PassedRecord {
        tree: tree.to_string(),
        key: key.to_string(),
        ts,
    };
    let json = serde_json::to_string_pretty(&record)
        .map_err(|e| format!("failed to serialize passed-tree record: {e}"))?;

    if let Err(e) = std::fs::write(&tmp_path, format!("{json}\n")) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("failed to write {}: {e}", tmp_path.display()));
    }

    if let Err(e) = std::fs::rename(&tmp_path, &final_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!(
            "failed to rename {} to {}: {e}",
            tmp_path.display(),
            final_path.display()
        ));
    }

    Ok(())
}

pub fn has_passed(root: &Path, tree: &str, key: &str, now: u64) -> Result<bool, String> {
    let main = crate::config::main_checkout(root).unwrap_or_else(|| root.to_path_buf());
    has_passed_in_main(&main, tree, key, now)
}

pub fn has_passed_in_main(
    main_checkout: &Path,
    tree: &str,
    key: &str,
    now: u64,
) -> Result<bool, String> {
    let path = main_checkout
        .join(".wt")
        .join("passed")
        .join(format!("{tree}.{key}.json"));
    let raw = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => {
            return Err(format!(
                "failed to read passed-tree record {}: {e}",
                path.display()
            ));
        }
    };

    let record: PassedRecord = serde_json::from_str(&raw)
        .map_err(|e| format!("failed to parse passed-tree record {}: {e}", path.display()))?;

    if record.tree != tree || record.key != key {
        return Ok(false);
    }

    Ok(now.saturating_sub(record.ts) <= MAX_AGE_SECS)
}

pub fn tree_of(dir: &Path, rev: &str) -> Result<String, String> {
    let spec = format!("{rev}^{{tree}}");
    let out = Command::new("git")
        .args(["rev-parse", "--verify", &spec])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to execute git rev-parse {spec}: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        return Err(format!("git rev-parse {spec} failed: {detail}"));
    }
    let tree = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if tree.is_empty() {
        return Err(format!("git rev-parse {spec} returned empty output"));
    }
    Ok(tree)
}

pub fn worktree_is_clean(dir: &Path) -> Result<bool, String> {
    let out = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("failed to execute git status --porcelain: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        return Err(format!("git status --porcelain failed: {detail}"));
    }
    Ok(out.stdout.is_empty())
}

fn ensure_wt_excluded_at(common_dir: &Path) {
    let exclude_path = common_dir.join("info").join("exclude");
    if let Some(parent) = exclude_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let content = std::fs::read_to_string(&exclude_path).unwrap_or_default();
    if !content.lines().any(|l| l.trim() == ".wt/") {
        let mut next = content;
        if !next.is_empty() && !next.ends_with('\n') {
            next.push('\n');
        }
        next.push_str(".wt/\n");
        let _ = std::fs::write(&exclude_path, next);
    }
}

fn ensure_wt_excluded(root: &Path) {
    let Ok(out) = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(root)
        .output()
    else {
        return;
    };
    if !out.status.success() {
        return;
    }
    let common = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if common.is_empty() {
        return;
    }
    let common_path = PathBuf::from(common);
    let common_dir = if common_path.is_absolute() {
        common_path
    } else {
        root.join(common_path)
    };
    ensure_wt_excluded_at(&common_dir);
}

fn hex_lower(bytes: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    buffer_len: usize,
    total_len: u64,
}

impl Sha256 {
    fn new() -> Self {
        Self {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buffer: [0u8; 64],
            buffer_len: 0,
            total_len: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u64);
        if self.buffer_len > 0 {
            let take = (64 - self.buffer_len).min(data.len());
            self.buffer[self.buffer_len..self.buffer_len + take].copy_from_slice(&data[..take]);
            self.buffer_len += take;
            data = &data[take..];
            if self.buffer_len == 64 {
                let block = self.buffer;
                self.compress(&block);
                self.buffer_len = 0;
            }
        }
        while data.len() >= 64 {
            let block: [u8; 64] = data[..64].try_into().unwrap();
            self.compress(&block);
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.buffer_len = data.len();
        }
    }

    fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.total_len.wrapping_mul(8);
        self.update(&[0x80]);
        let pad_len = if self.buffer_len <= 56 {
            56 - self.buffer_len
        } else {
            120 - self.buffer_len
        };
        let zeros = [0u8; 64];
        let mut remaining = pad_len;
        while remaining > 0 {
            let chunk = remaining.min(64);
            self.update(&zeros[..chunk]);
            remaining -= chunk;
        }
        self.update(&bit_len.to_be_bytes());
        let mut digest = [0u8; 32];
        for (i, &word) in self.state.iter().enumerate() {
            digest[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        digest
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vectors() {
        let h_empty = Sha256::new();
        assert_eq!(
            hex_lower(&h_empty.finalize()),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        let mut h_abc = Sha256::new();
        h_abc.update(b"abc");
        assert_eq!(
            hex_lower(&h_abc.finalize()),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let mut h_long = Sha256::new();
        h_long.update(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq");
        assert_eq!(
            hex_lower(&h_long.finalize()),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }
}
