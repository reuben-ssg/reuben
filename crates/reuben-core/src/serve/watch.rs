//! File watching for `reuben serve`: which paths to watch, event filtering, and debouncing bursts of events.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

/// What the event filter lets through from the site root, which is watched non-recursively.
#[derive(Default)]
struct RootFilter {
    root: PathBuf,
    /// Direct children of the root that matter: site folders, the theme's top folder, config files.
    names: HashSet<PathBuf>,
}

impl RootFilter {
    fn accepts(&self, path: &Path) -> bool {
        if path.parent() != Some(self.root.as_path()) {
            return true;
        }
        self.names.contains(path)
            || path
                .file_name()
                .is_some_and(|n| is_env_config(&n.to_string_lossy()))
    }
}

/// Watches the paths `watch_paths` names and keeps the set current as the config changes.
pub struct Watcher {
    inner: RecommendedWatcher,
    root: PathBuf,
    watched: HashMap<PathBuf, RecursiveMode>,
    filter: Arc<Mutex<RootFilter>>,
}

impl Watcher {
    /// `root` should be canonical so it matches the paths the OS reports.
    pub fn new(root: &Path) -> notify::Result<(Watcher, Receiver<notify::Result<Event>>)> {
        let (tx, rx) = mpsc::channel();
        let filter = Arc::new(Mutex::new(RootFilter {
            root: root.to_path_buf(),
            names: HashSet::new(),
        }));
        let handler_filter = Arc::clone(&filter);
        let inner = notify::recommended_watcher(move |event: notify::Result<Event>| {
            let keep = match &event {
                Ok(e) => {
                    !matches!(e.kind, EventKind::Access(_)) && {
                        let f = handler_filter.lock().unwrap_or_else(|e| e.into_inner());
                        e.paths.iter().any(|p| f.accepts(p))
                    }
                }
                Err(_) => true,
            };
            if keep {
                let _ = tx.send(event);
            }
        })?;
        let watcher = Watcher {
            inner,
            root: root.to_path_buf(),
            watched: HashMap::new(),
            filter,
        };
        Ok((watcher, rx))
    }

    /// Brings the OS watches in line with `config`. Returns a message for each path that could not be watched.
    pub fn sync(&mut self, config: &Config) -> Vec<String> {
        let mut wanted: HashMap<PathBuf, RecursiveMode> = HashMap::new();
        let mut names = HashSet::new();
        // Folders that do not exist yet are noticed through the root, and picked up on the next sync.
        wanted.insert(self.root.clone(), RecursiveMode::NonRecursive);
        for dir in WATCHED_DIRS {
            names.insert(self.root.join(dir));
        }
        for path in watch_paths(&self.root, config) {
            let Ok(path) = path.canonicalize() else {
                continue;
            };
            if let Ok(rel) = path.strip_prefix(&self.root)
                && let Some(first) = rel.components().next()
            {
                names.insert(self.root.join(first));
            }
            if path.is_dir() {
                wanted.insert(path, RecursiveMode::Recursive);
            } else if let Some(parent) = path.parent() {
                // A file is watched through its folder, since an atomic save replaces the file itself.
                wanted
                    .entry(parent.to_path_buf())
                    .or_insert(RecursiveMode::NonRecursive);
            }
        }
        self.filter.lock().unwrap_or_else(|e| e.into_inner()).names = names;

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
        let sender = thread::spawn(move || {
            for _ in 0..5 {
                tx.send(Ok(Event::default())).unwrap();
                thread::sleep(Duration::from_millis(10));
            }
            thread::sleep(Duration::from_millis(300));
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
}
