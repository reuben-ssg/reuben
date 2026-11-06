//! The `Collection` script type, and the sort rules shared by collections and `Array.sort_by`.

use std::cmp::Ordering;
use std::sync::Arc;

use rhai::{Array, Dynamic, Engine, EvalAltResult, FnPtr, Map, NativeCallContext};

type RhaiResult<T> = Result<T, Box<EvalAltResult>>;

/// A collection's pages in its sort order and its folder tree.
#[derive(Clone)]
pub struct Collection {
    pages: Arc<Array>,
    tree: Arc<Map>,
}

impl Collection {
    pub fn new(pages: Array, tree: Map) -> Self {
        Self {
            pages: Arc::new(pages),
            tree: Arc::new(tree),
        }
    }
}

impl IntoIterator for Collection {
    type Item = Dynamic;
    type IntoIter = std::vec::IntoIter<Dynamic>;

    fn into_iter(self) -> Self::IntoIter {
        self.pages.as_ref().clone().into_iter()
    }
}

/// Orders two items by `key` (a field name, `-` prefix for descending), then by content-relative path.
/// Numbers sort before strings and both before missing or other values, whatever the direction.
pub fn compare(
    key: &str,
    a: Option<&Map>,
    a_path: &str,
    b: Option<&Map>,
    b_path: &str,
) -> Ordering {
    let (name, descending) = key.strip_prefix('-').map_or((key, false), |n| (n, true));
    if !name.is_empty() {
        let av = a.and_then(|m| m.get(name));
        let bv = b.and_then(|m| m.get(name));
        let (ra, rb) = (rank(av), rank(bv));
        if ra != rb {
            return ra.cmp(&rb);
        }
        let ord = match (av, bv) {
            (Some(x), Some(y)) if ra == 0 => compare_numbers(x, y),
            (Some(x), Some(y)) if ra == 1 => x
                .read_lock::<rhai::ImmutableString>()
                .zip(y.read_lock::<rhai::ImmutableString>())
                .map_or(Ordering::Equal, |(x, y)| x.as_str().cmp(y.as_str())),
            _ => Ordering::Equal,
        };
        if ord != Ordering::Equal {
            return if descending { ord.reverse() } else { ord };
        }
    }
    a_path.split('/').cmp(b_path.split('/'))
}

fn rank(v: Option<&Dynamic>) -> u8 {
    match v {
        Some(v) if v.is_int() || v.is_float() => 0,
        Some(v) if v.is_string() => 1,
        _ => 2,
    }
}

fn compare_numbers(a: &Dynamic, b: &Dynamic) -> Ordering {
    match (a.as_int(), b.as_int()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        _ => {
            let f = |v: &Dynamic| {
                v.as_float()
                    .unwrap_or_else(|_| v.as_int().unwrap_or(0) as f64)
            };
            f(a).total_cmp(&f(b))
        }
    }
}

/// Sorts an array of page maps in place; non-map items count as missing and keep their order at the end.
pub fn sort_array(items: &mut Array, key: &str) {
    let path_of = |m: Option<&Map>| {
        m.and_then(|m| m.get("path"))
            .and_then(|p| p.clone().into_string().ok())
            .unwrap_or_default()
    };
    items.sort_by(|a, b| {
        let (a, b) = (a.read_lock::<Map>(), b.read_lock::<Map>());
        let (a, b) = (a.as_deref(), b.as_deref());
        compare(key, a, &path_of(a), b, &path_of(b))
    });
}

pub fn sorted(items: &Array, key: &str) -> Array {
    let mut out = items.clone();
    sort_array(&mut out, key);
    out
}

pub fn register(engine: &mut Engine) {
    engine
        .register_type_with_name::<Collection>("Collection")
        .register_get("pages", |c: &mut Collection| c.pages.as_ref().clone())
        .register_get("tree", |c: &mut Collection| c.tree.as_ref().clone())
        .register_fn("len", |c: &mut Collection| c.pages.len() as i64)
        .register_fn("sort_by", |c: &mut Collection, key: &str| {
            sorted(&c.pages, key)
        })
        .register_fn(
            "map",
            |ctx: NativeCallContext, c: &mut Collection, f: FnPtr| -> RhaiResult<Array> {
                c.pages
                    .iter()
                    .map(|item| f.call_within_context::<Dynamic>(&ctx, (item.clone(),)))
                    .collect()
            },
        )
        .register_fn(
            "filter",
            |ctx: NativeCallContext, c: &mut Collection, f: FnPtr| -> RhaiResult<Array> {
                let mut out = Array::new();
                for item in c.pages.iter() {
                    let keep = f.call_within_context::<Dynamic>(&ctx, (item.clone(),))?;
                    let keep = keep.as_bool().map_err(|t| {
                        format!("filter: the function must return true or false, not {t}")
                    })?;
                    if keep {
                        out.push(item.clone());
                    }
                }
                Ok(out)
            },
        )
        .register_indexer_get(|c: &mut Collection, index: i64| -> RhaiResult<Dynamic> {
            usize::try_from(index)
                .ok()
                .and_then(|i| c.pages.get(i))
                .cloned()
                .ok_or_else(|| {
                    format!(
                        "index {index} is out of range for a collection of {}",
                        c.pages.len()
                    )
                    .into()
                })
        })
        .register_iterator::<Collection>();
}

