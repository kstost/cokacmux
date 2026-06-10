//! JSONL (newline-delimited JSON) read/write helpers.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
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
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        match fs::rename(&tmp, path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                fs::remove_file(path)?;
                fs::rename(&tmp, path)?;
            }
            Err(error) => return Err(error.into()),
        }
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
