use std::path::{Path, PathBuf};
use tokio::fs;
use std::io;

/// Returns the path to a thumbnail based on its hash and size
/// Structure: {thumbnail_dir}/{first_4_chars}/{next_2_chars}/{rest_of_hash}_{size}.png
pub fn get_thumbnail_path(thumbnail_dir: &Path, hash: &str, size: &str) -> PathBuf {
    if hash.len() < 6 {
        // Fallback for short hashes, though they shouldn't happen with hex-encoded hashes
        return thumbnail_dir.join(format!("{}_{}.png", hash, size));
    }
    
    let part1 = &hash[0..4];
    let part2 = &hash[4..6];
    let rest = &hash[6..];
    
    thumbnail_dir
        .join(part1)
        .join(part2)
        .join(format!("{}_{}.png", rest, size))
}

/// Ensures the directory for a thumbnail path exists
pub async fn ensure_thumbnail_dir(thumbnail_path: &Path) -> io::Result<()> {
    if let Some(parent) = thumbnail_path.parent() {
        fs::create_dir_all(parent).await?;
    }
    Ok(())
}
