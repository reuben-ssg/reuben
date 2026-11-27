//! `reuben new`: writes a minimal site into a folder.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const REUBEN_TOML: &str = include_str!("new/reuben.toml.txt");
const INDEX_MD: &str = include_str!("new/index.md.txt");
const GITIGNORE: &str = include_str!("new/gitignore.txt");
const PACKAGE_JSON: &str = include_str!("new/package.json.txt");

#[derive(Debug, Clone, Copy, Default)]
pub struct NewOptions {
    pub npm: bool,
}

#[derive(Debug)]
pub struct Created {
    pub root: PathBuf,
    /// Relative to `root`, in creation order.
    pub files: Vec<PathBuf>,
}

#[derive(Debug)]
pub enum NewError {
    NotEmpty(PathBuf),
    NotADirectory(PathBuf),
    Io { path: PathBuf, source: io::Error },
}

impl fmt::Display for NewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotEmpty(p) => write!(
                f,
                "`{}` already exists and isn't empty: choose a new folder or empty this one",
                p.display()
            ),
            Self::NotADirectory(p) => write!(f, "`{}` is a file, not a folder", p.display()),
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for NewError {}

/// The target folder's name exactly as typed; `.` and `..` resolve to the real folder's name.
pub fn site_title(target: &Path) -> String {
    let named = target.file_name().map(|n| n.to_string_lossy().into_owned());
    if let Some(n) = named {
        return n;
    }
    let resolved = fs::canonicalize(target).ok();
    resolved
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "My site".to_string())
}

