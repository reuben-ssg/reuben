//! Last-modified dates for sitemap entries: the last commit that touched a source, else its file time.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;

/// The UTC date of each source's last commit when the index tracks it, otherwise of its modification time.
///
/// A tracked file edited since its last commit still gets the commit date, and one that was never committed gets its file time.
/// Reuben never reads working-tree content through git (no `status`, no diff) and never fetches, so filters, remotes and other repository config can't run commands.
/// A source with neither a commit nor a file time is absent. Git problems of any kind fall back to modification times.
pub fn source_dates(root: &Path, sources: &[&Path]) -> HashMap<PathBuf, Date> {
    let git = GitState::read(root);
    let mut dates = HashMap::new();
    for &source in sources {
        let committed = git.as_ref().and_then(|g| g.committed(source));
        if let Some(date) = committed.or_else(|| mtime_date(source)) {
            dates.insert(source.to_path_buf(), date);
        }
    }
    dates
}

struct GitState {
    top: PathBuf,
    commit_times: HashMap<String, i64>,
    tracked: HashSet<String>,
}

impl GitState {
    fn read(root: &Path) -> Option<GitState> {
        let top = git(root, &["rev-parse", "--show-toplevel"])?;
        let top = PathBuf::from(String::from_utf8(top).ok()?.trim_end_matches('\n'));
        let top = top.canonicalize().ok()?;
        let log = git(
            root,
            &[
                "log",
                "--format=%x01%ct",
                "--name-only",
                "--no-renames",
                "-z",
                "--",
                ".",
            ],
        )?;
        let tracked = git(root, &["ls-files", "-z", "--full-name"])?;
        Some(GitState {
            top,
            commit_times: parse_log(&log),
            tracked: parse_tracked(&tracked),
        })
    }

    fn committed(&self, source: &Path) -> Option<Date> {
        let rel = source.canonicalize().ok()?;
        let rel = rel.strip_prefix(&self.top).ok()?;
        let key = rel
            .components()
            .map(|c| c.as_os_str().to_str())
            .collect::<Option<Vec<_>>>()?
            .join("/");
        if !self.tracked.contains(&key) {
            return None;
        }
        let time = *self.commit_times.get(&key)?;
        Timestamp::from_second(time)
            .ok()
            .map(|t| t.to_zoned(TimeZone::UTC).date())
    }
}

fn git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "log.showSignature=false",
            "-c",
            "core.quotepath=false",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "diff.relative=false",
            // A shipped `.git/` can claim to be a partial clone, and fetching a missing object runs its `core.sshCommand`.
            "-c",
            "protocol.allow=never",
        ])
        .args(args)
        // Empty allows no transport and outranks every config layer, unlike `protocol.allow`.
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// Each commit is a `\x01<commit time>` header followed by its NUL-separated paths; commits are newest first, so the first time seen for a path wins.
fn parse_log(out: &[u8]) -> HashMap<String, i64> {
    let mut times = HashMap::new();
    let mut current = None;
    for token in out.split(|&b| b == 0) {
        if let Some(header) = token.strip_prefix(b"\x01") {
            let (time, rest) = match header.iter().position(|&b| b == b'\n') {
                Some(i) => (&header[..i], &header[i + 1..]),
                None => (header, &header[header.len()..]),
            };
            current = std::str::from_utf8(time)
                .ok()
                .and_then(|t| t.parse::<i64>().ok());
            if let Some(time) = current
                && let Ok(path) = std::str::from_utf8(rest)
                && !path.is_empty()
            {
                times.entry(path.to_string()).or_insert(time);
            }
            continue;
        }
        let path = token.strip_prefix(b"\n").unwrap_or(token);
        if let Some(time) = current
            && !path.is_empty()
            && let Ok(path) = std::str::from_utf8(path)
        {
            times.entry(path.to_string()).or_insert(time);
        }
    }
    times
}

fn parse_tracked(out: &[u8]) -> HashSet<String> {
    out.split(|&b| b == 0)
        .filter(|p| !p.is_empty())
        .filter_map(|p| std::str::from_utf8(p).ok())
        .map(str::to_string)
        .collect()
}

