//! The built site held in memory, and the atomic swap that puts it on disk.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::thread;

use rayon::prelude::*;

use crate::error::{Error, ErrorKind};
use crate::site::OUTPUT_MARKER;

/// One file of the built site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputFile {
    Bytes(Vec<u8>),
    /// Copied from this source path when the output is written.
    Copy(PathBuf),
    /// Already written into the staging directory while rendering. Only `Site::build`'s internal streaming path
    /// produces this; `Site::render` never returns it.
    Staged,
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
///
/// Writes only `Bytes` and `Copy` entries. A `Staged` entry is skipped without an error, and the staging directory
/// starts empty, so that file is missing from `out`; only `Site::build`'s streaming path produces `Staged`.
///
/// The previous output is deleted after the swap. What can't be deleted then is left behind without an error, and
/// deleted before the next swap.
pub fn write_atomic(output: &Output, out: &Path) -> Result<(), Vec<Error>> {
    let (real, _, _, _) = paths(out).map_err(|e| vec![e])?;
    check_out_dir(&real).map_err(|e| vec![e])?;
    let staging = prepare_staging(out).map_err(|e| vec![e])?;
    finish_atomic(output, out, &staging).map(|_| ())
}

/// Empties and creates the staging directory beside `out`, and returns it.
pub(crate) fn prepare_staging(out: &Path) -> Result<PathBuf, Error> {
    let (_, staging, _, _) = paths(out)?;
    remove_dir(&staging)?;
    fs::create_dir_all(&staging).map_err(|e| Error::new(&staging, e))?;
    Ok(staging)
}

/// Writes every file not yet in `staging`, writes the marker beside `out`, then swaps `staging` into `out`. On failure `out` is
/// untouched and `staging` is removed. On success, returns what of the old output couldn't be deleted.
pub(crate) fn finish_atomic(
    output: &Output,
    out: &Path,
    staging: &Path,
) -> Result<Vec<Error>, Vec<Error>> {
    let result = paths(out)
        .map_err(|e| vec![e])
        .and_then(|(real, _, old, marker)| {
            // Rechecked here because `out` may have changed while the site rendered, and the swap deletes it.
            check_out_dir(&real).map_err(|e| vec![e])?;
            stage(output, staging)?;
            // Written before the swap so `out` never exists without its marker. A swap that then fails leaves a marker
            // beside an `out` that was already Reuben's (or didn't exist), which the next build treats the same.
            let identity = fs::metadata(staging)
                .ok()
                .and_then(|m| identity(&m))
                .ok_or_else(|| {
                    let msg =
                        "can't read the staging folder's identity to record in the output marker";
                    vec![Error::new(staging, io::Error::other(msg))]
                })?;
            fs::write(&marker, identity).map_err(|e| vec![Error::new(&marker, e)])?;
            swap(staging, &real, &old).map_err(|e| vec![e])
        });
    if result.is_err() {
        let _ = fs::remove_dir_all(staging);
    }
    result
}

/// The staging directory that pages are written into as they render. It remembers the directories it has made, so
/// pages that share one don't each ask the filesystem for it again.
pub(crate) struct Stage {
    dir: PathBuf,
    made: Mutex<HashSet<PathBuf>>,
}

impl Stage {
    /// `dir` must already exist, as `prepare_staging` leaves it.
    pub(crate) fn new(dir: PathBuf) -> Self {
        let made = Mutex::new(HashSet::from([dir.clone()]));
        Self { dir, made }
    }

    /// Writes one rendered file, creating its directory.
    pub(crate) fn write(&self, rel: &Path, bytes: &[u8]) -> Result<(), Error> {
        let dest = self.dir.join(rel);
        #[cfg(test)]
        crate::site::record_work_thread(&dest);
        if let Some(dir) = dest.parent() {
            self.make_dir(dir)?;
        }
        fs::write(&dest, bytes).map_err(|e| Error::new(&dest, e))
    }

