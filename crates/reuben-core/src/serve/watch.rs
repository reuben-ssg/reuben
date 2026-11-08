//! File watching for `reuben serve`: which paths to watch, event filtering, and debouncing bursts of events.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::mem;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::event::ModifyKind;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::config::Config;

/// Site folders whose whole tree is watched.
const WATCHED_DIRS: [&str; 5] = ["content", "includes", "layouts", "data", "static"];

/// Existing paths a change to which should rebuild the site: the site folders, the theme folder,
/// `reuben.toml`, `reuben.<env>.toml` and the files `include` pulled in.
pub fn watch_paths(root: &Path, config: &Config) -> Vec<PathBuf> {
    let mut paths = BTreeSet::new();
    for dir in WATCHED_DIRS {
        paths.insert(root.join(dir));
    }
    paths.extend(config.theme.clone());
    paths.insert(root.join("reuben.toml"));
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            if is_env_config(&entry.file_name().to_string_lossy()) {
                paths.insert(entry.path());
            }
        }
    }
    paths.extend(config.sources.values().cloned());
    paths.into_iter().filter(|p| p.exists()).collect()
}

fn is_env_config(name: &str) -> bool {
    name.strip_prefix("reuben.")
        .is_some_and(|rest| rest.len() > ".toml".len() && rest.ends_with(".toml"))
}

/// The `include` globs of `reuben.toml` and the `env` overlay, read as plain TOML so they are found
/// even when the full config does not load.
pub fn include_patterns(root: &Path, env: Option<&str>) -> Vec<String> {
    let mut files = vec![root.join("reuben.toml")];
    files.extend(env.map(|e| root.join(format!("reuben.{e}.toml"))));
    let mut patterns = Vec::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        let Ok(table) = toml::from_str::<toml::Table>(&text) else {
            continue;
        };
        if let Some(toml::Value::Array(list)) = table.get("include") {
            patterns.extend(list.iter().filter_map(|v| v.as_str().map(String::from)));
        }
    }
    patterns
}

/// The folder an include glob lives in, relative to the root, and whether it reaches into subfolders.
fn include_base(pattern: &str) -> (PathBuf, bool) {
    let is_glob = |s: &str| s.contains(['*', '?', '[']);
    let parts: Vec<&str> = pattern.split('/').filter(|p| !p.is_empty()).collect();
    let literal = parts.iter().take_while(|p| !is_glob(p)).count();
    // A pattern with no wildcard names a file, so its folder is the parent.
    let keep = if literal == parts.len() {
        literal.saturating_sub(1)
    } else {
        literal
    };
    let base = parts[..keep].iter().collect();
    (base, pattern.contains("**"))
}

/// How to watch an include glob's base folder. The root and its ancestors are never watched recursively,
/// since that would take in `dist/`, `.git/` and everything else beside the site.
fn include_mode(root: &Path, dir: &Path, exists: bool, deep: bool) -> RecursiveMode {
    if exists && deep && !root.starts_with(dir) {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    }
}

/// Collapses a burst of events into one signal once no event has arrived for `quiet`.
/// Ends when the sender is gone and nothing is pending.
pub fn debounce(rx: Receiver<notify::Result<Event>>, quiet: Duration) -> impl Iterator<Item = ()> {
    std::iter::from_fn(move || {
        let _first = rx.recv().ok()?;
        loop {
            match rx.recv_timeout(quiet) {
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                    return Some(());
                }
            }
        }
    })
}

/// Editor droppings: hidden files, backups and swap files.
fn is_noise(path: &Path) -> bool {
    path.file_name().is_some_and(|n| {
        let n = n.to_string_lossy();
        n.starts_with('.') || n.ends_with('~') || n.ends_with(".swp") || n.ends_with(".swx")
    })
}

/// What the event handler needs to know, shared with the watcher that updates it.
#[derive(Default)]
struct State {
    root: PathBuf,
    /// Direct children of the root that matter: site folders, the theme's top folder, config files.
    names: HashSet<PathBuf>,
    /// Include globs, matched against root-level paths (the root is watched non-recursively).
    globs: Vec<glob::Pattern>,
    /// Files watched by name, which are never treated as editor noise.
    files: HashSet<PathBuf>,
    /// Mirror of the watcher's paths, so the handler can spot one being deleted or replaced.
    watched: HashSet<PathBuf>,
    /// Watched paths that were deleted or replaced; the OS may have dropped their watch.
    dirty: HashSet<PathBuf>,
}

impl State {
    fn accepts(&self, path: &Path) -> bool {
        if is_noise(path) && !self.files.contains(path) {
            return false;
        }
        if path.parent() != Some(self.root.as_path()) {
            return true;
        }
        if self.names.contains(path)
            || path
                .file_name()
                .is_some_and(|n| is_env_config(&n.to_string_lossy()))
        {
            return true;
        }
        path.strip_prefix(&self.root)
            .is_ok_and(|rel| self.globs.iter().any(|g| g.matches_path(rel)))
    }

