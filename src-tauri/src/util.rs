use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Resolve to an absolute path. On Windows, strips the `\\?\` verbatim prefix so Git and
/// other CLI tools accept the path.
pub fn absolute_path(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(path)
        }
    });
    strip_verbatim_prefix(abs)
}

/// Format a path for passing as a Git CLI argument (no `\\?\` prefix on Windows).
pub fn path_arg(path: &Path) -> String {
    strip_verbatim_prefix(path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    let normalized = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        return path;
    };
    PathBuf::from(normalized)
}
