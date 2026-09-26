//! Detects whether a path lives on a network filesystem, where SQLite's own
//! documentation says WAL mode is unsafe under concurrent access. Detection is
//! advisory only: callers warn and proceed, never refuse or switch journal modes.

use std::path::{Path, PathBuf};

/// Filesystem types SQLite's documentation calls out as unsafe for WAL mode.
const NETWORK_FSTYPES: [&str; 13] = [
    "nfs",
    "nfs4",
    "cifs",
    "smb3",
    "smbfs",
    "9p",
    "fuse.sshfs",
    "fuse.rclone",
    "afs",
    "ceph",
    "glusterfs",
    "davfs",
    "fuse.davfs2",
];

/// The network filesystem type the (nearest existing ancestor of) `path` sits on, or
/// `None` when it's local. Linux only: reads `/proc/mounts`, which doesn't exist on
/// macOS or Windows, so the read fails there and this returns `None` without any
/// platform-specific code. A real macOS check would need `statfs(2)`, which isn't worth
/// a new `libc` dependency for an advisory warning.
pub fn network_fs(path: &Path) -> Option<String> {
    let table = std::fs::read_to_string("/proc/mounts").ok()?;
    let anchor = nearest_existing_ancestor(path)?;
    let fstype = mount_fstype(&table, &anchor)?;
    NETWORK_FSTYPES.contains(&fstype.as_str()).then_some(fstype)
}

/// Prints a one-line stderr warning when `path` sits on a network filesystem. Never
/// refuses and never changes the journal mode; setup and relocate call this on every
/// vault path they choose.
pub fn warn_if_unsafe(path: &Path) {
    if let Some(fstype) = network_fs(path) {
        eprintln!(
            "warning: SQLite WAL mode is not safe on network filesystems ({fstype}); \
             keep the vault on a local disk"
        );
    }
}

/// `path` itself if it exists, else its nearest existing ancestor, canonicalized (a
/// mount table lists real, symlink-free paths). `None` only if canonicalization fails
/// (e.g. a permissions error), since the filesystem root always exists.
fn nearest_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut candidate = path;
    loop {
        if candidate.exists() {
            return std::fs::canonicalize(candidate).ok();
        }
        candidate = candidate.parent()?;
    }
}

/// The fstype of the mount whose mount point is the longest prefix of `path`, parsed
/// from `/proc/mounts`-format text: whitespace-separated `device mount_point fstype
/// ...` lines, with octal `\NNN` escapes (e.g. `\040` for a space) in the mount point.
fn mount_fstype(table: &str, path: &Path) -> Option<String> {
    let mut best: Option<(usize, &str)> = None;
    for line in table.lines() {
        let mut fields = line.split_whitespace();
        let (Some(_device), Some(raw_mount_point), Some(fstype)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let mount_point = PathBuf::from(unescape_octal(raw_mount_point));
        if !path.starts_with(&mount_point) {
            continue;
        }
        let len = mount_point.as_os_str().len();
        if best.is_none_or(|(best_len, _)| len > best_len) {
            best = Some((len, fstype));
        }
    }
    best.map(|(_, fstype)| fstype.to_owned())
}

/// Unescapes `/proc/mounts`-style octal escapes (`\040` -> space) in a mount point; any
/// other backslash sequence is left as-is.
fn unescape_octal(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let digits: String = chars.as_str().chars().take(3).collect();
        if digits.len() == 3
            && digits.chars().all(|d| d.is_digit(8))
            && let Ok(value) = u8::from_str_radix(&digits, 8)
        {
            out.push(value as char);
            for _ in 0..3 {
                chars.next();
            }
        } else {
            out.push('\\');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r"proc /proc proc rw,nosuid,nodev,noexec 0 0
sda1 / ext4 rw,relatime 0 0
server:/export /mnt/data nfs4 rw,vers=4.2 0 0
//server/share /mnt/win\040share cifs rw 0 0
tmpfs /mnt/data/scratch tmpfs rw 0 0
";

    #[test]
    fn longest_matching_mount_wins_over_a_shorter_parent() {
        assert_eq!(
            mount_fstype(FIXTURE, Path::new("/mnt/data/file.db")),
            Some("nfs4".to_owned())
        );
        assert_eq!(
            mount_fstype(FIXTURE, Path::new("/mnt/data/scratch/file.db")),
            Some("tmpfs".to_owned())
        );
    }

    #[test]
    fn the_root_mount_matches_anything_outside_a_more_specific_mount() {
        assert_eq!(
            mount_fstype(FIXTURE, Path::new("/home/user/vault.db")),
            Some("ext4".to_owned())
        );
    }

    #[test]
    fn a_mount_point_with_an_octal_escaped_space_is_matched() {
        assert_eq!(
            mount_fstype(FIXTURE, Path::new("/mnt/win share/file.db")),
            Some("cifs".to_owned())
        );
    }

    #[test]
    fn an_unmatched_path_finds_no_mount() {
        assert_eq!(mount_fstype("", Path::new("/anything")), None);
    }
}
