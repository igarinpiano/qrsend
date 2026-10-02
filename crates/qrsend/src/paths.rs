use std::path::PathBuf;

fn base(env: &str, fallback: Option<PathBuf>) -> PathBuf {
    std::env::var_os(env)
        .map(PathBuf::from)
        .or_else(|| fallback.map(|p| p.join("qrsend")))
        .unwrap_or_else(|| std::env::temp_dir().join("qrsend"))
}

/// Persistent receiver state (inbox).
pub fn data_dir() -> PathBuf {
    base("QRSEND_DATA_DIR", dirs::data_local_dir())
}

/// Sender spools (re-creatable, so they live in the cache).
pub fn cache_dir() -> PathBuf {
    base("QRSEND_CACHE_DIR", dirs::cache_dir())
}

pub fn sessions_dir() -> PathBuf {
    data_dir().join("sessions")
}

pub fn spools_dir() -> PathBuf {
    cache_dir().join("send")
}
