use std::path::Path;

use crate::shell;

pub const MIN_SECRET_BYTES: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redactor {
    pairs: Vec<(String, String)>,
}

enum Piece<'a> {
    Raw(&'a str),
    Redacted(&'a str),
}

impl Redactor {
    pub fn from_pairs(pairs: Vec<(String, String)>) -> Redactor {
        let mut filtered: Vec<(String, String)> = pairs
            .into_iter()
            .filter(|(_, value)| value.len() >= MIN_SECRET_BYTES)
            .collect();
        filtered.sort_by(|a, b| {
            b.1.len()
                .cmp(&a.1.len())
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.0.cmp(&b.0))
        });
        filtered.dedup_by(|a, b| a.1 == b.1);
        Redactor { pairs: filtered }
    }

    pub fn for_root(root: &Path) -> Result<Redactor, String> {
        let mut pairs = Vec::new();
        for &name in shell::HARNESS_ENV {
            if let Some(value) = std::env::var_os(name) {
                pairs.push((name.to_string(), value.to_string_lossy().into_owned()));
            }
        }
        for name in shell::withheld_env() {
            if let Some(value) = std::env::var_os(&name) {
                pairs.push((name, value.to_string_lossy().into_owned()));
            }
        }
        let env_path = root.join(".env");
        match std::fs::read_to_string(&env_path) {
            Ok(text) => {
                for (name, value) in parse_env_file(&text) {
                    if shell::credential_like(&name, &value) {
                        pairs.push((name, value));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if std::fs::symlink_metadata(&env_path).is_ok() {
                    return Err(format!("cannot read {}: {e}", env_path.display()));
                }
            }
            Err(e) => {
                return Err(format!("cannot read {}: {e}", env_path.display()));
            }
        }
        Ok(Redactor::from_pairs(pairs))
    }

    pub fn apply(&self, text: &str) -> String {
        if self.pairs.is_empty() || text.is_empty() {
            return text.to_string();
        }
        let mut pieces = vec![Piece::Raw(text)];
        for (name, value) in &self.pairs {
            if !pieces
                .iter()
                .any(|p| matches!(p, Piece::Raw(s) if s.contains(value.as_str())))
            {
                continue;
            }
            let mut next = Vec::with_capacity(pieces.len() + 2);
            for piece in pieces {
                match piece {
                    Piece::Redacted(n) => next.push(Piece::Redacted(n)),
                    Piece::Raw(s) => {
                        if !s.contains(value.as_str()) {
                            next.push(Piece::Raw(s));
                            continue;
                        }
                        let mut first = true;
                        for part in s.split(value.as_str()) {
                            if !first {
                                next.push(Piece::Redacted(name.as_str()));
                            }
                            first = false;
                            if !part.is_empty() {
                                next.push(Piece::Raw(part));
                            }
                        }
                    }
                }
            }
            pieces = next;
        }
        let mut out = String::with_capacity(text.len());
        for piece in pieces {
            match piece {
                Piece::Raw(s) => out.push_str(s),
                Piece::Redacted(name) => {
                    out.push_str("[redacted ");
                    out.push_str(name);
                    out.push(']');
                }
            }
        }
        out
    }
}

pub fn parse_env_file(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = if let Some(rest) = line.strip_prefix("export ") {
            rest.trim_start()
        } else if let Some(rest) = line.strip_prefix("export\t") {
            rest.trim_start()
        } else {
            line
        };
        let Some((raw_name, raw_val)) = line.split_once('=') else {
            continue;
        };
        let name = raw_name.trim();
        if name.is_empty() {
            continue;
        }
        let val = raw_val.trim();
        let unquoted = if val.len() >= 2
            && ((val.starts_with('"') && val.ends_with('"'))
                || (val.starts_with('\'') && val.ends_with('\'')))
        {
            &val[1..val.len() - 1]
        } else {
            val
        };
        pairs.push((name.to_string(), unquoted.to_string()));
    }
    pairs
}
