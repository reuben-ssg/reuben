//! The built site held in memory, and the atomic swap that puts it on disk.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

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
pub fn write_atomic(output: &Output, out: &Path) -> Result<(), Error> {
    let (staging, old) = sibling_dirs(out)?;
    check_out_dir(out)?;
    let result = stage(output, &staging).and_then(|()| swap(&staging, out, &old));
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

fn stage(output: &Output, staging: &Path) -> Result<(), Error> {
    remove_dir(staging)?;
    fs::create_dir_all(staging).map_err(|e| Error::new(staging, e))?;
    for (rel, file) in &output.files {
        let dest = staging.join(rel);
        let written = dest
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| match file {
                OutputFile::Bytes(bytes) => fs::write(&dest, bytes),
                OutputFile::Copy(src) => fs::copy(src, &dest).map(|_| ()),
            });
        written.map_err(|e| Error::new(&dest, e))?;
    }
    let marker = staging.join(OUTPUT_MARKER);
    fs::write(&marker, "").map_err(|e| Error::new(&marker, e))
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
    let _ = fs::remove_dir_all(old);
    Ok(())
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
        assert!(matches!(err.kind, ErrorKind::UnsafeOutputDir));
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
        assert!(matches!(err.kind, ErrorKind::Io(_)));
    }
}