    fn handle(&mut self, event: &Event) -> bool {
        let replaced = matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_))
        );
        if replaced {
            for p in &event.paths {
                if self.watched.contains(p) {
                    self.dirty.insert(p.clone());
                }
            }
        }
        // Dropped events arrive as a path-less notice, and may have hidden any change.
        if event.need_rescan() || event.paths.is_empty() {
            return true;
        }
        !matches!(event.kind, EventKind::Access(_)) && event.paths.iter().any(|p| self.accepts(p))
    }
}

/// Watches the paths `watch_paths` names and keeps the set current as the config changes.
pub struct Watcher {
    inner: RecommendedWatcher,
    root: PathBuf,
    watched: HashMap<PathBuf, RecursiveMode>,
    /// `watch_paths` of the last config that loaded, kept while the config is broken.
    config_paths: Vec<PathBuf>,
    state: Arc<Mutex<State>>,
}

impl Watcher {
    /// `root` should be canonical so it matches the paths the OS reports.
    pub fn new(root: &Path) -> notify::Result<(Watcher, Receiver<notify::Result<Event>>)> {
        let (tx, rx) = mpsc::channel();
        let state = Arc::new(Mutex::new(State {
            root: root.to_path_buf(),
            ..State::default()
        }));
        let handler_state = Arc::clone(&state);
        let inner = notify::recommended_watcher(move |event: notify::Result<Event>| match event {
            Ok(e) => {
                let keep = handler_state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .handle(&e);
                if keep {
                    let _ = tx.send(Ok(e));
                }
            }
            // Something may have been missed, so the error also asks for a rebuild.
            Err(e) => {
                eprintln!("warning: {e}");
                let _ = tx.send(Err(e));
            }
        })?;
        let watcher = Watcher {
            inner,
            root: root.to_path_buf(),
            watched: HashMap::new(),
            config_paths: Vec::new(),
            state,
        };
        Ok((watcher, rx))
    }

