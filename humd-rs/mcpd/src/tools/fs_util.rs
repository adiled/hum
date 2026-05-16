//! Shared filesystem helpers: sandbox check, line counting, image
//! detection.

use crate::session::SessionState;
use std::path::{Path, PathBuf};

/// Extra dirs always permitted on top of the session cwd. Mirrors
/// EXTRA_ALLOWED in mcp/tools.ts.
const EXTRA_ALLOWED: &[&str] = &["/tmp"];

/// Resolve `p` to an absolute path and confirm it sits under the
/// session cwd or one of the extra-allowed dirs. Returns a friendly
/// error string when out of bounds — passed straight to the agent.
pub fn assert_path(p: &str, session: &SessionState) -> Result<PathBuf, String> {
    let resolved: PathBuf = if Path::new(p).is_absolute() {
        PathBuf::from(p)
    } else {
        session.cwd.join(p)
    };
    let resolved = normalize(&resolved);
    let cwd = normalize(&session.cwd);

    let allowed = std::iter::once(cwd.as_path())
        .chain(EXTRA_ALLOWED.iter().map(Path::new))
        .any(|root| resolved == root || resolved.starts_with(root));

    if allowed {
        Ok(resolved)
    } else {
        Err(format!(
            "Path {} is outside allowed directories",
            resolved.display()
        ))
    }
}

/// Path normalization without touching the filesystem — `Path::canonicalize`
/// fails on non-existent paths, but we want the sandbox check to also work
/// for Write's create-mode (file not yet there).
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in p.components() {
        use std::path::Component;
        match component {
            Component::ParentDir => { out.pop(); }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// True when the file extension suggests image content. Matches the
/// IMAGE_EXTS set in mcp/tools.ts.
pub fn is_image(path: &Path) -> bool {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "ico" | "svg"
    )
}
