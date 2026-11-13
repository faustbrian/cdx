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
        assert!(stdout.contains("Global Claude Code and Codex conversation picker"));
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
                "--claude-projects-path",
                &temp_dir.path().join("missing-claude").display().to_string(),
                "--agent",
                "codex",
                "--dry-run",
            ])
            .output();
        let Ok(output) = output else {
            unreachable!("empty database command should run");
        };

        assert!(!output.status.success());

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("no Claude or Codex conversations found"));
    }

    #[test]
    fn dry_run_resume_works_with_fake_terminal() {
        for allow_daemon in [false, true] {
            let temp_dir = TestDir::new("dry-run-terminal");
            let db_path = temp_dir.path().join("state.sqlite");
            let codex_path = temp_dir.path().join("fake codex.sh");
            let marker_path = temp_dir.path().join("invoked");
            write_executable(
                &codex_path,
                &format!("#!/bin/sh\ntouch '{}'\n", marker_path.display()),
            );
            create_threads_db_with_single_conversation(&db_path, temp_dir.path());
            let mut command = Command::new(cdx_bin());
            command.env("CDX_TEST_FAKE_TERMINAL", "1").args([
                "--db-path",
                &db_path.display().to_string(),
                "--session-index-path",
                &temp_dir.path().join("missing.jsonl").display().to_string(),
                "--claude-projects-path",
                &temp_dir.path().join("missing-claude").display().to_string(),
                "--agent",
                "codex",
                "--dry-run",
                "--codex-bin",
                &codex_path.display().to_string(),
            ]);
            if allow_daemon {
                command.arg("--allow-daemon");
            }
            let output = command
                .output()
                .unwrap_or_else(|_| unreachable!("dry-run command should finish"));

            assert!(output.status.success());

            let stdout = String::from_utf8_lossy(&output.stdout);
            let flag = if allow_daemon { "" } else { "--no-daemon " };
            let expected = format!(
                "{} {flag}resume -C {} thread-1\n",
                codex_path.display(),
                temp_dir.path().display()
            );
            assert_eq!(stdout, expected);
            assert!(!marker_path.exists());
        }
    }

    #[test]
    fn subprocess_resume_works_with_fake_terminal() {
        for allow_daemon in [false, true] {
            let temp_dir = TestDir::new("resume-terminal");
            let db_path = temp_dir.path().join("state.sqlite");
            let codex_path = temp_dir.path().join("fake-codex.sh");
            let log_path = temp_dir.path().join("resume.log");

            create_threads_db_with_single_conversation(&db_path, temp_dir.path());
            write_executable(
                &codex_path,
                &format!(
                    "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$@\" > '{}'\n",
                    log_path.display()
                ),
            );

            let mut command = Command::new(cdx_bin());
            command.env("CDX_TEST_FAKE_TERMINAL", "1").args([
                "--db-path",
                &db_path.display().to_string(),
                "--session-index-path",
                &temp_dir.path().join("missing.jsonl").display().to_string(),
                "--claude-projects-path",
                &temp_dir.path().join("missing-claude").display().to_string(),
                "--agent",
                "codex",
                "--codex-bin",
                &codex_path.display().to_string(),
            ]);
            if allow_daemon {
                command.arg("--allow-daemon");
            }
            let output = command
                .output()
                .unwrap_or_else(|_| unreachable!("resume command should finish"));

            assert!(output.status.success());

            let log = fs::read_to_string(&log_path)
                .unwrap_or_else(|_| unreachable!("resume log should be written"));
            let canonical_cwd = fs::canonicalize(temp_dir.path())
                .unwrap_or_else(|_| unreachable!("temp cwd should canonicalize"));
            let flag = if allow_daemon { "" } else { "--no-daemon\n" };
            assert_eq!(
                log,
                format!(
                    "{}\n{flag}resume\n-C\n{}\nthread-1\n",
                    canonical_cwd.display(),
                    temp_dir.path().display()
                )
            );
        }
    }

    #[test]
    fn claude_sessions_resume_from_their_directory() {
        for dry_run in [true, false] {
            let temp_dir = TestDir::new("claude-resume");
            let db_path = temp_dir.path().join("state.sqlite");
            let projects_path = temp_dir.path().join("projects");
            let claude_path = temp_dir.path().join("fake-claude.sh");
            let log_path = temp_dir.path().join("resume.log");
            create_empty_threads_db(&db_path);
            write_claude_session(&projects_path, temp_dir.path(), "session-1");
            write_executable(
                &claude_path,
                &format!(
                    "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$@\" > '{}'\n",
                    log_path.display()
                ),
            );

            let mut command = Command::new(cdx_bin());
            command.env("CDX_TEST_FAKE_TERMINAL", "1").args([
                "--db-path",
                &db_path.display().to_string(),
                "--session-index-path",
                &temp_dir.path().join("missing.jsonl").display().to_string(),
                "--claude-projects-path",
                &projects_path.display().to_string(),
                "--claude-bin",
                &claude_path.display().to_string(),
            ]);
            if dry_run {
                command.arg("--dry-run");
            }
            let output = command
                .output()
                .unwrap_or_else(|_| unreachable!("claude resume command should finish"));

            assert!(output.status.success(), "{output:?}");

            if dry_run {
                assert_eq!(
                    String::from_utf8_lossy(&output.stdout),
                    format!(
                        "cd {} && {} --resume session-1\n",
                        temp_dir.path().display(),
                        claude_path.display()
                    )
                );
                assert!(!log_path.exists());
            } else {
                let log = fs::read_to_string(&log_path)
                    .unwrap_or_else(|_| unreachable!("resume log should be written"));
                let canonical_cwd = fs::canonicalize(temp_dir.path())
                    .unwrap_or_else(|_| unreachable!("temp cwd should canonicalize"));
                assert_eq!(
                    log,
                    format!("{}\n--resume\nsession-1\n", canonical_cwd.display())
                );
            }
        }
    }

    fn write_claude_session(projects_path: &Path, cwd: &Path, session_id: &str) {
        let project_dir = projects_path.join("project");
        fs::create_dir_all(&project_dir)
            .unwrap_or_else(|_| unreachable!("project dir should create"));
        fs::write(
            project_dir.join(format!("{session_id}.jsonl")),
            format!(
                "{{\"type\":\"user\",\"cwd\":\"{}\",\"message\":{{\"content\":\"hello\"}}}}\n",
                cwd.display()
            ),
        )
        .unwrap_or_else(|_| unreachable!("session should write"));
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

    fn create_threads_db_with_single_conversation(path: &Path, cwd: &Path) {
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
                    &cwd.display().to_string(),
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
