//! Per-session working-directory snapshots managed by cokacmux.

use std::fs::{self, File};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{ConvertError, Result};
use crate::providers::discovery::{home_dir, SessionInfo};
use crate::universal::Provider;

use super::clone::CloneReport;

const APP_DIR_NAME: &str = ".cokacmux";
const DATA_DIR_NAME: &str = "data";
const DATA_STORE_VERSION: u32 = 1;
const COPY_CHUNK_SIZE: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDataSnapshot {
    #[serde(default = "data_store_version")]
    pub version: u32,
    pub provider: Provider,
    pub session_id: String,
    pub source_provider: Provider,
    pub source_session_id: String,
    pub original_cwd: String,
    pub snapshot_path: PathBuf,
    pub created_at_epoch_s: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopyStats {
    pub files: u64,
    pub dirs: u64,
    pub symlinks: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopyProgress {
    pub stats: CopyStats,
    pub total: Option<CopyStats>,
    pub current_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDataSnapshotReport {
    pub snapshot: SessionDataSnapshot,
    pub stats: CopyStats,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDataRestoreReport {
    pub snapshot: SessionDataSnapshot,
    pub target_path: PathBuf,
    pub backup_path: Option<PathBuf>,
    pub stats: CopyStats,
    pub snapshot_removed: bool,
    pub snapshot_remove_error: Option<String>,
}

fn data_store_version() -> u32 {
    DATA_STORE_VERSION
}

pub fn data_root() -> Result<PathBuf> {
    Ok(home_dir()?.join(APP_DIR_NAME).join(DATA_DIR_NAME))
}

pub fn snapshot_for_session(info: &SessionInfo) -> Result<Option<SessionDataSnapshot>> {
    snapshot_for_session_at(&data_root()?, info)
}

pub fn create_snapshot_for_clone(
    source: &SessionInfo,
    report: &CloneReport,
) -> Result<SessionDataSnapshotReport> {
    create_snapshot_for_clone_at(&data_root()?, source, report)
}

pub fn create_snapshot_for_clone_with_progress<F>(
    source: &SessionInfo,
    report: &CloneReport,
    cancel: &AtomicBool,
    mut on_progress: F,
) -> Result<SessionDataSnapshotReport>
where
    F: FnMut(CopyProgress),
{
    create_snapshot_for_clone_at_with_progress(
        &data_root()?,
        source,
        report,
        Some(cancel),
        &mut on_progress,
    )
}

pub fn restore_snapshot_for_session(info: &SessionInfo) -> Result<SessionDataRestoreReport> {
    restore_snapshot_for_session_at(&data_root()?, info)
}

pub fn remove_snapshot_for_session(info: &SessionInfo) -> Result<bool> {
    remove_snapshot_for_session_at(&data_root()?, info)
}

pub fn create_snapshot_for_clone_at(
    data_root: &Path,
    source: &SessionInfo,
    report: &CloneReport,
) -> Result<SessionDataSnapshotReport> {
    let mut noop = |_progress: CopyProgress| {};
    create_snapshot_for_clone_at_with_progress(data_root, source, report, None, &mut noop)
}

fn create_snapshot_for_clone_at_with_progress(
    data_root: &Path,
    source: &SessionInfo,
    report: &CloneReport,
    cancel: Option<&AtomicBool>,
    on_progress: &mut dyn FnMut(CopyProgress),
) -> Result<SessionDataSnapshotReport> {
    check_cancelled(cancel)?;
    let source_dir = snapshot_source_dir(source)?;
    ensure_data_root(data_root)?;

    let stem = snapshot_stem(report.target_provider, &report.new_session_id);
    let snapshot_path = data_root.join(&stem);
    let meta_path = data_root.join(format!("{stem}.json"));
    let temp_suffix = temp_suffix();
    let tmp_path = data_root.join(format!(".{stem}.tmp-{temp_suffix}"));
    let tmp_meta_path = data_root.join(format!(".{stem}.json.tmp-{temp_suffix}"));
    remove_path_if_exists(&tmp_path)?;
    remove_path_if_exists(&tmp_meta_path)?;

    let exclude_roots = vec![canonical_or_self(data_root)];
    let mut total = CopyStats::default();
    if let Err(error) = scan_copy_totals(&source_dir, &exclude_roots, cancel, &mut total) {
        let _ = remove_path_if_exists(&tmp_path);
        return Err(error);
    }
    let mut stats = CopyStats::default();
    if let Err(error) = copy_dir_contents_with_progress(
        &source_dir,
        &tmp_path,
        &exclude_roots,
        &mut stats,
        Some(&total),
        cancel,
        on_progress,
    ) {
        let _ = remove_path_if_exists(&tmp_path);
        return Err(error);
    }

    if let Err(error) = check_cancelled(cancel) {
        let _ = remove_path_if_exists(&tmp_path);
        return Err(error);
    }

    if let Err(error) = remove_path_if_exists(&snapshot_path)
        .and_then(|_| fs::rename(&tmp_path, &snapshot_path).map_err(Into::into))
    {
        let _ = remove_path_if_exists(&tmp_path);
        return Err(error);
    }

    if let Err(error) = check_cancelled(cancel) {
        let _ = remove_path_if_exists(&snapshot_path);
        return Err(error);
    }

    let snapshot = SessionDataSnapshot {
        version: DATA_STORE_VERSION,
        provider: report.target_provider,
        session_id: report.new_session_id.clone(),
        source_provider: source.provider,
        source_session_id: source.session_id.clone(),
        original_cwd: source.cwd.clone(),
        snapshot_path: snapshot_path.clone(),
        created_at_epoch_s: current_epoch_s(),
    };
    let content = serde_json::to_vec_pretty(&snapshot)?;
    if let Err(error) = fs::write(&tmp_meta_path, content)
        .map_err(ConvertError::from)
        .and_then(|_| {
            set_private_file_permissions(&tmp_meta_path);
            fs::rename(&tmp_meta_path, &meta_path).map_err(Into::into)
        })
    {
        let _ = remove_path_if_exists(&tmp_meta_path);
        let _ = remove_path_if_exists(&snapshot_path);
        return Err(error);
    }

    crate::debug::log(
        "session_data_snapshot_created",
        serde_json::json!({
            "source_provider": source.provider.as_str(),
            "source_session_id": &source.session_id,
            "target_provider": report.target_provider.as_str(),
            "target_session_id": &report.new_session_id,
            "source_dir": source_dir.display().to_string(),
            "snapshot_path": snapshot_path.display().to_string(),
            "files": stats.files,
            "dirs": stats.dirs,
            "symlinks": stats.symlinks,
            "bytes": stats.bytes,
        }),
    );

    Ok(SessionDataSnapshotReport { snapshot, stats })
}

pub fn snapshot_for_session_at(
    data_root: &Path,
    info: &SessionInfo,
) -> Result<Option<SessionDataSnapshot>> {
    let stem = snapshot_stem(info.provider, &info.session_id);
    let meta_path = data_root.join(format!("{stem}.json"));
    if !meta_path.exists() {
        return Ok(None);
    }
    let mut snapshot: SessionDataSnapshot = serde_json::from_slice(&fs::read(&meta_path)?)?;
    snapshot.snapshot_path = data_root.join(&stem);
    if !snapshot.snapshot_path.is_dir() {
        crate::debug::log(
            "session_data_snapshot_missing_dir",
            serde_json::json!({
                "provider": info.provider.as_str(),
                "session_id": &info.session_id,
                "meta_path": meta_path.display().to_string(),
                "snapshot_path": snapshot.snapshot_path.display().to_string(),
            }),
        );
        return Ok(None);
    }
    Ok(Some(snapshot))
}

pub fn restore_snapshot_for_session_at(
    data_root: &Path,
    info: &SessionInfo,
) -> Result<SessionDataRestoreReport> {
    let Some(snapshot) = snapshot_for_session_at(data_root, info)? else {
        return Err(ConvertError::Other(format!(
            "no saved folder snapshot for {} {}",
            info.provider.as_str(),
            info.session_id
        )));
    };
    if info.cwd.trim().is_empty() {
        return Err(ConvertError::MissingField("session.cwd"));
    }
    let target_path = PathBuf::from(&info.cwd);
    if paths_overlap(&snapshot.snapshot_path, &target_path) {
        return Err(ConvertError::Other(format!(
            "refusing to restore snapshot into itself: {}",
            target_path.display()
        )));
    }
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut backup_path = None;
    if path_entry_exists(&target_path) {
        let backup = unique_backup_path(&target_path)?;
        fs::rename(&target_path, &backup)?;
        backup_path = Some(backup);
    }

    let mut stats = CopyStats::default();
    let restore_result = copy_dir_contents(&snapshot.snapshot_path, &target_path, &[], &mut stats);
    if let Err(error) = restore_result {
        let mut message = error.to_string();
        if let Err(cleanup_error) = remove_path_if_exists(&target_path) {
            message.push_str(&format!(
                "; failed to remove partial restore at {}: {}",
                target_path.display(),
                cleanup_error
            ));
        }
        if let Some(backup) = backup_path.as_ref() {
            if let Err(restore_error) = fs::rename(backup, &target_path) {
                message.push_str(&format!(
                    "; failed to restore backup {} to {}: {}",
                    backup.display(),
                    target_path.display(),
                    restore_error
                ));
            }
        }
        return Err(ConvertError::Other(message));
    }

    let (snapshot_removed, snapshot_remove_error) =
        match remove_snapshot_for_session_at(data_root, info) {
            Ok(removed) => (removed, None),
            Err(e) => {
                let error = e.to_string();
                crate::debug::log(
                    "session_data_snapshot_remove_after_restore_failed",
                    serde_json::json!({
                        "provider": info.provider.as_str(),
                        "session_id": &info.session_id,
                        "snapshot_path": snapshot.snapshot_path.display().to_string(),
                        "error": &error,
                    }),
                );
                (false, Some(error))
            }
        };

    crate::debug::log(
        "session_data_snapshot_restored",
        serde_json::json!({
            "provider": info.provider.as_str(),
            "session_id": &info.session_id,
            "target_path": target_path.display().to_string(),
            "snapshot_path": snapshot.snapshot_path.display().to_string(),
            "backup_path": backup_path.as_ref().map(|path| path.display().to_string()),
            "files": stats.files,
            "dirs": stats.dirs,
            "symlinks": stats.symlinks,
            "bytes": stats.bytes,
            "snapshot_removed": snapshot_removed,
            "snapshot_remove_error": snapshot_remove_error,
        }),
    );

    Ok(SessionDataRestoreReport {
        snapshot,
        target_path,
        backup_path,
        stats,
        snapshot_removed,
        snapshot_remove_error,
    })
}

pub fn remove_snapshot_for_session_at(data_root: &Path, info: &SessionInfo) -> Result<bool> {
    let stem = snapshot_stem(info.provider, &info.session_id);
    let snapshot_path = data_root.join(&stem);
    let meta_path = data_root.join(format!("{stem}.json"));
    let mut removed = false;
    if path_entry_exists(&snapshot_path) {
        remove_path_if_exists(&snapshot_path)?;
        removed = true;
    }
    if path_entry_exists(&meta_path) {
        remove_path_if_exists(&meta_path)?;
        removed = true;
    }
    Ok(removed)
}

pub fn snapshot_source_dir(info: &SessionInfo) -> Result<PathBuf> {
    if info.cwd.trim().is_empty() {
        return Err(ConvertError::MissingField("session.cwd"));
    }
    let path = PathBuf::from(&info.cwd);
    let metadata = fs::metadata(&path)?;
    if !metadata.is_dir() {
        return Err(ConvertError::Other(format!(
            "session cwd is not a directory: {}",
            path.display()
        )));
    }
    Ok(path)
}

fn ensure_data_root(data_root: &Path) -> Result<()> {
    fs::create_dir_all(data_root)?;
    set_private_dir_permissions(data_root);
    if let Some(parent) = data_root.parent() {
        set_private_dir_permissions(parent);
    }
    Ok(())
}

fn copy_dir_contents(
    source: &Path,
    dest: &Path,
    exclude_roots: &[PathBuf],
    stats: &mut CopyStats,
) -> Result<()> {
    let mut noop = |_progress: CopyProgress| {};
    copy_dir_contents_with_progress(source, dest, exclude_roots, stats, None, None, &mut noop)
}

fn copy_dir_contents_with_progress(
    source: &Path,
    dest: &Path,
    exclude_roots: &[PathBuf],
    stats: &mut CopyStats,
    total: Option<&CopyStats>,
    cancel: Option<&AtomicBool>,
    on_progress: &mut dyn FnMut(CopyProgress),
) -> Result<()> {
    check_cancelled(cancel)?;
    fs::create_dir_all(dest)?;
    stats.dirs = stats.dirs.saturating_add(1);
    emit_copy_progress(stats, total, source, on_progress);

    for entry in fs::read_dir(source)? {
        check_cancelled(cancel)?;
        let entry = entry?;
        let source_path = entry.path();
        if should_skip_path(&source_path, exclude_roots) {
            continue;
        }
        let dest_path = dest.join(entry.file_name());
        copy_path_with_progress(
            &source_path,
            &dest_path,
            exclude_roots,
            stats,
            total,
            cancel,
            on_progress,
        )?;
    }
    check_cancelled(cancel)?;
    if let Ok(metadata) = fs::metadata(source) {
        let _ = fs::set_permissions(dest, metadata.permissions());
    }
    Ok(())
}

fn copy_path_with_progress(
    source: &Path,
    dest: &Path,
    exclude_roots: &[PathBuf],
    stats: &mut CopyStats,
    total: Option<&CopyStats>,
    cancel: Option<&AtomicBool>,
    on_progress: &mut dyn FnMut(CopyProgress),
) -> Result<()> {
    check_cancelled(cancel)?;
    let metadata = fs::symlink_metadata(source)?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        copy_symlink(source, dest)?;
        check_cancelled(cancel)?;
        stats.symlinks = stats.symlinks.saturating_add(1);
        emit_copy_progress(stats, total, source, on_progress);
        return Ok(());
    }
    if unsupported_reparse_point(&metadata) {
        return Err(unsupported_entry_error(source));
    }
    if file_type.is_dir() {
        return copy_dir_contents_with_progress(
            source,
            dest,
            exclude_roots,
            stats,
            total,
            cancel,
            on_progress,
        );
    }
    if file_type.is_file() {
        copy_file_chunked_with_progress(
            source,
            dest,
            &metadata,
            stats,
            total,
            cancel,
            on_progress,
        )?;
        return Ok(());
    }

    Err(unsupported_entry_error(source))
}

fn copy_file_chunked_with_progress(
    source: &Path,
    dest: &Path,
    metadata: &fs::Metadata,
    stats: &mut CopyStats,
    total: Option<&CopyStats>,
    cancel: Option<&AtomicBool>,
    on_progress: &mut dyn FnMut(CopyProgress),
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut reader = File::open(source)?;
    let mut writer = File::create(dest)?;
    let mut buffer = vec![0; COPY_CHUNK_SIZE];
    loop {
        check_cancelled(cancel)?;
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        check_cancelled(cancel)?;
        writer.write_all(&buffer[..read])?;
        stats.bytes = stats.bytes.saturating_add(read as u64);
        emit_copy_progress(stats, total, source, on_progress);
    }
    writer.flush()?;
    check_cancelled(cancel)?;
    let _ = fs::set_permissions(dest, metadata.permissions());
    stats.files = stats.files.saturating_add(1);
    emit_copy_progress(stats, total, source, on_progress);
    Ok(())
}

fn scan_copy_totals(
    source: &Path,
    exclude_roots: &[PathBuf],
    cancel: Option<&AtomicBool>,
    total: &mut CopyStats,
) -> Result<()> {
    check_cancelled(cancel)?;
    let metadata = fs::symlink_metadata(source)?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        total.symlinks = total.symlinks.saturating_add(1);
        return Ok(());
    }
    if unsupported_reparse_point(&metadata) {
        return Err(unsupported_entry_error(source));
    }
    if file_type.is_dir() {
        total.dirs = total.dirs.saturating_add(1);
        for entry in fs::read_dir(source)? {
            check_cancelled(cancel)?;
            let entry = entry?;
            let source_path = entry.path();
            if should_skip_path(&source_path, exclude_roots) {
                continue;
            }
            scan_copy_totals(&source_path, exclude_roots, cancel, total)?;
        }
        return Ok(());
    }
    if file_type.is_file() {
        total.files = total.files.saturating_add(1);
        total.bytes = total.bytes.saturating_add(metadata.len());
        return Ok(());
    }
    Err(unsupported_entry_error(source))
}

fn check_cancelled(cancel: Option<&AtomicBool>) -> Result<()> {
    if cancel.is_some_and(|token| token.load(Ordering::Relaxed)) {
        Err(ConvertError::Cancelled("operation cancelled".into()))
    } else {
        Ok(())
    }
}

fn emit_copy_progress(
    stats: &CopyStats,
    total: Option<&CopyStats>,
    current_path: &Path,
    on_progress: &mut dyn FnMut(CopyProgress),
) {
    on_progress(CopyProgress {
        stats: stats.clone(),
        total: total.cloned(),
        current_path: current_path.to_path_buf(),
    });
}

fn unsupported_entry_error(source: &Path) -> ConvertError {
    ConvertError::Other(format!(
        "unsupported filesystem entry in session data: {}",
        source.display()
    ))
}

#[cfg(windows)]
fn unsupported_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn unsupported_reparse_point(_metadata: &fs::Metadata) -> bool {
    false
}

#[cfg(unix)]
fn copy_symlink(source: &Path, dest: &Path) -> Result<()> {
    use std::os::unix::fs as unix_fs;

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let target = fs::read_link(source)?;
    unix_fs::symlink(target, dest)?;
    Ok(())
}

#[cfg(windows)]
fn copy_symlink(source: &Path, dest: &Path) -> Result<()> {
    use std::os::windows::fs::{self as windows_fs, FileTypeExt};

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let target = fs::read_link(source)?;
    let file_type = fs::symlink_metadata(source)?.file_type();
    if file_type.is_symlink_dir() {
        windows_fs::symlink_dir(target, dest)?;
    } else {
        windows_fs::symlink_file(target, dest)?;
    }
    Ok(())
}

fn should_skip_path(path: &Path, exclude_roots: &[PathBuf]) -> bool {
    let candidate = canonical_or_self(path);
    exclude_roots
        .iter()
        .any(|root| candidate == *root || candidate.starts_with(root))
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    let left = canonical_or_existing_parent(left);
    let right = canonical_or_existing_parent(right);
    path_eq_or_child(&left, &right) || path_eq_or_child(&right, &left)
}

#[cfg(windows)]
fn path_eq_or_child(path: &Path, parent: &Path) -> bool {
    let path = windows_path_compare_key(path);
    let parent = windows_path_compare_key(parent);
    if path == parent {
        return true;
    }
    path.strip_prefix(&parent)
        .is_some_and(|rest| rest.starts_with('\\'))
}

#[cfg(windows)]
fn windows_path_compare_key(path: &Path) -> String {
    let mut value = path
        .to_string_lossy()
        .replace('/', "\\")
        .to_ascii_lowercase();
    while value.len() > 3 && value.ends_with('\\') {
        value.pop();
    }
    value
}

#[cfg(not(windows))]
fn path_eq_or_child(path: &Path, parent: &Path) -> bool {
    path == parent || path.starts_with(parent)
}

fn remove_path_if_exists(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => {
            make_tree_removable(path);
            fs::remove_dir_all(path)?;
            Ok(())
        }
        Ok(metadata) if metadata.file_type().is_symlink() => remove_symlink(path, &metadata),
        Ok(_) => {
            set_removable_file_permissions(path);
            fs::remove_file(path).map_err(Into::into)
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(windows)]
fn remove_symlink(path: &Path, metadata: &fs::Metadata) -> Result<()> {
    use std::os::windows::fs::FileTypeExt;

    if metadata.file_type().is_symlink_dir() {
        fs::remove_dir(path)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(not(windows))]
fn remove_symlink(path: &Path, _metadata: &fs::Metadata) -> Result<()> {
    fs::remove_file(path).map_err(Into::into)
}

fn make_tree_removable(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    if !metadata.file_type().is_dir() {
        set_removable_file_permissions(path);
        return;
    }
    set_removable_dir_permissions(path);
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            make_tree_removable(&entry.path());
        }
    }
    set_removable_dir_permissions(path);
}

#[cfg(unix)]
fn set_removable_dir_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    let mode = metadata.permissions().mode() | 0o700;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

#[cfg(windows)]
fn set_removable_dir_permissions(path: &Path) {
    clear_readonly_attribute(path);
}

#[cfg(not(any(unix, windows)))]
fn set_removable_dir_permissions(_path: &Path) {}

#[cfg(windows)]
fn set_removable_file_permissions(path: &Path) {
    clear_readonly_attribute(path);
}

#[cfg(not(windows))]
fn set_removable_file_permissions(_path: &Path) {}

#[cfg(windows)]
fn clear_readonly_attribute(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    let mut permissions = metadata.permissions();
    if permissions.readonly() {
        permissions.set_readonly(false);
        let _ = fs::set_permissions(path, permissions);
    }
}

#[cfg(unix)]
fn set_private_dir_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_path: &Path) {}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &Path) {}