/// A valid npm package name: lowercase `a-z0-9._-`, starting with a letter or digit.
pub fn npm_name(title: &str) -> String {
    let mapped: String = title
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = mapped.trim_start_matches(['.', '_', '-']);
    if trimmed.is_empty() {
        "site".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn scaffold(target: &Path, opts: &NewOptions) -> Result<Created, NewError> {
    scaffold_with(target, opts, &mut |path, text| fs::write(path, text))
}

fn io_err(path: &Path, source: io::Error) -> NewError {
    NewError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn toml_string(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

fn files_for(target: &Path, opts: &NewOptions) -> Vec<(&'static str, String)> {
    let title = site_title(target);
    let mut ignore = GITIGNORE.to_string();
    if opts.npm {
        ignore.push_str("node_modules/\n");
    }
    let mut files = vec![
        (
            "reuben.toml",
            REUBEN_TOML.replace("{title}", &toml_string(&title)),
        ),
        ("content/index.md", INDEX_MD.to_string()),
        (".gitignore", ignore),
    ];
    if opts.npm {
        let package = PACKAGE_JSON
            .replace("{name}", &npm_name(&title))
            .replace("{version}", env!("CARGO_PKG_VERSION"));
        files.push(("package.json", package));
    }
    files
}

/// `write` is injectable so tests can fail partway through.
fn scaffold_with(
    target: &Path,
    opts: &NewOptions,
    write: &mut dyn FnMut(&Path, &str) -> io::Result<()>,
) -> Result<Created, NewError> {
    match fs::metadata(target) {
        Ok(m) if !m.is_dir() => return Err(NewError::NotADirectory(target.to_path_buf())),
        Ok(_) => {
            let mut entries = fs::read_dir(target).map_err(|e| io_err(target, e))?;
            if entries.next().is_some() {
                return Err(NewError::NotEmpty(target.to_path_buf()));
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_err(target, e)),
    }

    let mut made_dirs: Vec<PathBuf> = Vec::new();
    let mut made_files: Vec<PathBuf> = Vec::new();
    if let Err(e) = write_all(target, opts, write, &mut made_dirs, &mut made_files) {
        for f in made_files.iter().rev() {
            let _ = fs::remove_file(f);
        }
        for d in made_dirs.iter().rev() {
            let _ = fs::remove_dir(d);
        }
        return Err(e);
    }
    let files = made_files
        .iter()
        .filter_map(|f| f.strip_prefix(target).ok().map(Path::to_path_buf))
        .collect();
    Ok(Created {
        root: target.to_path_buf(),
        files,
    })
}

fn write_all(
    target: &Path,
    opts: &NewOptions,
    write: &mut dyn FnMut(&Path, &str) -> io::Result<()>,
    made_dirs: &mut Vec<PathBuf>,
    made_files: &mut Vec<PathBuf>,
) -> Result<(), NewError> {
    let mut missing = Vec::new();
    let mut probe = Some(target);
    while let Some(p) = probe.filter(|p| !p.as_os_str().is_empty() && !p.exists()) {
        missing.push(p.to_path_buf());
        probe = p.parent();
    }
    for dir in missing.into_iter().rev() {
        fs::create_dir(&dir).map_err(|e| io_err(&dir, e))?;
        made_dirs.push(dir);
    }

    for (rel, text) in files_for(target, opts) {
        let path = target.join(rel);
        if let Some(parent) = path.parent().filter(|p| !p.exists()) {
            fs::create_dir(parent).map_err(|e| io_err(parent, e))?;
            made_dirs.push(parent.to_path_buf());
        }
        // Recorded first: a write that fails midway can leave a partial file to clean up.
        made_files.push(path.clone());
        write(&path, &text).map_err(|e| io_err(&path, e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::TempDir;

    fn read(root: &Path, rel: &str) -> String {
        fs::read_to_string(root.join(rel)).unwrap()
    }

    fn sorted(c: &Created) -> Vec<String> {
        let mut v: Vec<String> = c
            .files
            .iter()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        v.sort();
        v
    }

    #[test]
    fn creates_exactly_the_minimal_tree() {
        let tmp = TempDir::new().unwrap();
        let c = scaffold(&tmp.path().join("site"), &NewOptions::default()).unwrap();
        assert_eq!(
            sorted(&c),
            [".gitignore", "content/index.md", "reuben.toml"]
        );
        assert!(c.root.join("reuben.toml").is_file());
    }

    #[test]
    fn npm_adds_a_pinned_package_json() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("my site");
        let c = scaffold(&root, &NewOptions { npm: true }).unwrap();
        assert!(sorted(&c).contains(&"package.json".to_string()));
        let v: serde_json::Value = serde_json::from_str(&read(&root, "package.json")).unwrap();
        assert_eq!(v["private"], true);
        assert_eq!(v["name"], "my-site");
        assert_eq!(
            v["devDependencies"]["@reuben-ssg/reuben"],
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(v["scripts"]["dev"], "reuben serve");
        assert_eq!(v["scripts"]["build"], "reuben build");
        assert_eq!(v["scripts"]["check"], "reuben check");
        assert!(read(&root, ".gitignore").contains("node_modules/"));
    }

    #[test]
    fn gitignore_covers_dist_and_build_folders() {
        let tmp = TempDir::new().unwrap();
        let plain = tmp.path().join("a");
        scaffold(&plain, &NewOptions::default()).unwrap();
        let g = read(&plain, ".gitignore");
        let lines: Vec<&str> = g.lines().collect();
        assert_eq!(
            lines,
            [
                "dist/",
                ".dist.reuben-staging/",
                ".dist.reuben-old/",
                ".dist.reuben-output"
            ]
        );
        let npm = tmp.path().join("b");
        scaffold(&npm, &NewOptions { npm: true }).unwrap();
        assert_eq!(
            read(&npm, ".gitignore").lines().last(),
            Some("node_modules/")
        );
    }

    #[test]
    fn reuben_toml_parses_and_titles_from_folder() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("my site");
        scaffold(&root, &NewOptions::default()).unwrap();
        let v: toml::Table = read(&root, "reuben.toml").parse().unwrap();
        assert_eq!(v["title"].as_str(), Some("my site"));
        assert_eq!(v["base_url"].as_str(), Some("https://example.com/"));
    }

    #[test]
    fn npm_name_falls_back_when_nothing_is_usable() {
        assert_eq!(npm_name("..."), "site");
        assert_eq!(npm_name("日本語"), "site");
        assert_eq!(npm_name(""), "site");
    }

    // Windows can't create a folder named with double quotes.
    #[cfg(unix)]
    #[test]
    fn quotes_and_unicode_titles_stay_valid() {
        let tmp = TempDir::new().unwrap();
        let name = "My \"Café\"";
        let root = tmp.path().join(name);
        scaffold(&root, &NewOptions { npm: true }).unwrap();
        let v: toml::Table = read(&root, "reuben.toml").parse().unwrap();
        assert_eq!(v["title"].as_str(), Some(name));
        let n = npm_name(name);
        assert!(!n.is_empty());
        let mut chars = n.chars();
        assert!(
            chars
                .next()
                .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        );
        assert!(chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c)));
        assert_eq!(npm_name("my-site"), "my-site");
        let j: serde_json::Value = serde_json::from_str(&read(&root, "package.json")).unwrap();
        assert_eq!(j["name"], n.as_str());
    }

    #[test]
    fn refuses_a_non_empty_folder_and_writes_nothing() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("keep.txt"), "x").unwrap();
        let err = scaffold(tmp.path(), &NewOptions::default()).unwrap_err();
        assert!(matches!(err, NewError::NotEmpty(_)));
        assert!(err.to_string().contains("already exists and isn't empty"));
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[test]
    fn refuses_a_file_target() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("f");
        fs::write(&file, "x").unwrap();
        let err = scaffold(&file, &NewOptions::default()).unwrap_err();
        assert!(matches!(err, NewError::NotADirectory(_)));
        assert!(err.to_string().ends_with("is a file, not a folder"));
        assert_eq!(fs::read_to_string(&file).unwrap(), "x");
    }

    #[test]
    fn dot_target_in_an_empty_folder_works() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("here");
        fs::create_dir(&dir).unwrap();
        let c = scaffold(&dir.join("."), &NewOptions::default()).unwrap();
        assert_eq!(c.files.len(), 3);
        assert!(dir.join("reuben.toml").is_file());
        let v: toml::Table = read(&dir, "reuben.toml").parse().unwrap();
        assert_eq!(v["title"].as_str(), Some("here"));
        let cwd = std::env::current_dir().unwrap();
        let expected = cwd.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(site_title(Path::new(".")), expected);
    }

    #[test]
    fn missing_parents_are_created() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("a/b/site");
        scaffold(&root, &NewOptions::default()).unwrap();
        assert!(root.join("content/index.md").is_file());
    }

    #[test]
    fn failure_removes_only_what_it_created() {
        let tmp = TempDir::new().unwrap();
        let pre = tmp.path().join("pre");
        fs::create_dir(&pre).unwrap();
        let mut count = 0;
        let mut flaky = |p: &Path, t: &str| {
            count += 1;
            if count == 3 {
                fs::write(p, &t[..t.len() / 2])?;
                Err(io::Error::other("boom"))
            } else {
                fs::write(p, t)
            }
        };
        let err = scaffold_with(&pre, &NewOptions::default(), &mut flaky).unwrap_err();
        assert!(matches!(err, NewError::Io { .. }));
        assert!(pre.is_dir());
        assert_eq!(fs::read_dir(&pre).unwrap().count(), 0);

        let fresh = tmp.path().join("x/y/site");
        let mut count = 0;
        let mut flaky = |p: &Path, t: &str| {
            count += 1;
            if count == 2 {
                fs::write(p, &t[..t.len() / 2])?;
                Err(io::Error::other("boom"))
            } else {
                fs::write(p, t)
            }
        };
        scaffold_with(&fresh, &NewOptions::default(), &mut flaky).unwrap_err();
        assert!(!tmp.path().join("x").exists());
        assert!(pre.is_dir());
    }
}
