//! Integration coverage for the `cdx` binary entrypoint.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_exits_successfully() {
        let output = Command::new(cdx_bin()).arg("--help").output();
        let Ok(output) = output else {
            unreachable!("help command should run");
        };

        assert!(output.status.success());

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Global Codex conversation picker"));
    }

    #[test]
    fn empty_database_exits_with_clear_error() {
        let temp_dir = TestDir::new("empty-cli");
        let db_path = temp_dir.path().join("state.sqlite");
        create_empty_threads_db(&db_path);

        let output = Command::new(cdx_bin())
            .args([
                "--db-path",
                &db_path.display().to_string(),
                "--session-index-path",
                &temp_dir.path().join("missing.jsonl").display().to_string(),
                "--dry-run",
            ])
            .output();
        let Ok(output) = output else {
            unreachable!("empty database command should run");
        };

        assert!(!output.status.success());

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("no Codex conversations found"));
    }

    #[test]
    fn dry_run_resume_works_with_fake_terminal() {
        let temp_dir = TestDir::new("dry-run-terminal");
        let db_path = temp_dir.path().join("state.sqlite");
        create_threads_db_with_single_conversation(&db_path);
        let output = Command::new(cdx_bin())
            .env("CDX_TEST_FAKE_TERMINAL", "1")
            .args([
                "--db-path",
                &db_path.display().to_string(),
                "--session-index-path",
                &temp_dir.path().join("missing.jsonl").display().to_string(),
                "--dry-run",
            ])
            .output()
            .unwrap_or_else(|_| unreachable!("dry-run command should finish"));

        assert!(output.status.success());

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("codex resume thread-1"));
    }

    #[test]
    fn subprocess_resume_works_with_fake_terminal() {
        let temp_dir = TestDir::new("resume-terminal");
        let db_path = temp_dir.path().join("state.sqlite");
        let codex_path = temp_dir.path().join("fake-codex.sh");
        let log_path = temp_dir.path().join("resume.log");

        create_threads_db_with_single_conversation(&db_path);
        write_executable(
            &codex_path,
            &format!(
                "#!/bin/sh\nprintf '%s %s\\n' \"$1\" \"$2\" > '{}'\n",
                log_path.display()
            ),
        );

        let output = Command::new(cdx_bin())
            .env("CDX_TEST_FAKE_TERMINAL", "1")
            .args([
                "--db-path",
                &db_path.display().to_string(),
                "--session-index-path",
                &temp_dir.path().join("missing.jsonl").display().to_string(),
                "--codex-bin",
                &codex_path.display().to_string(),
            ])
            .output()
            .unwrap_or_else(|_| unreachable!("resume command should finish"));

        assert!(output.status.success());

        let log = fs::read_to_string(&log_path)
            .unwrap_or_else(|_| unreachable!("resume log should be written"));
        assert_eq!(log.trim(), "resume thread-1");
    }

    fn cdx_bin() -> &'static str {
        env!("CARGO_BIN_EXE_cdx")
    }

    fn create_empty_threads_db(path: &Path) {
        let connection = rusqlite::Connection::open(path)
            .unwrap_or_else(|_| unreachable!("sqlite database should open"));
        connection
            .execute_batch(
                "create table threads (
                    id text not null,
                    cwd text not null,
                    title text,
                    preview text,
                    first_user_message text,
                    recency_at_ms integer not null,
                    updated_at_ms integer not null,
                    created_at_ms integer not null,
                    created_at integer not null,
                    archived integer not null
                );",
            )
            .unwrap_or_else(|_| unreachable!("schema should create"));
    }

    fn create_threads_db_with_single_conversation(path: &Path) {
        create_empty_threads_db(path);

        let connection = rusqlite::Connection::open(path)
            .unwrap_or_else(|_| unreachable!("sqlite database should open"));
        connection
            .execute(
                "insert into threads (
                    id, cwd, title, preview, first_user_message, recency_at_ms,
                    updated_at_ms, created_at_ms, created_at, archived
                ) values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                (
                    "thread-1",
                    "/tmp/api",
                    "thread title",
                    "preview body",
                    "first message",
                    2_000_i64,
                    1_500_i64,
                    1_000_i64,
                    1_i64,
                    0_i64,
                ),
            )
            .unwrap_or_else(|_| unreachable!("thread should insert"));
    }

    fn write_executable(path: &Path, contents: &str) {
        fs::write(path, contents).unwrap_or_else(|_| unreachable!("script should write"));

        let metadata = fs::metadata(path)
            .unwrap_or_else(|_| unreachable!("script metadata should be readable"));
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o755);

        fs::set_permissions(path, permissions)
            .unwrap_or_else(|_| unreachable!("script permissions should update"));
    }

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(prefix: &str) -> Self {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0_u128, |duration| duration.as_nanos());
            let path = std::env::temp_dir().join(format!(
                "cdx-cli-tests-{prefix}-{}-{}",
                std::process::id(),
                timestamp
            ));
            fs::create_dir_all(&path).unwrap_or_else(|_| unreachable!("temp dir should create"));

            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _remove_result = fs::remove_dir_all(&self.path);
        }
    }
}