    /// Brings the OS watches in line with the site. `config` is `None` when the config failed to load, in
    /// which case the previous set stays and only the `include` globs are re-read. Returns a message for each
    /// path that could not be watched.
    pub fn sync(&mut self, config: Option<&Config>, env: Option<&str>) -> Vec<String> {
        if let Some(config) = config {
            self.config_paths = watch_paths(&self.root, config);
        }
        let dirty = mem::take(&mut self.state.lock().unwrap_or_else(|e| e.into_inner()).dirty);
        for path in dirty {
            if self.watched.remove(&path).is_some() {
                let _ = self.inner.unwatch(&path);
            }
        }

        let mut wanted: HashMap<PathBuf, RecursiveMode> = HashMap::new();
        let mut add = |path: PathBuf, mode: RecursiveMode| {
            let slot = wanted.entry(path).or_insert(mode);
            if mode == RecursiveMode::Recursive {
                *slot = mode;
            }
        };
        let mut names = HashSet::new();
        let mut files = HashSet::new();
        // Folders that do not exist yet are noticed through the root, and picked up on the next sync.
        add(self.root.clone(), RecursiveMode::NonRecursive);
        for dir in WATCHED_DIRS {
            names.insert(self.root.join(dir));
        }
        names.insert(self.root.join("reuben.toml"));
        files.insert(self.root.join("reuben.toml"));

        for path in &self.config_paths {
            let Ok(path) = path.canonicalize() else {
                continue;
            };
            if let Ok(rel) = path.strip_prefix(&self.root)
                && let Some(first) = rel.components().next()
            {
                names.insert(self.root.join(first));
            }
            if path.is_dir() {
                add(path, RecursiveMode::Recursive);
            } else if let Some(parent) = path.parent() {
                // A file is watched through its folder, since an atomic save replaces the file itself.
                add(parent.to_path_buf(), RecursiveMode::NonRecursive);
                files.insert(path);
            }
        }

        let mut globs = Vec::new();
        for pattern in include_patterns(&self.root, env) {
            if let Ok(glob) = glob::Pattern::new(&pattern) {
                globs.push(glob);
            }
            let (base, deep) = include_base(&pattern);
            if let Some(first) = base.components().next() {
                names.insert(self.root.join(first));
            }
            // A folder that is not there yet is watched through its nearest existing ancestor.
            let mut dir = self.root.join(&base);
            let exists = dir.is_dir();
            while !dir.is_dir() && dir != self.root && dir.pop() {}
            let Ok(dir) = dir.canonicalize() else {
                continue;
            };
            add(dir.clone(), include_mode(&self.root, &dir, exists, deep));
        }

        {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.names = names;
            state.globs = globs;
            state.files = files;
        }

        let mut problems = Vec::new();
        let stale: Vec<PathBuf> = self
            .watched
            .keys()
            .filter(|p| !wanted.contains_key(*p))
            .cloned()
            .collect();
        for path in stale {
            let _ = self.inner.unwatch(&path);
            self.watched.remove(&path);
        }
        for (path, mode) in wanted {
            if self.watched.get(&path) == Some(&mode) {
                continue;
            }
            match self.inner.watch(&path, mode) {
                Ok(()) => {
                    self.watched.insert(path, mode);
                }
                Err(e) => problems.push(format!("cannot watch {}: {e}", path.display())),
            }
        }
        self.state.lock().unwrap_or_else(|e| e.into_inner()).watched =
            self.watched.keys().cloned().collect();
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use assert_fs::TempDir;
    use assert_fs::prelude::*;
    use std::thread;

    #[test]
    fn debounce_collapses_bursts() {
        let (tx, rx) = mpsc::channel();
        let mut signals = debounce(rx, Duration::from_millis(150));
        // Each burst is queued at once, so no scheduling delay can split it, and the gap between bursts is
        // far longer than the quiet period, so none can merge them.
        let sender = thread::spawn(move || {
            for _ in 0..5 {
                tx.send(Ok(Event::default())).unwrap();
            }
            thread::sleep(Duration::from_millis(1200));
            tx.send(Ok(Event::default())).unwrap();
            // Dropping `tx` ends the iterator after the final burst.
        });
        assert_eq!(signals.next(), Some(()), "first burst");
        assert_eq!(signals.next(), Some(()), "second burst");
        assert_eq!(signals.next(), None, "channel closed");
        sender.join().unwrap();
    }

    #[test]
    fn watch_paths_include_theme_and_config() {
        let tmp = TempDir::new().unwrap();
        tmp.child("reuben.toml")
            .write_str("theme = \"themes/x\"\ninclude = [\"conf/*.toml\"]\n")
            .unwrap();
        tmp.child("reuben.prod.toml").write_str("").unwrap();
        tmp.child("conf/extra.toml")
            .write_str("title = \"t\"\n")
            .unwrap();
        tmp.child("themes/x/layouts").create_dir_all().unwrap();
        tmp.child("content").create_dir_all().unwrap();
        tmp.child("includes").create_dir_all().unwrap();
        tmp.child("layouts").create_dir_all().unwrap();
        let cfg = config::load(tmp.path(), None).unwrap();
        let paths = watch_paths(tmp.path(), &cfg);
        for want in [
            "content",
            "includes",
            "layouts",
            "themes/x",
            "reuben.toml",
            "reuben.prod.toml",
            "conf/extra.toml",
        ] {
            assert!(paths.contains(&tmp.path().join(want)), "missing {want}");
        }
        assert!(!paths.contains(&tmp.path().join("data")));
        assert!(!paths.contains(&tmp.path().join("static")));
    }

    #[test]
    fn include_base_is_the_literal_folder() {
        assert_eq!(include_base("conf/*.toml"), (PathBuf::from("conf"), false));
        assert_eq!(include_base("*.toml"), (PathBuf::new(), false));
        assert_eq!(include_base("a/b/**/x.toml"), (PathBuf::from("a/b"), true));
        assert_eq!(include_base("a/b.toml"), (PathBuf::from("a"), false));
    }

    #[test]
    fn include_patterns_survive_a_broken_config() {
        let tmp = TempDir::new().unwrap();
        tmp.child("reuben.toml")
            .write_str("bogus_key = 1\ninclude = [\"a/*.toml\"]\n")
            .unwrap();
        tmp.child("reuben.prod.toml")
            .write_str("include = [\"b/*.toml\"]\n")
            .unwrap();
        assert_eq!(
            include_patterns(tmp.path(), Some("prod")),
            ["a/*.toml", "b/*.toml"]
        );
        assert!(include_patterns(tmp.path(), None).len() == 1);
    }

    #[test]
    fn rescan_and_pathless_events_rebuild() {
        let mut state = State {
            root: PathBuf::from("/site"),
            ..State::default()
        };
        let rescan = Event::new(EventKind::Other).set_flag(notify::event::Flag::Rescan);
        assert!(state.handle(&rescan));
        assert!(state.handle(&Event::new(EventKind::Other)));
        let access = Event::new(EventKind::Access(notify::event::AccessKind::Any))
            .add_path(PathBuf::from("/site/content/a.md"));
        assert!(!state.handle(&access));
    }

    #[test]
    fn root_and_ancestors_are_never_watched_recursively() {
        let root = Path::new("/a/b/site");
        let deep = |dir: &str| include_mode(root, Path::new(dir), true, true);
        assert_eq!(deep("/a/b/site"), RecursiveMode::NonRecursive);
        assert_eq!(deep("/a/b"), RecursiveMode::NonRecursive);
        assert_eq!(deep("/"), RecursiveMode::NonRecursive);
        assert_eq!(deep("/a/b/site/conf"), RecursiveMode::Recursive);
        assert_eq!(deep("/a/b/other"), RecursiveMode::Recursive);
        assert_eq!(
            include_mode(root, Path::new("/a/b/site/conf"), false, true),
            RecursiveMode::NonRecursive
        );
    }

    #[test]
    fn state_ignores_editor_noise() {
        let root = PathBuf::from("/site");
        let state = State {
            root: root.clone(),
            ..State::default()
        };
        assert!(!state.accepts(&root.join("content/.index.md.swp")));
        assert!(!state.accepts(&root.join("content/index.md~")));
        assert!(!state.accepts(&root.join("content/.hidden")));
        assert!(state.accepts(&root.join("content/index.md")));
    }
}