    /// Creates `dir` and its parents unless this stage already has. Two pages may both create a new directory at
    /// once, which `create_dir_all` allows; a directory is remembered only once it exists.
    fn make_dir(&self, dir: &Path) -> Result<(), Error> {
        let made = |d: &Path| {
            self.made
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(d)
        };
        if made(dir) {
            return Ok(());
        }
        #[cfg(test)]
        MKDIRS.lock().unwrap().push(dir.to_path_buf());
        fs::create_dir_all(dir).map_err(|e| Error::new(dir, e))?;
        let mut set = self.made.lock().unwrap_or_else(PoisonError::into_inner);
        for d in dir.ancestors().take_while(|d| *d != self.dir) {
            if !set.insert(d.to_path_buf()) {
                break;
            }
        }
        Ok(())
    }
}

/// Every directory `Stage::write` asked the filesystem to create, for tests that count them.
#[cfg(test)]
static MKDIRS: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// The output's real location (a symlinked output is swapped where it points, so the link keeps pointing at the new
/// build), and its staging, old and marker siblings.
fn paths(out: &Path) -> Result<(PathBuf, PathBuf, PathBuf, PathBuf), Error> {
    let real = if fs::symlink_metadata(out).is_ok_and(|m| m.is_symlink()) {
        fs::canonicalize(out).map_err(|e| Error::new(out, e))?
    } else {
        out.to_path_buf()
    };
    let (staging, old, marker) = siblings(&real)?;
    Ok((real, staging, old, marker))
}

/// Refuses a non-empty directory that no earlier Reuben build created: one with neither the marker beside it nor
/// the 0.3.0 marker inside it.
pub(crate) fn check_out_dir(out: &Path) -> Result<(), Error> {
    let io = |e: io::Error| Error::new(out, e);
    if out.exists() {
        let mut entries = fs::read_dir(out).map_err(io)?;
        if entries.next().is_some() {
            // A symlinked `out` keeps its marker beside the folder it points to, as the swap does.
            let (real, _, _, marker) = paths(out)?;
            let ours = fs::read_to_string(&marker).ok().is_some_and(|recorded| {
                fs::metadata(out)
                    .ok()
                    .and_then(|m| identity(&m))
                    .is_some_and(|now| now == recorded)
            });
            if !ours && !real.join(OUTPUT_MARKER).is_file() {
                return Err(Error::new(out, ErrorKind::UnsafeOutputDir));
            }
        }
    }
    Ok(())
}

/// Names a folder so a marker can tell the folder it was written for from a different one created later at the same
/// path. A rename keeps it, so the staging folder's identity is the output's after the swap. `None` when it can't be
/// read, which makes the folder count as foreign.
fn identity(meta: &fs::Metadata) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(format!("{}:{}", meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        let created = meta.created().ok()?;
        let since = created.duration_since(std::time::UNIX_EPOCH).ok()?;
        Some(format!("{}", since.as_nanos()))
    }
}

/// The staging, old and marker paths beside `out`.
fn siblings(out: &Path) -> Result<(PathBuf, PathBuf, PathBuf), Error> {
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
        parent.join(format!(".{name}.reuben-output")),
    ))
}

