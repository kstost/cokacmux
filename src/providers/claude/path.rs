//! Claude Code's filesystem encoding for `~/.claude/projects/<encoded-cwd>/`.
//!
//! Encoding: every path separator plus `.`, `_`, and Windows drive `:` in the
//! absolute path is replaced with `-`.
//! No length limit. Example:
//!   `/mnt/hgfs/vmware_ubuntu_shared/cokacmux`
//! → `-mnt-hgfs-vmware-ubuntu-shared-cokacmux`
//! And `/home/kst/.cokacmux-workspace-280AE0F2`
//! → `-home-kst--cokacmux-workspace-280AE0F2` (the leading dot becomes a
//!   second hyphen, producing `--`).

/// Encode an absolute filesystem path to Claude Code's directory naming.
pub fn encode_cwd(abs_path: &str) -> String {
    abs_path
        .chars()
        .map(|c| match c {
            '/' | '\\' | '.' | '_' | ':' => '-',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_observed_encodings() {
        assert_eq!(
            encode_cwd("/mnt/hgfs/vmware_ubuntu_shared/cokacmux"),
            "-mnt-hgfs-vmware-ubuntu-shared-cokacmux"
        );
        assert_eq!(
            encode_cwd("/home/kst/.cokacmux-workspace-280AE0F2"),
            "-home-kst--cokacmux-workspace-280AE0F2"
        );
        assert_eq!(
            encode_cwd(r"C:\Users\kst\repo.name"),
            "C--Users-kst-repo-name"
        );
    }
}
