//! The built site held in memory, and the atomic swap that puts it on disk.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::error::{Error, ErrorKind};
use crate::site::OUTPUT_MARKER;

/// One file of the built site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputFile {
    Bytes(Vec<u8>),
    /// Copied from this source path when the output is written.
    Copy(PathBuf),
}

/// The whole built site, keyed by path relative to the output directory.
#[derive(Debug, Default)]
pub struct Output {
    pub files: BTreeMap<PathBuf, OutputFile>,
}

/// Replaces `out` with `output`, or leaves `out` untouched if anything fails.
///
/// The staging and old directories are siblings of `out` so the renames stay on one filesystem. They sit beside the
/// output, never under `content/` or `static/`, so a site walk can't pick them up. An `out` with no final component
/// (`.`, `/`) has no sibling to stage in and is refused.
pub fn write_atomic(output: &Output, out: &Path) -> Result<(), Vec<Error>> {
    // A symlinked output is swapped at its real location so the link keeps pointing at the new build.
    let real;
    let out = if fs::symlink_metadata(out).is_ok_and(|m| m.is_symlink()) {
        real = fs::canonicalize(out).map_err(|e| vec![Error::new(out, e)])?;
        real.as_path()
    } else {
        out
    };
    let (staging, old) = sibling_dirs(out).map_err(|e| vec![e])?;
    check_out_dir(out).map_err(|e| vec![e])?;
    let result =
        stage(output, &staging).and_then(|()| swap(&staging, out, &old).map_err(|e| vec![e]));
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

/// Refuses a non-empty directory that no earlier Reuben build created.
pub(crate) fn check_out_dir(out: &Path) -> Result<(), Error> {
    let io = |e: io::Error| Error::new(out, e);
    if out.exists() {
        let mut entries = fs::read_dir(out).map_err(io)?;
        if entries.next().is_some() && !out.join(OUTPUT_MARKER).is_file() {
            return Err(Error::new(out, ErrorKind::UnsafeOutputDir));
        }
    }
    Ok(())
}

fn sibling_dirs(out: &Path) -> Result<(PathBuf, PathBuf), Error> {
    let name = out.file_name().map(|n| n.to_string_lossy().into_owned());
    let Some(name) = name else {
        let msg =
            "output directory must have a name and a parent directory to stage in (not `.` or `/`)";
        return Err(Error::new(
            out,
            io::Error::new(io::ErrorKind::InvalidInput, msg),
        ));
    };
    let parent = out.parent().filter(|p| !p.as_os_str().is_empty());
    let parent = parent.unwrap_or(Path::new("."));
    Ok((
        parent.join(format!(".{name}.reuben-staging")),
        parent.join(format!(".{name}.reuben-old")),
    ))
}

/// Writes every file, collecting every failure rather than stopping at the first.
fn stage(output: &Output, staging: &Path) -> Result<(), Vec<Error>> {
    remove_dir(staging).map_err(|e| vec![e])?;
    fs::create_dir_all(staging).map_err(|e| vec![Error::new(staging, e)])?;
    let mut errors = Vec::new();
    // Each file's destination and parent, joined once.
    let targets: Vec<(PathBuf, PathBuf, &OutputFile)> = output
        .files
        .iter()
        .map(|(rel, file)| {
            let dest = staging.join(rel);
            let parent = dest.parent().unwrap_or(staging).to_path_buf();
            (dest, parent, file)
        })
        .collect();
    let dirs: BTreeSet<&Path> = targets
        .iter()
        .map(|(_, parent, _)| parent.as_path())
        .collect();
    for dir in dirs {
        if let Err(e) = fs::create_dir_all(dir) {
            errors.push(Error::new(dir, e));
        }
    }
    // Files whose directory couldn't be made are skipped: the directory's error already explains them.
    let files: Vec<&(PathBuf, PathBuf, &OutputFile)> = if errors.is_empty() {
        targets.iter().collect()
    } else {
        targets
            .iter()
            .filter(|(_, parent, _)| !errors.iter().any(|e| parent.starts_with(&e.path)))
            .collect()
    };
    // Collected in path order, so the errors come out the same on every run.
    let write_errors: Vec<Error> = files
        .par_iter()
        .filter_map(|(dest, _, file)| {
            let written = match file {
                OutputFile::Bytes(bytes) => fs::write(dest, bytes).map_err(|e| (dest.clone(), e)),
                OutputFile::Copy(src) => fs::copy(src, dest)
                    .map(|_| ())
                    .map_err(|e| (src.clone(), e)),
            };
            written.err().map(|(path, e)| Error::new(path, e))
        })
        .collect();
    errors.extend(write_errors);
    let marker = staging.join(OUTPUT_MARKER);
    if let Err(e) = fs::write(&marker, "") {
        errors.push(Error::new(&marker, e));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn swap(staging: &Path, out: &Path, old: &Path) -> Result<(), Error> {
    remove_dir(old)?;
    let had_old = out.exists();
    if had_old {
        fs::rename(out, old).map_err(|e| Error::new(out, e))?;
    }
    if let Err(e) = fs::rename(staging, out) {
        if had_old {
            let _ = fs::rename(old, out);
        }
        return Err(Error::new(out, e));
    }
    // The new output is in place; a failure to delete the old one isn't a failed build.
    remove_tree_par(old);
    Ok(())
}

/// Deletes a directory tree, removing its top-level entries in parallel. Each subtree goes through
/// `remove_dir_all`, which is safe against a directory being swapped for a symlink mid-delete, and a symlink is
/// removed itself, never followed. Best effort: a failure leaves files behind.
fn remove_tree_par(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let entries: Vec<_> = entries.flatten().collect();
    entries.par_iter().for_each(|entry| {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = fs::remove_dir_all(&path);
        } else {
            let _ = fs::remove_file(&path);
        }
    });
    let _ = fs::remove_dir(dir);
}

fn remove_dir(dir: &Path) -> Result<(), Error> {
    match fs::remove_dir_all(dir) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(Error::new(dir, e)),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;

    fn one_page() -> Output {
        let mut output = Output::default();
        output
            .files
            .insert("a.html".into(), OutputFile::Bytes(b"new".to_vec()));
        output
    }

    #[test]
    fn write_atomic_replaces_and_drops_stale_files() {
        let tmp = TempDir::new().unwrap();
        tmp.child("dist/.reuben-output").write_str("").unwrap();
        tmp.child("dist/stale.html").write_str("old").unwrap();
        write_atomic(&one_page(), &tmp.path().join("dist")).unwrap();
        let dist = tmp.path().join("dist");
        assert_eq!(std::fs::read_to_string(dist.join("a.html")).unwrap(), "new");
        assert!(dist.join(OUTPUT_MARKER).is_file());
        assert!(!dist.join("stale.html").exists());
        assert!(!tmp.path().join(".dist.reuben-staging").exists());
        assert!(!tmp.path().join(".dist.reuben-old").exists());
    }

    #[test]
    fn write_atomic_refuses_foreign_dir() {
        let tmp = TempDir::new().unwrap();
        tmp.child("dist/keep.txt").write_str("mine").unwrap();
        let err = write_atomic(&one_page(), &tmp.path().join("dist")).unwrap_err();
        assert!(matches!(err[0].kind, ErrorKind::UnsafeOutputDir));
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("dist/keep.txt")).unwrap(),
            "mine"
        );
        assert!(!tmp.path().join("dist/a.html").exists());
    }

    #[test]
    fn leftover_staging_is_cleaned() {
        let tmp = TempDir::new().unwrap();
        tmp.child(".dist.reuben-staging/junk.txt")
            .write_str("junk")
            .unwrap();
        write_atomic(&one_page(), &tmp.path().join("dist")).unwrap();
        assert!(!tmp.path().join(".dist.reuben-staging").exists());
        assert!(!tmp.path().join("dist/junk.txt").exists());
        assert!(tmp.path().join("dist/a.html").is_file());
    }

    #[test]
    fn nameless_out_is_refused() {
        let err = write_atomic(&one_page(), Path::new(".")).unwrap_err();
        assert!(matches!(err[0].kind, ErrorKind::Io(_)));
    }

    #[test]
    fn missing_copy_source_names_the_source_and_changes_nothing() {
        let tmp = TempDir::new().unwrap();
        tmp.child("dist/.reuben-output").write_str("").unwrap();
        tmp.child("dist/keep.html").write_str("old").unwrap();
        let mut output = one_page();
        let src = tmp.path().join("missing.css");
        output
            .files
            .insert("m.css".into(), OutputFile::Copy(src.clone()));
        let errs = write_atomic(&output, &tmp.path().join("dist")).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].path, src);
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("dist/keep.html")).unwrap(),
            "old"
        );
        assert!(!tmp.path().join(".dist.reuben-staging").exists());
    }

    #[test]
    fn every_missing_copy_source_is_reported_in_path_order() {
        let tmp = TempDir::new().unwrap();
        let mut output = one_page();
        let (a, b) = (
            tmp.path().join("a-missing.css"),
            tmp.path().join("b-missing.css"),
        );
        output
            .files
            .insert("css/b.css".into(), OutputFile::Copy(b.clone()));
        output
            .files
            .insert("css/a.css".into(), OutputFile::Copy(a.clone()));
        let errs = write_atomic(&output, &tmp.path().join("dist")).unwrap_err();
        let paths: Vec<&Path> = errs.iter().map(|e| e.path.as_path()).collect();
        assert_eq!(paths, [a.as_path(), b.as_path()]);
        assert!(!tmp.path().join("dist").exists());
        assert!(!tmp.path().join(".dist.reuben-staging").exists());
    }

    #[cfg(unix)]
    #[test]
    fn old_output_symlink_is_removed_not_followed() {
        let tmp = TempDir::new().unwrap();
        tmp.child("outside/keep.txt").write_str("mine").unwrap();
        tmp.child("dist/.reuben-output").write_str("").unwrap();
        tmp.child("dist/sub/old.html").write_str("old").unwrap();
        std::os::unix::fs::symlink(tmp.path().join("outside"), tmp.path().join("dist/link"))
            .unwrap();
        write_atomic(&one_page(), &tmp.path().join("dist")).unwrap();
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("outside/keep.txt")).unwrap(),
            "mine"
        );
        assert!(!tmp.path().join(".dist.reuben-old").exists());
        assert!(!tmp.path().join("dist/link").exists());
    }
}
