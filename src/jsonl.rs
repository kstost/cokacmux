//! JSONL (newline-delimited JSON) read/write helpers.

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
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
    let f = File::create(path).map_err(ConvertError::Io)?;
    let mut w = BufWriter::new(f);
    for v in values {
        let s = serde_json::to_string(v).map_err(ConvertError::Json)?;
        w.write_all(s.as_bytes()).map_err(ConvertError::Io)?;
        w.write_all(b"\n").map_err(ConvertError::Io)?;
    }
    w.flush().map_err(ConvertError::Io)?;
    Ok(())
}

/// Atomic write — write to `<path>.tmp` then rename. Defends against crash
/// mid-write leaving a half-written file at `path`.
pub fn write_lines_atomic(path: &Path, values: &[Value]) -> Result<()> {
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension().and_then(|s| s.to_str()).unwrap_or("")
    ));
    write_lines(&tmp, values)?;
    std::fs::rename(&tmp, path).map_err(ConvertError::Io)?;
    Ok(())
}