fn unique_backup_path(target: &Path) -> Result<PathBuf> {
    let parent = target
        .parent()
        .ok_or_else(|| ConvertError::Other(format!("cannot back up {}", target.display())))?;
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ConvertError::Other(format!("cannot back up {}", target.display())))?;
    let base = format!("{name}.cokacmux-backup-{}", current_epoch_s());
    for index in 0..1000 {
        let candidate = if index == 0 {
            parent.join(&base)
        } else {
            parent.join(format!("{base}-{index}"))
        };
        if !path_entry_exists(&candidate) {
            return Ok(candidate);
        }
    }
    Err(ConvertError::Other(format!(
        "cannot allocate backup path for {}",
        target.display()
    )))
}

fn snapshot_stem(provider: Provider, session_id: &str) -> String {
    format!("{}-{}", provider.as_str(), safe_path_component(session_id))
}

fn safe_path_component(value: &str) -> String {
    if value.is_empty() {
        return "%EMPTY".to_string();
    }

    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'-' | b'_') {
            out.push(char::from(*byte));
        } else {
            out.push('%');
            out.push(hex_digit(byte >> 4));
            out.push(hex_digit(byte & 0x0f));
        }
    }
    out
}

fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => char::from(b'0' + nibble),
        10..=15 => char::from(b'A' + (nibble - 10)),
        _ => unreachable!("hex nibble is always <= 15"),
    }
}

