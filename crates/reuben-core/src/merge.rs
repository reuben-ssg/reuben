//! Deep merge of JSON-shaped layers that records which file set each value.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// A merged map plus the file that last set each leaf, keyed by dotted path.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layered {
    pub value: Map<String, Value>,
    pub sources: BTreeMap<String, PathBuf>,
}

impl Layered {
    /// Objects merge key by key; any other overlay value replaces what was there.
    pub fn merge(&mut self, overlay: Map<String, Value>, source: &Path) {
        merge_map(&mut self.value, overlay, "", source, &mut self.sources);
    }
}

fn merge_map(
    base: &mut Map<String, Value>,
    overlay: Map<String, Value>,
    prefix: &str,
    source: &Path,
    sources: &mut BTreeMap<String, PathBuf>,
) {
    for (key, value) in overlay {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match (base.get_mut(&key), value) {
            (Some(Value::Object(existing)), Value::Object(incoming)) => {
                if !incoming.is_empty() {
                    // An empty table recorded its own path as a leaf; it has children now.
                    sources.remove(&path);
                    merge_map(existing, incoming, &path, source, sources);
                }
            }
            (_, value) => {
                forget(sources, &path);
                record(sources, &path, &value, source);
                base.insert(key, value);
            }
        }
    }
}

fn forget(sources: &mut BTreeMap<String, PathBuf>, path: &str) {
    let nested = format!("{path}.");
    sources.retain(|k, _| k != path && !k.starts_with(&nested));
}

fn record(sources: &mut BTreeMap<String, PathBuf>, path: &str, value: &Value, source: &Path) {
    match value {
        Value::Object(map) if !map.is_empty() => {
            for (k, v) in map {
                record(sources, &format!("{path}.{k}"), v, source);
            }
        }
        _ => {
            sources.insert(path.to_string(), source.to_path_buf());
        }
    }
}

/// TOML dates become strings, matching how YAML dates arrive, so `date` works the same in both formats.
pub fn toml_to_json(value: toml::Value) -> Value {
    match value {
        toml::Value::String(s) => Value::String(s),
        toml::Value::Integer(i) => Value::from(i),
        toml::Value::Float(f) => Value::from(f),
        toml::Value::Boolean(b) => Value::Bool(b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(a) => Value::Array(a.into_iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => {
            Value::Object(t.into_iter().map(|(k, v)| (k, toml_to_json(v))).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn tables_merge_and_scalars_replace() {
        let mut l = Layered::default();
        l.merge(
            obj(json!({"a": 1, "t": {"x": 1, "y": 2}})),
            Path::new("base.toml"),
        );
        l.merge(obj(json!({"a": 2, "t": {"y": 3}})), Path::new("over.toml"));
        assert_eq!(
            Value::Object(l.value.clone()),
            json!({"a": 2, "t": {"x": 1, "y": 3}})
        );
        assert_eq!(l.sources["a"], PathBuf::from("over.toml"));
        assert_eq!(l.sources["t.x"], PathBuf::from("base.toml"));
        assert_eq!(l.sources["t.y"], PathBuf::from("over.toml"));
    }

    #[test]
    fn arrays_replace() {
        let mut l = Layered::default();
        l.merge(obj(json!({"tags": ["a", "b"]})), Path::new("1"));
        l.merge(obj(json!({"tags": ["c"]})), Path::new("2"));
        assert_eq!(l.value["tags"], json!(["c"]));
    }

    #[test]
    fn replacing_a_table_with_a_scalar_drops_stale_sources() {
        let mut l = Layered::default();
        l.merge(obj(json!({"t": {"x": 1}})), Path::new("1"));
        l.merge(obj(json!({"t": 5})), Path::new("2"));
        assert!(!l.sources.contains_key("t.x"));
        assert_eq!(l.sources["t"], PathBuf::from("2"));
    }

    #[test]
    fn replacing_a_scalar_with_a_table_records_its_leaves() {
        let mut l = Layered::default();
        l.merge(obj(json!({"t": 5})), Path::new("1"));
        l.merge(obj(json!({"t": {"x": 1, "u": {"z": 2}}})), Path::new("2"));
        assert!(!l.sources.contains_key("t"));
        assert_eq!(l.sources["t.x"], PathBuf::from("2"));
        assert_eq!(l.sources["t.u.z"], PathBuf::from("2"));
    }

    #[test]
    fn empty_object_overlay_is_a_leaf() {
        let mut l = Layered::default();
        l.merge(obj(json!({"t": {}})), Path::new("1"));
        assert_eq!(l.value["t"], json!({}));
        assert_eq!(l.sources["t"], PathBuf::from("1"));
    }

    #[test]
    fn empty_object_over_a_table_merges_and_keeps_sources() {
        let mut l = Layered::default();
        l.merge(obj(json!({"t": {"x": 1}})), Path::new("1"));
        l.merge(obj(json!({"t": {}})), Path::new("2"));
        assert_eq!(l.value["t"], json!({"x": 1}));
        assert_eq!(l.sources["t.x"], PathBuf::from("1"));
    }

    #[test]
    fn filling_an_empty_object_drops_its_own_source() {
        let mut l = Layered::default();
        l.merge(obj(json!({"t": {}})), Path::new("1"));
        l.merge(obj(json!({"t": {"x": 1}})), Path::new("2"));
        assert!(!l.sources.contains_key("t"));
        assert_eq!(l.sources["t.x"], PathBuf::from("2"));
    }

    #[test]
    fn insertion_order_is_kept() {
        let mut l = Layered::default();
        l.merge(obj(json!({"b": 1, "a": 2})), Path::new("1"));
        l.merge(obj(json!({"c": 3, "b": 4})), Path::new("2"));
        let keys: Vec<_> = l.value.keys().cloned().collect();
        assert_eq!(keys, ["b", "a", "c"]);
    }

    #[test]
    fn toml_datetimes_become_strings() {
        let v: toml::Value = toml::from_str("d = 2024-01-02").unwrap();
        assert_eq!(toml_to_json(v), json!({"d": "2024-01-02"}));
    }
}
