//! Last-modified dates for sitemap entries: the last commit that touched a source, else its file time.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;

/// The UTC date of each source's last commit when it is tracked and unmodified, otherwise of its modification time.
///
/// A source with neither is absent. Git problems of any kind fall back to modification times.
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
    dirty: HashSet<String>,
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
                "--format=%x00%ct",
                "--name-only",
                "--no-renames",
                "-z",
                "--",
                ".",
            ],
        )?;
        let status = git(
            root,
            &["status", "--porcelain", "-z", "--untracked-files=all"],
        )?;
        Some(GitState {
            top,
            commit_times: parse_log(&log),
            dirty: parse_status(&status),
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
        if self.dirty.contains(&key) {
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
        ])
        .args(args)
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// Records are `NUL, commit time, NUL, "\n", path NUL, path NUL, ...`; commits are newest first, so the first time seen for a path wins.
fn parse_log(out: &[u8]) -> HashMap<String, i64> {
    let mut times = HashMap::new();
    let mut tokens = out.split(|&b| b == 0);
    let mut current = None;
    while let Some(token) = tokens.next() {
        if token.is_empty() {
            current = tokens
                .next()
                .and_then(|t| std::str::from_utf8(t).ok())
                .and_then(|t| t.parse::<i64>().ok());
            if let Some(time) = current
                && let Some(first) = tokens.next()
                && let Some(path) = path_after_time(first)
            {
                times.entry(path).or_insert(time);
            }
        } else if let Some(time) = current
            && let Ok(path) = std::str::from_utf8(token)
        {
            times.entry(path.to_string()).or_insert(time);
        }
    }
    times
}

fn path_after_time(token: &[u8]) -> Option<String> {
    let token = token.strip_prefix(b"\n").unwrap_or(token);
    if token.is_empty() {
        return None;
    }
    std::str::from_utf8(token).ok().map(str::to_string)
}

fn parse_status(out: &[u8]) -> HashSet<String> {
    let mut dirty = HashSet::new();
    let mut entries = out.split(|&b| b == 0).filter(|e| !e.is_empty());
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        if let Ok(path) = std::str::from_utf8(&entry[3..]) {
            dirty.insert(path.to_string());
        }
        if matches!(entry[0], b'R' | b'C')
            && let Some(Ok(old)) = entries.next().map(std::str::from_utf8)
        {
            dirty.insert(old.to_string());
        }
    }
    dirty
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

    use super::{mtime_date, source_dates};

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
    fn lastmod_modified_uses_mtime() {
        if !git_available() {
            return;
        }
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("a.md");
        fs::write(&file, "x").unwrap();
        run_git(tmp.path(), &["init", "-q"], None);
        commit_all(tmp.path(), "2024-03-05T12:00:00Z");
        fs::write(&file, "edited").unwrap();
        assert_eq!(one(tmp.path(), &file), mtime_date(&file));
        assert_ne!(one(tmp.path(), &file), Some(date(2024, 3, 5)));
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
}