/// Writes every file not already staged, collecting every failure rather than stopping at the first.
fn stage(output: &Output, staging: &Path) -> Result<(), Vec<Error>> {
    let mut errors = Vec::new();
    // Each file's destination and parent, joined once.
    let targets: Vec<(PathBuf, PathBuf, &OutputFile)> = output
        .files
        .iter()
        .filter(|(_, file)| !matches!(file, OutputFile::Staged))
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
            #[cfg(test)]
            crate::site::record_work_thread(dest);
            let written = match file {
                OutputFile::Bytes(bytes) => fs::write(dest, bytes).map_err(|e| (dest.clone(), e)),
                OutputFile::Copy(src) => fs::copy(src, dest)
                    .map(|_| ())
                    .map_err(|e| (src.clone(), e)),
                OutputFile::Staged => Ok(()),
            };
            written.err().map(|(path, e)| Error::new(path, e))
        })
        .collect();
    errors.extend(write_errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Swaps `staging` into `out`, then deletes the old output. Returns what couldn't be deleted after the swap, which
/// doesn't fail the build: the new output is already in place.
fn swap(staging: &Path, out: &Path, old: &Path) -> Result<Vec<Error>, Error> {
    // Left by a run that stopped between the swap and the deletion.
    remove_stale(old)?;
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
    Ok(if had_old {
        remove_tree_par(old, DELETE_THREADS)
    } else {
        Vec::new()
    })
}

/// How many threads delete the old output. Deleting isn't build work, so `--jobs` doesn't limit it: with one job a
/// rebuild would wait for a deletion on one thread. More than four delete no faster on APFS.
const DELETE_THREADS: usize = 4;

/// Deletes an old-output folder left from an earlier run with one `remove_dir_all` call on the folder itself, which
/// never follows a symlink at any level, even if the folder is swapped for one meanwhile: it sat on disk where
/// anything could have happened to it. A symlink or file in its place is removed, not followed, and a missing folder
/// is not an error.
fn remove_stale(dir: &Path) -> Result<(), Error> {
    #[cfg(test)]
    if FAIL_REMOVAL.lock().unwrap().iter().any(|p| p == dir) {
        let e = io::Error::from(io::ErrorKind::PermissionDenied);
        return Err(removal_error(dir, e));
    }
    match fs::remove_dir_all(dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) if fs::symlink_metadata(dir).is_ok_and(|m| m.is_file()) => {
            fs::remove_file(dir).map_err(|e| removal_error(dir, e))
        }
        Err(e) => Err(removal_error(dir, e)),
        Ok(()) => Ok(()),
    }
}

/// Deletes the old output this run just renamed aside, removing its top-level entries on up to `threads` threads,
/// and returns every failure in path order. Each subtree goes through `remove_dir_all`, which is safe against a
/// directory being swapped for a symlink mid-delete, and a symlink is removed itself, never followed. The folder
/// was renamed a moment ago from the output this run checked, so reaching entries through its path is safe enough.
fn remove_tree_par(dir: &Path, threads: usize) -> Vec<Error> {
    let mut errors = Vec::new();
    let entries: Vec<fs::DirEntry> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(|entry| entry.map_err(|e| errors.push(removal_error(dir, e))).ok())
            .collect(),
        Err(e) => return vec![removal_error(dir, e)],
    };
    let next = AtomicUsize::new(0);
    let failed = Mutex::new(Vec::new());
    let work = || {
        while let Some(entry) = entries.get(next.fetch_add(1, Ordering::Relaxed)) {
            if let Err(e) = remove_entry(entry) {
                failed
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(e);
            }
        }
    };
    thread::scope(|s| {
        // A thread that can't start leaves its share to the others; this one always works too.
        for _ in 1..threads.min(entries.len()) {
            if thread::Builder::new().spawn_scoped(s, work).is_err() {
                break;
            }
        }
        work();
    });
    errors.extend(failed.into_inner().unwrap_or_else(PoisonError::into_inner));
    // A folder with something left in it can't be removed, and the error for what's left already says why.
    if errors.is_empty()
        && let Err(e) = fs::remove_dir(dir)
    {
        errors.push(removal_error(dir, e));
    }
    errors.sort_by(|a, b| a.path.cmp(&b.path));
    errors
}

fn remove_entry(entry: &fs::DirEntry) -> Result<(), Error> {
    let path = entry.path();
    #[cfg(test)]
    if FAIL_REMOVAL.lock().unwrap().contains(&path) {
        let e = io::Error::from(io::ErrorKind::PermissionDenied);
        return Err(removal_error(&path, e));
    }
    let removed = if entry.file_type().is_ok_and(|t| t.is_dir()) {
        fs::remove_dir_all(&path)
    } else {
        fs::remove_file(&path)
    };
    match removed {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(removal_error(&path, e)),
        _ => Ok(()),
    }
}

fn removal_error(path: &Path, e: io::Error) -> Error {
    let message = format!("can't delete the previous build's output: {e}");
    Error::new(path, io::Error::new(e.kind(), message))
}

