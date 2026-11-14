# reuben

The tastiest static site generator available.
Chips and pickle included.

Reuben is a static site generator written in Rust.
It's built on rye: [Rhai](https://rhai.rs) scripting runs inside your Markdown at build time, with access to collections and page data.
Includes are plain Rhai functions, so every variable a page uses can be traced back to where it was passed in.

Reuben is in early development: `reuben build`, `serve`, `check` and `explain` work today, but anything may still change.

## Checks

Every build checks each page for common accessibility problems, on both the Markdown tree and the final HTML.
Findings print as warnings with the file and line that caused them, and they never fail a build.

| Rule                | What it checks                                                     |
| ------------------- | ------------------------------------------------------------------ |
| `heading-order`     | Headings don't skip a level on the way down.                       |
| `link-text`         | Links have text that says where they go.                           |
| `img-alt`           | Images have an alt attribute, which may be empty for decoration.   |
| `heading-id-unique` | No two headings on a page share an id.                             |
| `table-header`      | Tables have a header row or column.                                |
| `html-lang`         | The html element declares a language.                              |
| `page-title`        | The page has a non-empty title.                                    |
| `single-h1`         | The page has exactly one h1.                                       |
| `main-landmark`     | The page has a main landmark.                                      |
| `id-unique`         | No two elements on a page share an id.                             |
| `form-label`        | Form controls have a label.                                        |
| `link-target`       | Internal links point at a page or file that is built.              |

A finding looks like this:

```text
warning[link-target]: links to `/about/`, which isn't built
  --> content/index.md (built as index.html, line 12)
```

### Switching rules off

List rule ids under `a11y.disable` to switch them off.
For the whole site, put them in `reuben.toml`:

```toml
[a11y]
disable = ["main-landmark", "html-lang"]
```

For every page in a directory and the directories below it, put the same table in that directory's `_dir.toml`.
For one page, put it in the page's front matter:

```yaml
---
title: Gallery
a11y:
  disable: [img-alt]
---
```

Each level adds to the ones above it, so a page skips every rule switched off in `reuben.toml`, in any `_dir.toml` on its path, and in its own front matter.
A rule switched off at one level can't be switched back on below it.
An id that isn't a rule is an error, reported against the file it's in.

### `reuben check` in CI

`reuben check` loads and renders the site like `reuben build` but writes nothing.
It prints every error and every finding, then a summary line.
It exits 0 only when there are no errors and no findings, and 1 otherwise, so it can gate a CI job:

```bash
reuben check
```

Add `--drafts` to check draft pages too, and `--env <name>` to check with `reuben.<name>.toml` merged in.

### `reuben explain`

`reuben explain <page>` lists every name in scope for one page, such as `page.title`, `site.base_url`, collections and helpers, with its value and the file and line that set it.
The page can be a source path like `content/blog/x.md` or `blog/x.md`, or a URL like `/blog/x/`.

```bash
reuben explain /blog/x/
```

It exits 1 if loading the site found errors, after printing them.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
