//! Threads are short-lived: one without a question for the set number of
//! days is deleted, with the Claude Code sessions Handy made for it. A
//! running agent's own session is never touched; only Handy's copies go.

use std::path::{Path, PathBuf};

/// Claude Code's folder: `CLAUDE_CONFIG_DIR`, else `~/.claude`.
pub fn claude_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".claude"))
}

/// The folder name Claude Code keeps a working folder's sessions under:
/// every character but ASCII letters and digits becomes `-`.
pub fn project_folder_name(cwd: &Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Deletes session `id`'s transcript (and its folder of extras) from the
/// sessions kept for `workspace`.
pub fn delete_session(claude_dir: &Path, workspace: &Path, id: &str) {
    // Only ids Handy made, which are UUIDs: never a path.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return;
    }
    let project = claude_dir
        .join("projects")
        .join(project_folder_name(workspace));
    let _ = std::fs::remove_file(project.join(format!("{id}.jsonl")));
    let _ = std::fs::remove_dir_all(project.join(id));
}

/// Whether a thread last used at `last_used` is past keeping.
pub fn expired(last_used: i64, now_ms: i64, keep_days: u32) -> bool {
    keep_days > 0 && now_ms - last_used > i64::from(keep_days) * 24 * 60 * 60 * 1000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_project_folders_like_claude_code() {
        assert_eq!(
            project_folder_name(Path::new(r"C:\Users\bruce.mckay\dev\tts-handy\Handy")),
            "C--Users-bruce-mckay-dev-tts-handy-Handy"
        );
        assert_eq!(
            project_folder_name(Path::new(r"C:\Users\BRUCE~1.MCK\AppData")),
            "C--Users-BRUCE-1-MCK-AppData"
        );
    }

    #[test]
    fn deletes_only_that_session() {
        let root = std::env::temp_dir().join("handy-ask-expiry-test");
        let workspace = Path::new(r"C:\ws\ask-workspace");
        let project = root.join("projects").join(project_folder_name(workspace));
        std::fs::create_dir_all(project.join("aaaa-1111")).unwrap();
        std::fs::write(project.join("aaaa-1111.jsonl"), "x").unwrap();
        std::fs::write(project.join("bbbb-2222.jsonl"), "y").unwrap();

        delete_session(&root, workspace, "aaaa-1111");
        delete_session(&root, workspace, "../bbbb-2222");

        assert!(!project.join("aaaa-1111.jsonl").exists());
        assert!(!project.join("aaaa-1111").exists());
        assert!(project.join("bbbb-2222.jsonl").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn keeps_for_the_set_days() {
        let day = 24 * 60 * 60 * 1000;
        assert!(!expired(0, 7 * day, 7));
        assert!(expired(0, 7 * day + 1, 7));
        assert!(!expired(0, 999 * day, 0), "0 keeps forever");
    }
}