/// Paths that `remove_stale` and `remove_entry` refuse to delete, for tests of a failed deletion.
#[cfg(test)]
static FAIL_REMOVAL: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

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
        write_atomic(&one_page(), &tmp.path().join("dist")).unwrap();
        tmp.child("dist/stale.html").write_str("old").unwrap();
        write_atomic(&one_page(), &tmp.path().join("dist")).unwrap();
        let dist = tmp.path().join("dist");
        assert_eq!(std::fs::read_to_string(dist.join("a.html")).unwrap(), "new");
        assert!(tmp.path().join(".dist.reuben-output").is_file());
        assert!(!dist.join(OUTPUT_MARKER).exists());
        assert!(!dist.join("stale.html").exists());
        assert!(!tmp.path().join(".dist.reuben-staging").exists());
        assert!(!tmp.path().join(".dist.reuben-old").exists());
    }

    #[test]
    fn fresh_write_puts_the_marker_beside_out_not_inside() {
        let tmp = TempDir::new().unwrap();
        write_atomic(&one_page(), &tmp.path().join("public")).unwrap();
        assert!(tmp.path().join(".public.reuben-output").is_file());
        let inside: Vec<_> = std::fs::read_dir(tmp.path().join("public"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(inside, ["a.html"]);
        write_atomic(&one_page(), &tmp.path().join("public")).unwrap();
        assert!(tmp.path().join("public/a.html").is_file());
    }

    #[test]
    fn output_from_0_3_0_is_replaced_and_its_inner_marker_dropped() {
        let tmp = TempDir::new().unwrap();
        tmp.child("dist/.reuben-output").write_str("").unwrap();
        tmp.child("dist/old.html").write_str("old").unwrap();
        write_atomic(&one_page(), &tmp.path().join("dist")).unwrap();
        assert!(tmp.path().join("dist/a.html").is_file());
        assert!(!tmp.path().join("dist/old.html").exists());
        assert!(!tmp.path().join("dist/.reuben-output").exists());
        assert!(tmp.path().join(".dist.reuben-output").is_file());
    }

    #[test]
    fn another_outputs_marker_does_not_vouch_for_a_foreign_dir() {
        let tmp = TempDir::new().unwrap();
        write_atomic(&one_page(), &tmp.path().join("dist")).unwrap();
        tmp.child("public/keep.txt").write_str("mine").unwrap();
        let err = write_atomic(&one_page(), &tmp.path().join("public")).unwrap_err();
        assert!(matches!(err[0].kind, ErrorKind::UnsafeOutputDir));
        assert!(tmp.path().join("public/keep.txt").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn unwritable_parent_is_a_clear_error() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let parent = tmp.path().join("ro");
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = write_atomic(&one_page(), &parent.join("dist"));
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        if std::fs::write(parent.join("probe"), "").is_ok() {
            return; // running as a user who ignores permissions
        }
        let errs = result.unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].path.starts_with(&parent));
        assert!(!parent.join("dist").exists());
    }

    #[test]
    fn foreign_dir_made_after_the_output_was_deleted_is_refused() {
        let tmp = TempDir::new().unwrap();
        let dist = tmp.path().join("dist");
        write_atomic(&one_page(), &dist).unwrap();
        std::fs::remove_dir_all(&dist).unwrap();
        tmp.child("dist/keep.txt").write_str("mine").unwrap();
        let err = write_atomic(&one_page(), &dist).unwrap_err();
        assert!(matches!(err[0].kind, ErrorKind::UnsafeOutputDir));
        assert_eq!(
            std::fs::read_to_string(dist.join("keep.txt")).unwrap(),
            "mine"
        );
    }

    #[test]
    fn leftover_marker_with_no_output_folder_is_overwritten() {
        let tmp = TempDir::new().unwrap();
        let dist = tmp.path().join("dist");
        write_atomic(&one_page(), &dist).unwrap();
        std::fs::remove_dir_all(&dist).unwrap();
        write_atomic(&one_page(), &dist).unwrap();
        write_atomic(&one_page(), &dist).unwrap();
        assert!(dist.join("a.html").is_file());
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
    fn stale_old_output_from_a_crash_is_deleted() {
        let tmp = TempDir::new().unwrap();
        // Half deleted: no marker, and a folder of what's left.
        tmp.child(".dist.reuben-old/blog/x/index.html")
            .write_str("left")
            .unwrap();
        tmp.child("dist/.reuben-output").write_str("").unwrap();
        tmp.child("dist/keep.html").write_str("previous").unwrap();
        write_atomic(&one_page(), &tmp.path().join("dist")).unwrap();
        assert!(!tmp.path().join(".dist.reuben-old").exists());
        assert!(tmp.path().join("dist/a.html").is_file());
        assert!(!tmp.path().join("dist/keep.html").exists());
    }

    #[test]
    fn stale_old_output_file_is_deleted() {
        let tmp = TempDir::new().unwrap();
        tmp.child(".dist.reuben-old")
            .write_str("not a folder")
            .unwrap();
        write_atomic(&one_page(), &tmp.path().join("dist")).unwrap();
        assert!(!tmp.path().join(".dist.reuben-old").exists());
        assert!(tmp.path().join("dist/a.html").is_file());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_stale_old_output_is_removed_not_followed() {
        let tmp = TempDir::new().unwrap();
        tmp.child("elsewhere/keep/index.html")
            .write_str("mine")
            .unwrap();
        let old = tmp.path().join(".dist.reuben-old");
        std::os::unix::fs::symlink(tmp.path().join("elsewhere"), &old).unwrap();
        write_atomic(&one_page(), &tmp.path().join("dist")).unwrap();
        assert!(fs::symlink_metadata(&old).is_err());
        assert_eq!(
            fs::read_to_string(tmp.path().join("elsewhere/keep/index.html")).unwrap(),
            "mine"
        );
        assert!(tmp.path().join("dist/a.html").is_file());
    }

    #[test]
    fn custom_output_name_deletes_only_its_own_old_output() {
        let tmp = TempDir::new().unwrap();
        tmp.child(".public.reuben-old/x.html")
            .write_str("x")
            .unwrap();
        tmp.child(".dist.reuben-old/y.html").write_str("y").unwrap();
        write_atomic(&one_page(), &tmp.path().join("public")).unwrap();
        assert!(!tmp.path().join(".public.reuben-old").exists());
        assert!(tmp.path().join(".dist.reuben-old/y.html").is_file());
        assert!(tmp.path().join("public/a.html").is_file());
    }

    #[test]
    fn stale_old_output_that_cant_be_deleted_fails_and_keeps_out() {
        let tmp = TempDir::new().unwrap();
        let (dist, old) = (tmp.path().join("dist"), tmp.path().join(".dist.reuben-old"));
        tmp.child("dist/.reuben-output").write_str("").unwrap();
        tmp.child("dist/keep.html").write_str("previous").unwrap();
        tmp.child(".dist.reuben-old/b/index.html")
            .write_str("")
            .unwrap();
        FAIL_REMOVAL.lock().unwrap().push(old.clone());
        let errs = write_atomic(&one_page(), &dist).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].path, old);
        assert!(
            errs[0]
                .to_string()
                .contains("can't delete the previous build's output")
        );
        assert_eq!(
            fs::read_to_string(dist.join("keep.html")).unwrap(),
            "previous"
        );
        assert!(!dist.join("a.html").exists());
        assert!(!tmp.path().join(".dist.reuben-staging").exists());
    }

    /// A real failure, where the platform allows one: a read-only folder's files can't be deleted.
    #[cfg(unix)]
    #[test]
    fn unwritable_stale_old_output_fails_the_write() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let (dist, old) = (tmp.path().join("dist"), tmp.path().join(".dist.reuben-old"));
        tmp.child("dist/.reuben-output").write_str("").unwrap();
        tmp.child("dist/keep.html").write_str("previous").unwrap();
        tmp.child(".dist.reuben-old/locked/x.html")
            .write_str("")
            .unwrap();
        let locked = old.join("locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
        // Root ignores the permission, so there's nothing to test.
        if fs::write(locked.join("probe"), "").is_ok() {
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }
        let result = write_atomic(&one_page(), &dist);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        let errs = result.unwrap_err();
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].path.starts_with(&old), "{errs:?}");
        assert_eq!(
            fs::read_to_string(dist.join("keep.html")).unwrap(),
            "previous"
        );
        assert!(!tmp.path().join(".dist.reuben-staging").exists());
    }

    #[test]
    fn old_output_left_after_the_swap_is_returned_and_deleted_next_time() {
        let tmp = TempDir::new().unwrap();
        let (dist, old) = (tmp.path().join("dist"), tmp.path().join(".dist.reuben-old"));
        tmp.child("dist/.reuben-output").write_str("").unwrap();
        tmp.child("dist/a/index.html").write_str("").unwrap();
        tmp.child("dist/b/index.html").write_str("").unwrap();
        tmp.child("dist/c.html").write_str("").unwrap();
        FAIL_REMOVAL.lock().unwrap().push(old.join("b"));
        FAIL_REMOVAL.lock().unwrap().push(old.join("c.html"));
        let staging = prepare_staging(&dist).unwrap();
        let left = finish_atomic(&one_page(), &dist, &staging).unwrap();
        let paths: Vec<&Path> = left.iter().map(|e| e.path.as_path()).collect();
        assert_eq!(
            paths,
            [old.join("b").as_path(), old.join("c.html").as_path()]
        );
        assert_eq!(fs::read_to_string(dist.join("a.html")).unwrap(), "new");
        assert!(!old.join("a").exists());
        assert!(old.join("b/index.html").is_file());
        // The next write deletes the rest before it swaps.
        write_atomic(&one_page(), &dist).unwrap();
        assert!(!old.exists());
    }

    #[test]
    fn remove_tree_par_deletes_everything() {
        let tmp = TempDir::new().unwrap();
        for i in 0..40 {
            tmp.child(format!("old/s{i}/p/index.html"))
                .write_str("x")
                .unwrap();
            tmp.child(format!("old/f{i}.html")).write_str("x").unwrap();
        }
        assert!(remove_tree_par(&tmp.path().join("old"), DELETE_THREADS).is_empty());
        assert!(!tmp.path().join("old").exists());
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

    #[test]
    fn finish_atomic_keeps_staged_files_and_writes_the_rest() {
        let tmp = TempDir::new().unwrap();
        tmp.child(".dist.reuben-staging/junk.txt")
            .write_str("junk")
            .unwrap();
        let out = tmp.path().join("dist");
        let staging = prepare_staging(&out).unwrap();
        assert_eq!(staging, tmp.path().join(".dist.reuben-staging"));
        assert!(!staging.join("junk.txt").exists());
        std::fs::create_dir_all(staging.join("blog")).unwrap();
        std::fs::write(staging.join("blog/index.html"), "streamed").unwrap();
        let mut output = one_page();
        output
            .files
            .insert("blog/index.html".into(), OutputFile::Staged);
        finish_atomic(&output, &out, &staging).unwrap();
        assert_eq!(
            std::fs::read_to_string(out.join("blog/index.html")).unwrap(),
            "streamed"
        );
        assert_eq!(std::fs::read_to_string(out.join("a.html")).unwrap(), "new");
        assert!(tmp.path().join(".dist.reuben-output").is_file());
        assert!(!out.join(OUTPUT_MARKER).exists());
        assert!(!staging.exists());
    }

    #[test]
    fn failed_finish_removes_staging_and_keeps_out() {
        let tmp = TempDir::new().unwrap();
        tmp.child("dist/.reuben-output").write_str("").unwrap();
        tmp.child("dist/keep.html").write_str("old").unwrap();
        let out = tmp.path().join("dist");
        let staging = prepare_staging(&out).unwrap();
        std::fs::write(staging.join("page.html"), "streamed").unwrap();
        let mut output = one_page();
        output.files.insert("page.html".into(), OutputFile::Staged);
        let src = tmp.path().join("missing.css");
        output
            .files
            .insert("m.css".into(), OutputFile::Copy(src.clone()));
        let errs = finish_atomic(&output, &out, &staging).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].path, src);
        assert_eq!(
            std::fs::read_to_string(out.join("keep.html")).unwrap(),
            "old"
        );
        assert!(!staging.exists());
    }

    #[test]
    fn finish_atomic_refuses_out_that_turned_foreign() {
        let tmp = TempDir::new().unwrap();
        let out = tmp.path().join("dist");
        std::fs::create_dir_all(&out).unwrap();
        let staging = prepare_staging(&out).unwrap();
        std::fs::write(out.join("keep.txt"), "mine").unwrap();
        let errs = finish_atomic(&one_page(), &out, &staging).unwrap_err();
        assert_eq!(errs.len(), 1);
        assert!(matches!(errs[0].kind, ErrorKind::UnsafeOutputDir));
        assert_eq!(
            std::fs::read_to_string(out.join("keep.txt")).unwrap(),
            "mine"
        );
        assert!(!staging.exists());
        assert!(!tmp.path().join(".dist.reuben-old").exists());
    }

    fn mkdirs_under(dir: &Path) -> usize {
        MKDIRS
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.starts_with(dir))
            .count()
    }

    #[test]
    fn staged_pages_in_one_directory_create_it_once() {
        let tmp = TempDir::new().unwrap();
        let staging = prepare_staging(&tmp.path().join("dist")).unwrap();
        let stage = Stage::new(staging.clone());
        for name in ["a", "b", "c"] {
            let rel = PathBuf::from(format!("blog/{name}.html"));
            stage.write(&rel, name.as_bytes()).unwrap();
        }
        stage.write(Path::new("index.html"), b"home").unwrap();
        assert_eq!(mkdirs_under(&staging), 1);
        for name in ["a", "b", "c"] {
            let got = std::fs::read_to_string(staging.join(format!("blog/{name}.html"))).unwrap();
            assert_eq!(got, name);
        }
        assert_eq!(
            std::fs::read_to_string(staging.join("index.html")).unwrap(),
            "home"
        );
    }

    #[test]
    fn staged_pages_in_nested_new_directories_are_written() {
        let tmp = TempDir::new().unwrap();
        let staging = prepare_staging(&tmp.path().join("dist")).unwrap();
        let stage = Stage::new(staging.clone());
        stage.write(Path::new("a/b/c/index.html"), b"deep").unwrap();
        stage.write(Path::new("a/b/index.html"), b"mid").unwrap();
        stage.write(Path::new("a/index.html"), b"top").unwrap();
        stage.write(Path::new("a/b/d/index.html"), b"sib").unwrap();
        let read = |rel: &str| std::fs::read_to_string(staging.join(rel)).unwrap();
        assert_eq!(read("a/b/c/index.html"), "deep");
        assert_eq!(read("a/b/index.html"), "mid");
        assert_eq!(read("a/index.html"), "top");
        assert_eq!(read("a/b/d/index.html"), "sib");
        // `a/b/c` makes its parents too, so only `a/b/d` needs another.
        assert_eq!(mkdirs_under(&staging), 2);
    }

    #[test]
    fn staged_pages_written_concurrently_into_one_new_directory() {
        let tmp = TempDir::new().unwrap();
        let staging = prepare_staging(&tmp.path().join("dist")).unwrap();
        let stage = Stage::new(staging.clone());
        let names: Vec<String> = (0..64).map(|i| format!("p{i}")).collect();
        let results: Vec<Result<(), Error>> = names
            .par_iter()
            .map(|n| {
                stage.write(
                    &Path::new("shared/deeper").join(format!("{n}.html")),
                    n.as_bytes(),
                )
            })
            .collect();
        assert!(results.iter().all(Result::is_ok));
        for n in &names {
            let got =
                std::fs::read_to_string(staging.join(format!("shared/deeper/{n}.html"))).unwrap();
            assert_eq!(&got, n);
        }
    }

    #[test]
    fn a_staged_page_whose_directory_cant_be_made_names_the_directory_every_time() {
        let tmp = TempDir::new().unwrap();
        let staging = prepare_staging(&tmp.path().join("dist")).unwrap();
        std::fs::write(staging.join("blocked"), "a file").unwrap();
        let stage = Stage::new(staging.clone());
        for name in ["a", "b"] {
            let err = stage
                .write(&Path::new("blocked/x").join(format!("{name}.html")), b"x")
                .unwrap_err();
            assert_eq!(err.path, staging.join("blocked/x"));
            assert!(matches!(err.kind, ErrorKind::Io(_)));
        }
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
