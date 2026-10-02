//! Receiver-side path sanitisation. See docs/PROTOCOL.md §7.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    #[error("empty path")]
    Empty,
    #[error("absolute path {0:?}")]
    Absolute(String),
    #[error("path {0:?} contains a '.' or '..' component")]
    Traversal(String),
    #[error("path {0:?} contains NUL")]
    Nul(String),
    #[error("path component too long in {0:?}")]
    TooLong(String),
}

/// A relative path made only of safe components.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SafePath(Vec<String>);

const RESERVED: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];

fn is_reserved(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        return true;
    }
    let b = stem.as_bytes();
    b.len() == 4
        && (stem.starts_with("COM") || stem.starts_with("LPT"))
        && (b'1'..=b'9').contains(&b[3])
}

fn clean_component(raw: &str) -> String {
    let replaced: String = raw
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\\' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = replaced.trim_end_matches(['.', ' ']);
    let mut out = if trimmed.is_empty() {
        "_".to_string()
    } else {
        trimmed.to_string()
    };
    if is_reserved(&out) {
        out.insert(0, '_');
    }
    out
}

impl SafePath {
    pub fn parse(raw: &str) -> Result<SafePath, PathError> {
        if raw.is_empty() {
            return Err(PathError::Empty);
        }
        if raw.contains('\0') {
            return Err(PathError::Nul(raw.into()));
        }
        let b = raw.as_bytes();
        let drive = b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
        if raw.starts_with('/') || raw.starts_with('\\') || drive {
            return Err(PathError::Absolute(raw.into()));
        }
        let mut parts = Vec::new();
        for comp in raw.split('/') {
            if comp.is_empty() {
                continue; // tolerate "a//b" and trailing slashes
            }
            if comp == "." || comp == ".." || comp.split('\\').any(|c| c == "..") {
                return Err(PathError::Traversal(raw.into()));
            }
            let cleaned = clean_component(comp);
            if cleaned.len() > 255 {
                return Err(PathError::TooLong(raw.into()));
            }
            parts.push(cleaned);
        }
        if parts.is_empty() {
            return Err(PathError::Empty);
        }
        Ok(SafePath(parts))
    }

    pub fn components(&self) -> &[String] {
        &self.0
    }

    pub fn file_name(&self) -> &str {
        self.0.last().expect("SafePath is never empty")
    }

    pub fn parent(&self) -> Option<SafePath> {
        (self.0.len() > 1).then(|| SafePath(self.0[..self.0.len() - 1].to_vec()))
    }

    /// Same path with the last component replaced.
    pub fn with_file_name(&self, name: String) -> SafePath {
        let mut parts = self.0.clone();
        *parts.last_mut().unwrap() = name;
        SafePath(parts)
    }

    pub fn join_to(&self, base: &Path) -> PathBuf {
        let mut p = base.to_path_buf();
        p.extend(&self.0);
        p
    }

    /// Key for detecting collisions on case-insensitive file systems.
    pub fn fold_key(&self) -> String {
        self.0.join("/").to_lowercase()
    }
}

impl std::fmt::Display for SafePath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.join("/"))
    }
}

/// `name` with ` (n)` inserted before the extension: `a.txt` → `a (1).txt`.
pub fn numbered_name(name: &str, n: u32) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} ({n}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} ({n})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(raw: &str) -> String {
        SafePath::parse(raw).unwrap().to_string()
    }

    #[test]
    fn accepts_normal_paths() {
        assert_eq!(ok("a/b/c.txt"), "a/b/c.txt");
        assert_eq!(ok("dir//file/"), "dir/file");
        assert_eq!(ok("日本語/ファイル.md"), "日本語/ファイル.md");
    }

    #[test]
    fn rejects_dangerous_paths() {
        for p in [
            "",
            "/etc/passwd",
            "\\x",
            "C:/x",
            "../x",
            "a/../../x",
            "a/./b",
            "a\0b",
            "a\\..\\b",
        ] {
            assert!(SafePath::parse(p).is_err(), "{p:?} should be rejected");
        }
        assert!(SafePath::parse(&"x".repeat(256)).is_err());
    }

    #[test]
    fn neutralises_hostile_names() {
        assert_eq!(ok("ab:c*d?.txt"), "ab_c_d_.txt");
        assert_eq!(ok("trailing. ./x"), "trailing/x");
        assert_eq!(ok("CON"), "_CON");
        assert_eq!(ok("dir/lpt1.txt"), "dir/_lpt1.txt");
        assert_eq!(ok("COM10"), "COM10");
        assert_eq!(ok("a\\b"), "a_b");
    }

    #[test]
    fn numbering() {
        assert_eq!(numbered_name("a.txt", 1), "a (1).txt");
        assert_eq!(numbered_name(".bashrc", 2), ".bashrc (2)");
        assert_eq!(numbered_name("Makefile", 3), "Makefile (3)");
    }
}