fn mtime_date(path: &Path) -> Option<Date> {
    let modified: SystemTime = std::fs::metadata(path).ok()?.modified().ok()?;
    let timestamp = Timestamp::try_from(modified).ok()?;
    Some(timestamp.to_zoned(TimeZone::UTC).date())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use assert_fs::TempDir;
    use jiff::civil::date;

    use super::{mtime_date, parse_log, source_dates};

    fn git_available() -> bool {
        let ok = Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success());
        if !ok {
            eprintln!("skipping: git is not available");
        }
        ok
    }

    fn run_git(dir: &Path, args: &[&str], commit_date: Option<&str>) {
        let mut cmd = Command::new("git");
        cmd.arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1");
        if let Some(d) = commit_date {
            cmd.env("GIT_COMMITTER_DATE", d).env("GIT_AUTHOR_DATE", d);
        }
        let out = cmd.output().unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    fn commit_all(dir: &Path, when: &str) {
        run_git(dir, &["add", "."], None);
        run_git(dir, &["commit", "-q", "-m", "c"], Some(when));
    }

    fn one(root: &Path, source: &Path) -> Option<jiff::civil::Date> {
        source_dates(root, &[source]).get(source).copied()
    }

    #[test]
    fn lastmod_from_commit() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("content/a.md");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "x").unwrap();
        run_git(tmp.path(), &["init", "-q"], None);
        commit_all(tmp.path(), "2024-03-05T12:00:00Z");
        assert_eq!(one(tmp.path(), &file), Some(date(2024, 3, 5)));
    }

    #[test]
    fn lastmod_uses_newest_commit() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.md");
        fs::write(&file, "x").unwrap();
        run_git(tmp.path(), &["init", "-q"], None);
        commit_all(tmp.path(), "2024-03-05T12:00:00Z");
        fs::write(&file, "y").unwrap();
        commit_all(tmp.path(), "2024-06-07T23:30:00Z");
        assert_eq!(one(tmp.path(), &file), Some(date(2024, 6, 7)));
    }

    #[test]
    fn lastmod_modified_keeps_commit_date() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.md");
        fs::write(&file, "x").unwrap();
        run_git(tmp.path(), &["init", "-q"], None);
        commit_all(tmp.path(), "2024-03-05T12:00:00Z");
        fs::write(&file, "edited").unwrap();
        // `git status` would be needed to notice the edit, and it can run repository-configured filters.
        assert_eq!(one(tmp.path(), &file), Some(date(2024, 3, 5)));
    }

    #[test]
    fn lastmod_added_never_committed_uses_mtime() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let first = tmp.path().join("first.md");
        fs::write(&first, "x").unwrap();
        run_git(tmp.path(), &["init", "-q"], None);
        commit_all(tmp.path(), "2024-03-05T12:00:00Z");
        let added = tmp.path().join("added.md");
        fs::write(&added, "y").unwrap();
        run_git(tmp.path(), &["add", "added.md"], None);
        assert_eq!(one(tmp.path(), &added), mtime_date(&added));
    }

    #[test]
    fn lastmod_untracked_uses_mtime() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        run_git(tmp.path(), &["init", "-q"], None);
        let file = tmp.path().join("a.md");
        fs::write(&file, "x").unwrap();
        assert_eq!(one(tmp.path(), &file), mtime_date(&file));
        assert!(one(tmp.path(), &file).is_some());
    }

    #[test]
    fn lastmod_no_git_uses_mtime() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.md");
        fs::write(&file, "x").unwrap();
        assert_eq!(one(tmp.path(), &file), mtime_date(&file));
        assert!(one(tmp.path(), &file).is_some());
    }

    #[test]
    fn lastmod_missing_source_is_absent() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(one(tmp.path(), &tmp.path().join("gone.md")), None);
    }

    #[test]
    fn lastmod_site_in_repo_subfolder() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let site = tmp.path().join("docs/site");
        let file = site.join("content/a.md");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "x").unwrap();
        let other = tmp.path().join("other.md");
        fs::write(&other, "x").unwrap();
        run_git(tmp.path(), &["init", "-q"], None);
        commit_all(tmp.path(), "2023-01-02T12:00:00Z");
        assert_eq!(one(&site, &file), Some(date(2023, 1, 2)));
    }

    #[test]
    fn lastmod_site_in_repo_subfolder_ignores_diff_relative() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let site = tmp.path().join("docs/site");
        let file = site.join("content/a.md");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "x").unwrap();
        run_git(tmp.path(), &["init", "-q"], None);
        commit_all(tmp.path(), "2023-01-02T12:00:00Z");
        run_git(tmp.path(), &["config", "diff.relative", "true"], None);
        assert_eq!(one(&site, &file), Some(date(2023, 1, 2)));
    }

    #[test]
    fn lastmod_handles_unusual_file_names() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("caf\u{e9} \"q\".md");
        fs::write(&file, "x").unwrap();
        run_git(tmp.path(), &["init", "-q"], None);
        commit_all(tmp.path(), "2022-05-06T12:00:00Z");
        assert_eq!(one(tmp.path(), &file), Some(date(2022, 5, 6)));
    }

    #[test]
    fn parse_log_skips_commits_without_files() {
        let out = b"\x011700000000\0\x011600000000\0\nf.md\0d/g.md\0\x011500000000\0\ng.md\0";
        let map = parse_log(out);
        assert_eq!(map.len(), 3, "{map:?}");
        assert_eq!(map["f.md"], 1_600_000_000);
        assert_eq!(map["d/g.md"], 1_600_000_000);
        assert_eq!(map["g.md"], 1_500_000_000);
    }

    #[cfg(unix)]
    #[test]
    fn lastmod_ignores_repository_fsmonitor() {
        use std::os::unix::fs::PermissionsExt;

        if !git_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        let file = repo.join("a.md");
        fs::write(&file, "x").unwrap();
        run_git(&repo, &["init", "-q"], None);
        commit_all(&repo, "2024-03-05T12:00:00Z");
        let marker = tmp.path().join("ran");
        let script = tmp.path().join("hook.sh");
        fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        run_git(
            &repo,
            &["config", "core.fsmonitor", script.to_str().unwrap()],
            None,
        );
        assert_eq!(one(&repo, &file), Some(date(2024, 3, 5)));
        assert!(!marker.exists(), "repository config ran a command");
    }

    #[cfg(unix)]
    #[test]
    fn lastmod_never_lazy_fetches_missing_objects() {
        use std::os::unix::fs::PermissionsExt;

        if !git_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        let file = repo.join("content/a.md");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "x").unwrap();
        run_git(&repo, &["init", "-q"], None);
        commit_all(&repo, "2024-03-05T12:00:00Z");
        let old_tree = git_output(&repo, &["rev-parse", "HEAD:content"]);
        fs::write(&file, "y").unwrap();
        commit_all(&repo, "2024-06-07T12:00:00Z");
        let (dir, name) = old_tree.split_at(2);
        fs::remove_file(repo.join(".git/objects").join(dir).join(name)).unwrap();

        let marker = tmp.path().join("ran");
        let script = tmp.path().join("ssh.sh");
        fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        for (key, value) in [
            ("core.repositoryformatversion", "1"),
            ("extensions.partialClone", "origin"),
            ("remote.origin.url", "ssh://git@example.invalid/repo.git"),
            ("remote.origin.promisor", "true"),
            // A per-protocol key outranks `protocol.allow`.
            ("protocol.ssh.allow", "always"),
            ("core.sshCommand", script.to_str().unwrap()),
        ] {
            run_git(&repo, &["config", key, value], None);
        }
        let _ = source_dates(&repo, &[file.as_path()]);
        assert!(!marker.exists(), "a lazy fetch ran core.sshCommand");
    }

    fn git_output(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    #[test]
    fn lastmod_rename_committed_and_uncommitted() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let old = tmp.path().join("old.md");
        fs::write(&old, "some longer content so the rename is detected\n").unwrap();
        run_git(tmp.path(), &["init", "-q"], None);
        commit_all(tmp.path(), "2024-03-05T12:00:00Z");
        run_git(tmp.path(), &["mv", "old.md", "new.md"], None);
        let new = tmp.path().join("new.md");
        assert_eq!(one(tmp.path(), &new), mtime_date(&new));
        run_git(
            tmp.path(),
            &["commit", "-q", "-m", "mv"],
            Some("2025-01-02T12:00:00Z"),
        );
        assert_eq!(one(tmp.path(), &new), Some(date(2025, 1, 2)));
    }

    #[test]
    fn lastmod_staged_edit_keeps_commit_date() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.md");
        fs::write(&file, "x").unwrap();
        run_git(tmp.path(), &["init", "-q"], None);
        commit_all(tmp.path(), "2024-03-05T12:00:00Z");
        fs::write(&file, "edited").unwrap();
        run_git(tmp.path(), &["add", "a.md"], None);
        // Same trade-off as an unstaged edit: only the index and log are read, never the working tree through git.
        assert_eq!(one(tmp.path(), &file), Some(date(2024, 3, 5)));
    }

    #[cfg(unix)]
    #[test]
    fn lastmod_ignores_repository_clean_filter() {
        use std::os::unix::fs::PermissionsExt;

        if !git_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let repo = tmp.path().join("repo");
        let file = repo.join("content/a.md");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "x").unwrap();
        fs::write(repo.join(".gitattributes"), "*.md filter=evil\n").unwrap();
        run_git(&repo, &["init", "-q"], None);
        commit_all(&repo, "2024-03-05T12:00:00Z");
        let marker = tmp.path().join("ran");
        let script = tmp.path().join("clean.sh");
        fs::write(
            &script,
            format!("#!/bin/sh\ntouch '{}'\ncat\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        run_git(
            &repo,
            &["config", "filter.evil.clean", script.to_str().unwrap()],
            None,
        );
        let _ = fs::remove_file(&marker);
        fs::write(&file, "x").unwrap();
        assert!(one(&repo, &file).is_some());
        assert!(!marker.exists(), "repository config ran a clean filter");
    }
}