#[cfg(test)]
mod tests {
    use rhai::Dynamic;

    use crate::template::Escape;

    use super::*;

    fn engine() -> Engine {
        let mut e = Engine::new();
        register(&mut e);
        e.register_fn("sort_by", |a: &mut Array, key: &str| sorted(a, key));
        e
    }

    fn scope() -> rhai::Scope<'static> {
        let page = |t: &str, path: &str, w: Option<i64>| {
            let mut m = Map::new();
            m.insert("title".into(), t.to_string().into());
            m.insert("path".into(), path.to_string().into());
            m.insert("weight".into(), w.map_or(Dynamic::UNIT, Dynamic::from));
            Dynamic::from(m)
        };
        let pages = vec![
            page("B", "x/b.md", Some(2)),
            page("N", "x/n.md", None),
            page("A", "x/a.md", Some(2)),
            page("C", "x/c.md", Some(1)),
        ];
        let mut scope = rhai::Scope::new();
        scope.push_constant("c", Collection::new(pages, Map::new()));
        scope
    }

    fn titles(src: &str) -> String {
        let r: Array = engine().eval_with_scope(&mut scope(), src).unwrap();
        r.into_iter().map(|t| t.into_string().unwrap()).collect()
    }

    #[test]
    fn sort_by_ascending_ties_by_path_and_missing_last() {
        assert_eq!(titles(r#"c.sort_by("weight").map(|p| p.title)"#), "CABN");
    }

    #[test]
    fn sort_by_descending_keeps_missing_last() {
        assert_eq!(titles(r#"c.sort_by("-weight").map(|p| p.title)"#), "ABCN");
    }

    #[test]
    fn array_sort_by_matches_collection_sort_by() {
        assert_eq!(
            titles(r#"c.filter(|p| p.title != "C").sort_by("weight").map(|p| p.title)"#),
            "ABN"
        );
    }

    #[test]
    fn index_iterate_and_len() {
        let mut s = scope();
        let e = engine();
        assert_eq!(e.eval_with_scope::<i64>(&mut s, "c.len()").unwrap(), 4);
        assert_eq!(
            e.eval_with_scope::<String>(&mut s, "c[1].title").unwrap(),
            "N"
        );
        let n = e
            .eval_with_scope::<i64>(&mut s, "let n = 0; for p in c { n += 1; } n")
            .unwrap();
        assert_eq!(n, 4);
        assert!(e.eval_with_scope::<Dynamic>(&mut s, "c[9]").is_err());
    }

    #[test]
    fn dates_sort_as_strings_and_mixed_types_go_last() {
        let mk = |d: Dynamic, path: &str| {
            let mut m = Map::new();
            m.insert("date".into(), d);
            m.insert("path".into(), path.to_string().into());
            Dynamic::from(m)
        };
        let mut items = vec![
            mk(Dynamic::from("2025-01-01".to_string()), "a"),
            mk(Dynamic::from(3_i64), "b"),
            mk(Dynamic::from("2024-01-01".to_string()), "c"),
        ];
        sort_array(&mut items, "date");
        let order: String = items
            .iter()
            .map(|i| {
                i.read_lock::<Map>().unwrap()["path"]
                    .clone()
                    .into_string()
                    .unwrap()
            })
            .collect();
        assert_eq!(order, "bca");
    }

    fn real_eval(src: &str) -> Result<crate::splice::Output, crate::script::ScriptError> {
        use crate::blocks::{Block, BlockKind};
        let engine = crate::script::ScriptEngine::new(std::path::Path::new("."));
        let mut s = scope();
        let block = Block {
            kind: BlockKind::Output,
            source: src.into(),
            span: Default::default(),
        };
        engine.eval_block(&mut s, &block, std::path::Path::new("p.md"), Escape::Html)
    }

    #[test]
    fn errors_inside_map_and_filter_closures_propagate() {
        assert!(real_eval("c.map(|p| p.nope.x)").is_err());
        let err = real_eval("c.filter(|p| 1)").unwrap_err();
        assert!(err.message.contains("true or false"), "{}", err.message);
    }

    #[test]
    fn registered_array_sort_by_orders_arrays() {
        let out = real_eval(
            r#"[#{d: 2, path: "b", t: "B"}, #{d: 1, path: "a", t: "A"}].sort_by("d").map(|m| m.t)"#,
        )
        .unwrap();
        assert_eq!(out, crate::splice::Output::Text("AB".into()));
    }
}
