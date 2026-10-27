//! A size-limited disk cache keyed by URL. Reads update the file time, and eviction removes
//! the files with the oldest time first.
//!
//! The cache only makes loads faster, so I/O errors count as a miss instead of a failure.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::file::write_atomically;

/// Size limit of the tile cache.
pub const DEFAULT_CACHE_BYTES: u64 = 500 * 1024 * 1024;
const WRITES_PER_EVICTION: u32 = 256;
/// Each file starts with the expiry time as little-endian Unix seconds.
const HEADER_LEN: usize = 8;

pub struct DiskCache {
    root: PathBuf,
    max_bytes: u64,
    writes: AtomicU32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub body: Vec<u8>,
    expires: SystemTime,
}

impl Entry {
    #[must_use]
    pub fn is_fresh(&self) -> bool {
        SystemTime::now() < self.expires
    }
}

impl DiskCache {
    #[must_use]
    pub fn new(root: PathBuf, max_bytes: u64) -> Self {
        Self {
            root,
            max_bytes,
            writes: AtomicU32::new(0),
        }
    }

    pub async fn get(&self, url: &str) -> Option<Entry> {
        let path = self.path(url);
        tokio::task::spawn_blocking(move || read_entry(&path).ok())
            .await
            .ok()
            .flatten()
    }

    pub async fn put(&self, url: &str, body: &[u8], expires: SystemTime) {
        let path = self.path(url);
        let mut contents = Vec::with_capacity(HEADER_LEN + body.len());
        contents.extend_from_slice(&unix_seconds(expires).to_le_bytes());
        contents.extend_from_slice(body);
        let _ = tokio::task::spawn_blocking(move || write_atomically(&path, &contents)).await;
        if self
            .writes
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(WRITES_PER_EVICTION)
        {
            let (root, max_bytes) = (self.root.clone(), self.max_bytes);
            tokio::task::spawn_blocking(move || evict(&root, max_bytes));
        }
    }

    /// Removes the least recently used files until the cache is below its size limit.
    ///
    /// # Errors
    ///
    /// Returns the error if the cache directory cannot be read.
    pub fn evict(&self) -> io::Result<()> {
        evict(&self.root, self.max_bytes)
    }

    /// A hash of the URL, so that the URL of one file can never be a directory of another, and
    /// two URLs never share a file. Two directory levels keep each directory small.
    fn path(&self, url: &str) -> PathBuf {
        let hash = format!("{:x}", Sha256::digest(url));
        let (dir, file) = hash.split_at(2);
        self.root.join(dir).join(file)
    }
}

fn read_entry(path: &Path) -> io::Result<Entry> {
    let mut contents = fs::read(path)?;
    let header: [u8; HEADER_LEN] = contents
        .get(..HEADER_LEN)
        .and_then(|header| header.try_into().ok())
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
    let expires = UNIX_EPOCH
        .checked_add(Duration::from_secs(u64::from_le_bytes(header)))
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
    File::options()
        .write(true)
        .open(path)?
        .set_modified(SystemTime::now())?;
    contents.drain(..HEADER_LEN);
    Ok(Entry {
        body: contents,
        expires,
    })
}

fn evict(root: &Path, max_bytes: u64) -> io::Result<()> {
    let mut files = Vec::new();
    for entry in fs::read_dir(root)? {
        collect_files(&entry?.path(), &mut files);
    }
    let mut total: u64 = files.iter().map(|file| file.size).sum();
    if total <= max_bytes {
        return Ok(());
    }
    // Stop at 90% of the limit, so that the next writes do not start an eviction again.
    let target = max_bytes - max_bytes / 10;
    files.sort_by_key(|file| file.modified);
    for file in files {
        if total <= target {
            break;
        }
        if fs::remove_file(&file.path).is_ok() {
            total = total.saturating_sub(file.size);
        }
    }
    Ok(())
}

struct CachedFile {
    path: PathBuf,
    size: u64,
    modified: SystemTime,
}

/// Skips what it cannot read: other tasks add and remove files during the walk.
fn collect_files(path: &Path, files: &mut Vec<CachedFile>) {
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    if metadata.is_dir() {
        for entry in fs::read_dir(path).into_iter().flatten().flatten() {
            collect_files(&entry.path(), files);
        }
    } else {
        files.push(CachedFile {
            path: path.to_owned(),
            size: metadata.len(),
            modified: metadata.modified().unwrap_or(UNIX_EPOCH),
        });
    }
}

fn unix_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    const URL: &str = "https://tiles.example/planet/20261018/14/8610/5863.pbf";

    fn cache(max_bytes: u64) -> (tempfile::TempDir, DiskCache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCache::new(dir.path().to_owned(), max_bytes);
        (dir, cache)
    }

    fn in_one_hour() -> SystemTime {
        SystemTime::now() + Duration::from_secs(3600)
    }

    #[tokio::test]
    async fn get_returns_what_put_stored() {
        let (_dir, cache) = cache(DEFAULT_CACHE_BYTES);
        cache.put(URL, b"tile", in_one_hour()).await;
        let entry = cache.get(URL).await.unwrap();
        assert_eq!(
            (entry.body.as_slice(), entry.is_fresh()),
            (&b"tile"[..], true)
        );
    }

    #[tokio::test]
    async fn entry_past_its_expiry_is_not_fresh() {
        let (_dir, cache) = cache(DEFAULT_CACHE_BYTES);
        cache.put(URL, b"tile", SystemTime::now()).await;
        assert!(!cache.get(URL).await.unwrap().is_fresh());
    }

    #[tokio::test]
    async fn url_below_a_cached_url_is_cached_too() {
        let (_dir, cache) = cache(DEFAULT_CACHE_BYTES);
        cache
            .put("https://tiles.example/planet", b"tilejson", in_one_hour())
            .await;
        cache.put(URL, b"tile", in_one_hour()).await;
        let bodies = (
            cache.get("https://tiles.example/planet").await,
            cache.get(URL).await,
        );
        assert_eq!(
            (bodies.0.unwrap().body, bodies.1.unwrap().body),
            (b"tilejson".to_vec(), b"tile".to_vec())
        );
    }

    #[tokio::test]
    async fn entry_with_an_invalid_expiry_is_a_miss() {
        let (_dir, cache) = cache(DEFAULT_CACHE_BYTES);
        let path = cache.path(URL);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, [0xff; 12]).unwrap();
        assert_eq!(cache.get(URL).await, None);
    }

    #[test]
    fn evict_removes_the_least_recently_used_files() {
        let (dir, cache) = cache(10);
        for (name, age_s) in [("old", 30), ("middle", 20), ("new", 10)] {
            let path = dir.path().join(name);
            fs::write(&path, [0; 5]).unwrap();
            File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(SystemTime::now() - Duration::from_secs(age_s))
                .unwrap();
        }
        cache.evict().unwrap();
        let mut left: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        left.sort();
        assert_eq!(left, ["new"]);
    }
}