fn canonical_or_self(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn canonical_or_existing_parent(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }

    let mut missing_components = Vec::new();
    let mut current = path;
    while let Some(parent) = current.parent() {
        if let Some(name) = current.file_name() {
            missing_components.push(name.to_os_string());
        }
        if let Ok(mut canonical_parent) = parent.canonicalize() {
            for component in missing_components.iter().rev() {
                canonical_parent.push(component);
            }
            return canonical_parent;
        }
        current = parent;
    }

    path.to_path_buf()
}

fn path_entry_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn current_epoch_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn temp_suffix() -> String {
    format!("{}-{}", current_epoch_s(), std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::clone::{ArtifactPath, CloneReport};

    fn info(provider: Provider, session_id: &str, cwd: &Path) -> SessionInfo {
        SessionInfo {
            provider,
            session_id: session_id.to_string(),
            cwd: cwd.display().to_string(),
            source: PathBuf::from("/tmp/source"),
            updated_at_epoch_s: 0,
            title: None,
        }
    }

    #[test]
    fn snapshot_for_clone_copies_and_restores_directory_with_backup() {
        let dir = tempfile::tempdir().unwrap();
        let data_root = dir.path().join("data");
        let project = dir.path().join("project");
        fs::create_dir_all(project.join("src")).unwrap();
        fs::write(project.join("src").join("main.rs"), "original").unwrap();
        fs::write(project.join("README.md"), "snapshot").unwrap();

        let source = info(Provider::Codex, "source", &project);
        let report = CloneReport {
            source_provider: Provider::Codex,
            source_session_id: "source".into(),
            new_session_id: "clone-1".into(),
            target_provider: Provider::Codex,
            artifact: ArtifactPath::File(dir.path().join("clone.jsonl")),
        };

        let snapshot = create_snapshot_for_clone_at(&data_root, &source, &report).unwrap();
        assert_eq!(snapshot.stats.files, 2);
        assert!(snapshot.snapshot.snapshot_path.join("src/main.rs").exists());

        fs::write(project.join("README.md"), "current").unwrap();
        fs::write(project.join("only-current.txt"), "keep in backup").unwrap();

        let clone = info(Provider::Codex, "clone-1", &project);
        let restored = restore_snapshot_for_session_at(&data_root, &clone).unwrap();
        assert!(restored.snapshot_removed);
        assert!(restored.snapshot_remove_error.is_none());
        assert_eq!(
            fs::read_to_string(project.join("README.md")).unwrap(),
            "snapshot"
        );
        assert_eq!(
            fs::read_to_string(project.join("src").join("main.rs")).unwrap(),
            "original"
        );
        assert!(!project.join("only-current.txt").exists());

        let backup = restored
            .backup_path
            .expect("existing project should be backed up");
        assert_eq!(
            fs::read_to_string(backup.join("README.md")).unwrap(),
            "current"
        );
        assert_eq!(
            fs::read_to_string(backup.join("only-current.txt")).unwrap(),
            "keep in backup"
        );
        assert!(!snapshot.snapshot.snapshot_path.exists());
        assert!(!data_root.join("codex-clone-1.json").exists());
    }

    #[test]
    fn snapshot_copy_skips_data_root_when_source_contains_it() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("home");
        let data_root = source.join(".cokacmux").join("data");
        fs::create_dir_all(&data_root).unwrap();
        fs::write(source.join("project.txt"), "included").unwrap();
        fs::write(data_root.join("existing.txt"), "excluded").unwrap();

        let source_info = info(Provider::Claude, "source", &source);
        let report = CloneReport {
            source_provider: Provider::Claude,
            source_session_id: "source".into(),
            new_session_id: "clone-2".into(),
            target_provider: Provider::Claude,
            artifact: ArtifactPath::File(dir.path().join("clone.jsonl")),
        };

        let snapshot = create_snapshot_for_clone_at(&data_root, &source_info, &report).unwrap();
        assert!(snapshot.snapshot.snapshot_path.join("project.txt").exists());
        assert!(!snapshot
            .snapshot
            .snapshot_path
            .join(".cokacmux/data")
            .exists());
    }

    #[test]
    fn cancelled_snapshot_copy_cleans_partial_data() {
        let dir = tempfile::tempdir().unwrap();
        let data_root = dir.path().join("data");
        let project = dir.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("README.md"), "snapshot").unwrap();

        let source = info(Provider::Codex, "source", &project);
        let report = CloneReport {
            source_provider: Provider::Codex,
            source_session_id: "source".into(),
            new_session_id: "clone-cancelled".into(),
            target_provider: Provider::Codex,
            artifact: ArtifactPath::File(dir.path().join("clone.jsonl")),
        };
        let cancel = AtomicBool::new(true);

        let error = create_snapshot_for_clone_at_with_progress(
            &data_root,
            &source,
            &report,
            Some(&cancel),
            &mut |_progress| {},
        )
        .expect_err("cancelled snapshot creation should fail");

        assert!(matches!(error, ConvertError::Cancelled(_)));
        assert!(!data_root.join("codex-clone-cancelled").exists());
        assert!(!data_root.join("codex-clone-cancelled.json").exists());
        if data_root.exists() {
            let leftovers: Vec<_> = fs::read_dir(&data_root)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            assert!(leftovers.is_empty(), "leftover temp entries: {leftovers:?}");
        }
    }

    #[test]
    fn snapshot_copy_can_cancel_during_large_file_chunk() {
        let dir = tempfile::tempdir().unwrap();
        let data_root = dir.path().join("data");
        let project = dir.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("large.bin"),
            vec![7_u8; COPY_CHUNK_SIZE + 4096],
        )
        .unwrap();

        let source = info(Provider::Codex, "source", &project);
        let report = CloneReport {
            source_provider: Provider::Codex,
            source_session_id: "source".into(),
            new_session_id: "clone-cancelled-chunk".into(),
            target_provider: Provider::Codex,
            artifact: ArtifactPath::File(dir.path().join("clone.jsonl")),
        };
        let cancel = AtomicBool::new(false);

        let error = create_snapshot_for_clone_at_with_progress(
            &data_root,
            &source,
            &report,
            Some(&cancel),
            &mut |progress| {
                if progress.stats.bytes > 0 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
        )
        .expect_err("chunk-level cancellation should fail snapshot creation");

        assert!(matches!(error, ConvertError::Cancelled(_)));
        assert!(!data_root.join("codex-clone-cancelled-chunk").exists());
        assert!(!data_root.join("codex-clone-cancelled-chunk.json").exists());
        let leftovers: Vec<_> = fs::read_dir(&data_root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(leftovers.is_empty(), "leftover temp entries: {leftovers:?}");
    }

    #[test]
    fn missing_snapshot_directory_is_treated_as_no_saved_data() {
        let dir = tempfile::tempdir().unwrap();
        let data_root = dir.path().join("data");
        fs::create_dir_all(&data_root).unwrap();
        let source = info(Provider::Codex, "source", dir.path());
        let report = CloneReport {
            source_provider: Provider::Codex,
            source_session_id: "source".into(),
            new_session_id: "clone-missing-data".into(),
            target_provider: Provider::Codex,
            artifact: ArtifactPath::File(dir.path().join("clone.jsonl")),
        };

        let snapshot = SessionDataSnapshot {
            version: DATA_STORE_VERSION,
            provider: report.target_provider,
            session_id: report.new_session_id.clone(),
            source_provider: source.provider,
            source_session_id: source.session_id,
            original_cwd: source.cwd,
            snapshot_path: data_root.join("codex-clone-missing-data"),
            created_at_epoch_s: current_epoch_s(),
        };
        fs::write(
            data_root.join("codex-clone-missing-data.json"),
            serde_json::to_vec_pretty(&snapshot).unwrap(),
        )
        .unwrap();

        let clone = info(Provider::Codex, "clone-missing-data", dir.path());
        assert!(snapshot_for_session_at(&data_root, &clone)
            .unwrap()
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn failed_snapshot_copy_cleans_partial_data() {
        let dir = tempfile::tempdir().unwrap();
        let data_root = dir.path().join("data");
        let project = dir.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("included.txt"), "included").unwrap();
        let _socket = std::os::unix::net::UnixListener::bind(project.join("unsupported.sock"))
            .expect("create unsupported filesystem entry");

        let source = info(Provider::Codex, "source", &project);
        let report = CloneReport {
            source_provider: Provider::Codex,
            source_session_id: "source".into(),
            new_session_id: "clone-partial-failure".into(),
            target_provider: Provider::Codex,
            artifact: ArtifactPath::File(dir.path().join("clone.jsonl")),
        };

        let error = create_snapshot_for_clone_at(&data_root, &source, &report)
            .expect_err("unsupported entries should fail snapshot creation");
        assert!(error
            .to_string()
            .contains("unsupported filesystem entry in session data"));
        assert!(!data_root.join("codex-clone-partial-failure").exists());
        assert!(!data_root.join("codex-clone-partial-failure.json").exists());
        let leftovers: Vec<_> = fs::read_dir(&data_root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(leftovers.is_empty(), "leftover temp entries: {leftovers:?}");
    }

    #[cfg(unix)]
    #[test]
    fn remove_snapshot_handles_readonly_snapshot_dirs() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let data_root = dir.path().join("data");
        let project = dir.path().join("project");
        let readonly_dir = project.join("readonly");
        fs::create_dir_all(&readonly_dir).unwrap();
        fs::write(readonly_dir.join("file.txt"), "content").unwrap();
        fs::set_permissions(&readonly_dir, fs::Permissions::from_mode(0o555)).unwrap();

        let source = info(Provider::Claude, "source", &project);
        let report = CloneReport {
            source_provider: Provider::Claude,
            source_session_id: "source".into(),
            new_session_id: "clone-readonly".into(),
            target_provider: Provider::Claude,
            artifact: ArtifactPath::File(dir.path().join("clone.jsonl")),
        };

        create_snapshot_for_clone_at(&data_root, &source, &report).unwrap();
        fs::set_permissions(&readonly_dir, fs::Permissions::from_mode(0o700)).unwrap();

        let clone = info(Provider::Claude, "clone-readonly", &project);
        assert!(remove_snapshot_for_session_at(&data_root, &clone).unwrap());
        assert!(!data_root.join("claude-clone-readonly").exists());
        assert!(!data_root.join("claude-clone-readonly.json").exists());
    }

    #[test]
    fn remove_snapshot_handles_readonly_snapshot_files() {
        let dir = tempfile::tempdir().unwrap();
        let data_root = dir.path().join("data");
        let project = dir.path().join("project");
        fs::create_dir_all(&project).unwrap();
        let readonly_file = project.join("readonly.txt");
        fs::write(&readonly_file, "content").unwrap();
        set_readonly(&readonly_file, true);

        let source = info(Provider::Codex, "source", &project);
        let report = CloneReport {
            source_provider: Provider::Codex,
            source_session_id: "source".into(),
            new_session_id: "clone-readonly-file".into(),
            target_provider: Provider::Codex,
            artifact: ArtifactPath::File(dir.path().join("clone.jsonl")),
        };

        create_snapshot_for_clone_at(&data_root, &source, &report).unwrap();
        set_readonly(&readonly_file, false);

        let clone = info(Provider::Codex, "clone-readonly-file", &project);
        assert!(remove_snapshot_for_session_at(&data_root, &clone).unwrap());
        assert!(!data_root.join("codex-clone-readonly-file").exists());
        assert!(!data_root.join("codex-clone-readonly-file.json").exists());
    }

    #[cfg(unix)]
    #[test]
    fn restore_backs_up_broken_symlink_target_entry() {
        use std::os::unix::fs as unix_fs;

        let dir = tempfile::tempdir().unwrap();
        let data_root = dir.path().join("data");
        let project = dir.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("README.md"), "snapshot").unwrap();

        let source = info(Provider::Codex, "source", &project);
        let report = CloneReport {
            source_provider: Provider::Codex,
            source_session_id: "source".into(),
            new_session_id: "clone-broken-link".into(),
            target_provider: Provider::Codex,
            artifact: ArtifactPath::File(dir.path().join("clone.jsonl")),
        };
        create_snapshot_for_clone_at(&data_root, &source, &report).unwrap();

        let target = dir.path().join("target-project");
        unix_fs::symlink("missing-target", &target).unwrap();
        let clone = info(Provider::Codex, "clone-broken-link", &target);
        let restored = restore_snapshot_for_session_at(&data_root, &clone).unwrap();

        let backup = restored
            .backup_path
            .expect("broken target symlink should be backed up");
        assert_eq!(
            fs::read_link(backup).unwrap(),
            PathBuf::from("missing-target")
        );
        assert_eq!(
            fs::read_to_string(target.join("README.md")).unwrap(),
            "snapshot"
        );
    }

    #[cfg(unix)]
    #[test]
    fn restore_refuses_missing_target_inside_snapshot_via_symlink_parent() {
        use std::os::unix::fs as unix_fs;

        let dir = tempfile::tempdir().unwrap();
        let data_root = dir.path().join("data");
        let project = dir.path().join("project");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("README.md"), "snapshot").unwrap();

        let source = info(Provider::Codex, "source", &project);
        let report = CloneReport {
            source_provider: Provider::Codex,
            source_session_id: "source".into(),
            new_session_id: "clone-symlink-overlap".into(),
            target_provider: Provider::Codex,
            artifact: ArtifactPath::File(dir.path().join("clone.jsonl")),
        };
        let snapshot = create_snapshot_for_clone_at(&data_root, &source, &report).unwrap();

        let linked_snapshot = dir.path().join("linked-snapshot");
        unix_fs::symlink(&snapshot.snapshot.snapshot_path, &linked_snapshot).unwrap();
        let nested_target = linked_snapshot.join("nested-target");
        let clone = info(Provider::Codex, "clone-symlink-overlap", &nested_target);

        let error = restore_snapshot_for_session_at(&data_root, &clone).unwrap_err();

        assert!(error
            .to_string()
            .contains("refusing to restore snapshot into itself"));
        assert!(snapshot.snapshot.snapshot_path.exists());
    }

    #[cfg(windows)]
    #[test]
    fn paths_overlap_is_case_insensitive_on_windows() {
        assert!(paths_overlap(
            Path::new(r"C:\Users\me\.cokacmux\data\codex-id"),
            Path::new(r"c:\users\ME\.cokacmux\data\codex-id\child")
        ));
    }

    #[test]
    fn snapshot_file_stems_do_not_collapse_unsafe_session_ids() {
        assert_eq!(safe_path_component("abc-_123"), "abc-_123");
        assert_eq!(safe_path_component("abc-_.123"), "abc-_%2E123");
        assert_eq!(safe_path_component(""), "%EMPTY");
        assert_ne!(safe_path_component("a/b"), safe_path_component("a_b"));
        assert_ne!(safe_path_component("a.b"), safe_path_component("a%2Eb"));
        assert_ne!(safe_path_component("a%b"), safe_path_component("a%25b"));
        assert_ne!(
            snapshot_stem(Provider::Codex, "same/id"),
            snapshot_stem(Provider::Codex, "same_id")
        );
    }

    fn set_readonly(path: &Path, readonly: bool) {
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_readonly(readonly);
        fs::set_permissions(path, permissions).unwrap();
    }
}
