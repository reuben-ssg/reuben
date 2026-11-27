# reuben

The tastiest static site generator available.
Chips and pickle included.

Reuben is a static site generator written in Rust.
It's built on rye: [Rhai](https://rhai.rs) scripting runs inside your Markdown at build time, with access to collections and page data.
Includes are plain Rhai functions, so every variable a page uses can be traced back to where it was passed in.

Reuben is in early development: `reuben new`, `build`, `serve`, `check` and `explain` work today, but anything may still change.

## Install

In a site project, add Reuben as a dev dependency from npm:

```sh
npm install --save-dev @reuben-ssg/reuben
```

The npm package installs the prebuilt `reuben` command, so run it with `npx reuben build`.

To install the `reuben` command from source with Cargo:

```sh
cargo install reuben
```

To install a prebuilt binary on macOS or Linux with the shell installer from the latest GitHub Release:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/reuben-ssg/reuben/releases/latest/download/reuben-installer.sh | sh
```

On Windows, use the PowerShell installer from the same release:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/reuben-ssg/reuben/releases/latest/download/reuben-installer.ps1 | iex"
```

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
| `region-name`       | Focusable regions, such as code blocks and tables, have a name.    |

`region-name` runs only when one of the rules from `html-lang` to `link-target` is on too, because those rules decide whether Reuben reads the finished HTML of each page a second time.

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

## Linking to pages

Link to a page by naming its source file, and Reuben writes the page's real URL into the built site.
Links then keep working when a page moves or its URL changes.

```markdown
See the [scope guide](@/writing-guide/style/scope.md#assumptions).
```

builds to `<a href="/writing-guide/style/scope/#assumptions">`.

There are two forms:

- `@/path.md` starts at `content/`, so `@/writing-guide/style/scope.md` always means that file.
- A relative path such as `scope.md`, `./scope.md` or `../style/scope.md` starts at the folder of the page that contains the link.

A destination is a source link when it starts with `@/`, or when it has no scheme, doesn't start with `/` or `#`, and its path ends in `.md`.
A fragment (`#assumptions`) or query (`?x=1`) is kept as written.
Any other link builds exactly as before, and a link that starts with `/` is a plain URL even if it ends in `.md`.
Raw HTML links aren't rewritten.

Every source link that can't be resolved is a build error at the link's file and line, reported with all the other errors in the run:

```text
error: no page `_index.md`; did you mean `index.md`?
  --> content/a.md:4:1
```

| Message                                                 | Cause                                                              |
| ------------------------------------------------------- | ------------------------------------------------------------------ |
| ``no page `x.md` ``                                     | No such file under `content/`, with a suggestion when one is near. |
| ``links to `x.md`, a draft that isn't built ...``       | The target is a draft and the build isn't using `--drafts`.        |
| ``link `../../x.md` points outside content/``           | The path climbs above `content/`.                                  |
| ``link `@/x.txt` must name a .md file``                 | An `@/` link to something that isn't Markdown.                     |
| ``links to `#id` on `x.md`, which has no such id``      | The target page's built HTML has no element with that id.          |

There is no `_index.md` alias: link to `index.md`.
These are errors, not accessibility findings, so `a11y.disable` doesn't switch them off.

Layouts and includes can use the same forms with `url_for`, which is how to link from outside Markdown, such as a navigation bar:

```html
<a href="{{ url_for("@/portfolio/index.md") }}">Portfolio</a>
```

`url_for` joins the full `base_url`, so the result also works in feeds, while a Markdown source link gets only the base path, such as `/sub/writing-guide/`, so the output stays portable between hosts.
Relative paths resolve against the page being built, including in an include or layout it uses, so use `@/` in includes and layouts shared across folders.
A relative path in `markdown()` output resolves the same way.
The fragment check covers only the Markdown of a page, not `url_for` results or links inside `markdown()` output, because that text may land anywhere on the page.
A `markdown()` link is still resolved, and a missing file there is an error, but its `#fragment` isn't checked.
Outside a page being built, such as in a `data/` file, `url_for` and `markdown()` can't resolve a source link, so it is an error there.

Made with love by [@nickvigilante](https://github.com/nickvigilante).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
