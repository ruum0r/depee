//! Location of depee's on-disk cache (registry documents and update checks).

use std::{fs, path::PathBuf};

use anyhow::{anyhow, Context, Result};

/// `~/.cache/depee` on macOS, the platform cache directory elsewhere.
pub fn dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let base = dirs::home_dir().map(|home| home.join(".cache"));
    #[cfg(not(target_os = "macos"))]
    let base = dirs::cache_dir();
    base.map(|base| base.join("depee"))
}

pub fn clear() -> Result<()> {
    let path = dir().ok_or_else(|| anyhow!("Could not determine the platform cache directory"))?;
    match fs::remove_dir_all(&path) {
        Ok(()) => println!("Cleared depee cache at {}", path.display()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!("No depee cache found at {}", path.display());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("Could not clear cache at {}", path.display()))
        }
    }
    Ok(())
}
