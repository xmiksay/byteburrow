use std::io;
use std::path::{Path, PathBuf};
use tokio::fs;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_hash_is_sharded_into_two_level_dirs() {
        // {dir}/{first4}/{next2}/{rest}_{size}.png
        let dir = Path::new("/thumbs");
        // 16-char hash, first 4 and next 2 are stripped, rest + size in filename.
        let hash = "0123456789abcdef";
        let p = get_thumbnail_path(dir, hash, "small");
        assert_eq!(p, Path::new("/thumbs/0123/45/6789abcdef_small.png"));
    }

    #[test]
    fn long_hash_keeps_all_three_components_distinct() {
        let dir = Path::new("/t");
        let p = get_thumbnail_path(dir, "aabbccdd", "mini");
        assert_eq!(p, Path::new("/t/aabb/cc/dd_mini.png"));
    }

    #[test]
    fn short_hash_falls_back_to_flat_path() {
        // Hashes shorter than 6 chars can't shard; fall back to a flat name.
        let dir = Path::new("/thumbs");
        let p = get_thumbnail_path(dir, "abc", "large");
        assert_eq!(p, Path::new("/thumbs/abc_large.png"));
    }

    #[test]
    fn exactly_six_char_hash_uses_sharded_path() {
        // 6 chars: 4 + 2 + empty rest → filename is just "_{size}.png".
        let dir = Path::new("/thumbs");
        let p = get_thumbnail_path(dir, "abcdef", "small");
        assert_eq!(p, Path::new("/thumbs/abcd/ef/_small.png"));
    }
}
