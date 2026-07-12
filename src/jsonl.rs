//! JSONL (newline-delimited JSON) read/write helpers.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use serde_json::Value;

use crate::error::{ConvertError, Result};

/// Iterate over JSONL lines, yielding parsed `serde_json::Value` for each
/// non-empty line. Lines that fail to parse are reported as errors.
pub fn read_lines(path: &Path) -> Result<Vec<Value>> {
    let f = File::open(path).map_err(ConvertError::Io)?;
    let r = BufReader::new(f);
    let mut out = Vec::new();
    for (i, line) in r.lines().enumerate() {
        let line = line.map_err(ConvertError::Io)?;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(&line)
            .map_err(|e| ConvertError::Parse(format!("line {}: {}", i + 1, e)))?;
        out.push(v);
    }
    Ok(out)
}

/// Write `values` to `path` as JSONL (one compact JSON per line).
pub fn write_lines(path: &Path, values: &[Value]) -> Result<()> {
    let mut text = String::new();
    for v in values {
        text.push_str(&serde_json::to_string(v).map_err(ConvertError::Json)?);
        text.push('\n');
    }
    write_text_atomic(path, &text)
}

/// Atomic text write — write to a sibling temp file, fsync it, then rename.
/// This prevents process crashes from leaving a half-written file at `path`.
pub fn write_text_atomic(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("jsonl");
    let tmp = path.with_file_name(format!(".{}.tmp-{}", file_name, uuid::Uuid::now_v7()));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        // Session transcripts contain prompts, tool output, and often secrets.
        // Create the temporary file private from its first observable moment;
        // chmod-after-create alone would briefly expose it under a permissive
        // process umask.  Also set the exact mode so an unusually restrictive
        // umask does not leave an installed session unreadable by its owner.
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&tmp)?;
        #[cfg(unix)]
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        // std::fs::rename replaces an existing non-directory destination on
        // supported platforms.  Never emulate replacement by deleting `path`
        // first: if the following rename failed, the last complete copy would
        // already be gone and this function would violate its atomic-write
        // contract.
        fs::rename(&tmp, path)?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            let _ = File::open(parent).and_then(|dir| dir.sync_all());
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Atomic write — write to a sibling temp file then rename. Defends against
/// process crashes mid-write leaving a half-written file at `path`.
pub fn write_lines_atomic(path: &Path, values: &[Value]) -> Result<()> {
    write_lines(path, values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_complete_file_without_temp_remnants() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        fs::write(&path, "old\n").unwrap();

        write_text_atomic(&path, "new\ncomplete\n").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "new\ncomplete\n");
        let names = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(names, vec![path.file_name().unwrap().to_os_string()]);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_creates_private_session_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");

        write_text_atomic(&path, "secret\n").unwrap();

        let mode = fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn failed_replacement_keeps_destination_and_cleans_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("session.jsonl");
        fs::create_dir(&destination).unwrap();

        assert!(write_text_atomic(&destination, "new\n").is_err());
        assert!(destination.is_dir());
        let names = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        assert_eq!(names, vec![destination.file_name().unwrap().to_os_string()]);
    }
}
