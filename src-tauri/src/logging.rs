use chrono::Local;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::fs;
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

pub const MAX_LOG_SIZE: u64 = 100 * 1024 * 1024; // 100 MB
pub const LOG_RETENTION_DAYS: u64 = 7;

/// Ensure the logs directory and history subdirectory exist, return logs dir path.
pub fn logs_dir(base_dir: &Path) -> PathBuf {
    let dir = base_dir.join("logs");
    let history = dir.join("history");
    let _ = fs::create_dir_all(&history);
    dir
}

/// Archive a current log file to history/ with gzip compression.
pub fn archive_current_log(log_path: &Path, prefix: &str) -> io::Result<PathBuf> {
    if !log_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "log file not found",
        ));
    }
    let history = log_path.parent().unwrap().join("history");
    let _ = fs::create_dir_all(&history);

    let timestamp = Local::now().format("%Y-%m-%dT%H-%M-%S");
    let mut n = 0u32;
    loop {
        let name = format!("{}-{}.{}.log.gz", prefix, timestamp, n);
        let dest = history.join(&name);
        if !dest.exists() {
            let input = fs::File::open(log_path)?;
            let mut reader = BufReader::new(input);
            let output = fs::File::create(&dest)?;
            let mut encoder = GzEncoder::new(BufWriter::new(output), Compression::default());
            let mut buf = [0u8; 8192];
            loop {
                let bytes_read = reader.read(&mut buf)?;
                if bytes_read == 0 {
                    break;
                }
                encoder.write_all(&buf[..bytes_read])?;
            }
            encoder.finish()?;
            fs::remove_file(log_path)?;
            return Ok(dest);
        }
        n += 1;
    }
}

/// Delete history files older than `days` days.
pub fn cleanup_old_logs(history_dir: &Path, days: u64) {
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(days * 24 * 3600);
    if let Ok(entries) = fs::read_dir(history_dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    if modified < cutoff {
                        let _ = fs::remove_file(entry.path());
                    }
                }
            }
        }
    }
}

/// If the log file exceeds `max_bytes`, archive it and return true.
pub fn rotate_if_needed(log_path: &Path, prefix: &str, max_bytes: u64) -> bool {
    if let Ok(meta) = fs::metadata(log_path) {
        if meta.len() > max_bytes {
            let _ = archive_current_log(log_path, prefix);
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_logs_dir_created() {
        let tmp = TempDir::new().unwrap();
        let logs = logs_dir(tmp.path());
        assert!(logs.exists());
        assert!(logs.join("history").exists());
    }

    #[test]
    fn test_archive_current_log() {
        let tmp = TempDir::new().unwrap();
        let logs = logs_dir(tmp.path());
        let log_file = logs.join("backend.log");
        fs::write(&log_file, "line1\nline2\n").unwrap();

        archive_current_log(&log_file, "backend").unwrap();

        assert!(!log_file.exists());
        let history = logs.join("history");
        let entries: Vec<_> = fs::read_dir(&history).unwrap().collect();
        assert_eq!(entries.len(), 1);
        let name = entries[0].as_ref().unwrap().file_name();
        let name = name.to_str().unwrap();
        assert!(name.starts_with("backend-"));
        assert!(name.ends_with(".0.log.gz"));
    }

    #[test]
    fn test_cleanup_old_logs() {
        let tmp = TempDir::new().unwrap();
        let logs = logs_dir(tmp.path());
        let history = logs.join("history");

        let fresh = history.join("backend-2099-01-01T00-00-00.0.log.gz");
        fs::write(&fresh, "data").unwrap();

        let old = history.join("backend-2020-01-01T00-00-00.0.log.gz");
        fs::write(&old, "data").unwrap();
        let thirty_days_ago =
            std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 24 * 3600);
        filetime::set_file_mtime(&old, filetime::FileTime::from_system_time(thirty_days_ago))
            .unwrap();

        cleanup_old_logs(&history, 7);

        assert!(fresh.exists());
        assert!(!old.exists());
    }

    #[test]
    fn test_rotate_if_needed_under_limit() {
        let tmp = TempDir::new().unwrap();
        let logs = logs_dir(tmp.path());
        let log_file = logs.join("backend.log");
        fs::write(&log_file, "small content").unwrap();

        let rotated = rotate_if_needed(&log_file, "backend", 100 * 1024 * 1024);
        assert!(!rotated);
        assert!(log_file.exists());
    }

    #[test]
    fn test_rotate_if_needed_over_limit() {
        let tmp = TempDir::new().unwrap();
        let logs = logs_dir(tmp.path());
        let log_file = logs.join("backend.log");
        fs::write(&log_file, "x".repeat(2000)).unwrap();

        let rotated = rotate_if_needed(&log_file, "backend", 1024);
        assert!(rotated);
        assert!(!log_file.exists());

        let history = logs.join("history");
        let entries: Vec<_> = fs::read_dir(&history).unwrap().collect();
        assert_eq!(entries.len(), 1);
    }
}
