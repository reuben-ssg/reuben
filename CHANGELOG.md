# Changelog

## Unreleased

### Added

An `[assets]` table with `hash`, `headers` and `keep`, and an `asset()` template helper.
Unknown keys in the table are config errors.

```toml
[assets]
hash = true
headers = true
```

`hash = true` renames each static file to `<stem>.<8-hex>.<ext>`, where the hex is a hash of the file's content, so a changed file gets a new URL.
Root-level static files and everything under `.well-known/` keep their names.
`keep` is a list of globs for other files that should keep their names too.
`hash` needs `[css] bundle = true`, and `headers` needs `hash`; without them the config is an error.
`headers = true` writes a `_headers` file that gives hashed files `Cache-Control: public, max-age=31536000, immutable`.
When every output under a folder is hashed, one `/<dir>/*` rule covers the folder; otherwise each file gets its own rule, up to 100 rules.
The rules go after the ones in your own `_headers`, if you have one.
`asset("css/site.css")` returns the file's root-relative URL, including the path of `base_url`, and the hashed name when hashing is on:

```html
<link rel="stylesheet" href="{{ asset("css/site.css") }}">
```

A path that matches no static file is a build error, and the build reports every such error at once.
With hashing on, each `url()` in your CSS points at the hashed name, and a `url()` to a missing file is an error.

### Changed checks

The `link-target` check now also checks `<link href>`, and `src` and `srcset` on `img`, `source`, `script`, `video` and `audio`.
This applies to every site, whether or not it uses `[assets]`.
A site with a broken `src`, `srcset` or `<link href>` will start failing checks after you upgrade.
To fix it, correct the path, or turn the check off with `[a11y] disable = ["link-target"]`.

## 0.2.0 (2026-11-21)

### Changed defaults

Every byte Reuben adds beyond the Markdown's own HTML is now opt-in.
The sitemap, code highlighting and math rendering are off unless you turn them on.
To get the old behavior back, add these lines to `reuben.toml`:

```toml
sitemap = true

[markdown]
highlight = true
math = true
```

With math off, a `$…$` span is plain text, so a `{{ … }}` inside it is now a live block: `$x^{{2}}$` prints `$x^2$`, and `{{n}}` fails if `n` is undefined.
A 0.1.1 site that used math needs `math = true` to keep those braces literal.

### Added

A `[markdown]` table with `highlight`, `math` and `footnote_backlinks`.
Unknown keys in it are config errors.
`[markdown] footnote_backlinks = false` removes the links from each footnote back to its reference, which screen-reader and keyboard users rely on; it turns off an accessibility feature.
When the sitemap is on, each URL gets a `<lastmod>` date: the last commit that touched the page's source, or the file's modification time when git has none.
A page edited but not yet committed keeps its last commit date, because Reuben reads only git's index and history, never working files through git, so a repository's own config can't make the build run commands.
Shallow clones in CI give every page the date of the one fetched commit; fetch full history if the dates matter.

### Changed output

Footnotes lose the `data-footnote*` attributes and the per-link classes; the `footnotes` section class and the back-links' `aria-label` stay.
Code blocks without highlighting are written as `<pre><code class="language-x">` with the code escaped.

### Performance

Sources are read in parallel, each page is written to disk as soon as it renders, and the old output is deleted in parallel after the swap.
`dist/` is still replaced all at once, only when the whole build succeeds, and a failed build leaves it untouched.
On the 1,000-page benchmark (bare layout, Apple M3 Pro), a build takes 74 ms and 28 MiB, against 219 ms and 57 MiB for 0.1.1.
The numbers and the scripts are in [reuben-ssg/benchmarks](https://github.com/reuben-ssg/benchmarks).

### Library API

These changes to `reuben-core` break code that builds the changed structs with literals or calls the changed functions.

Added:

- `config::MarkdownConfig`, the `[markdown]` table, with public `highlight`, `math` and `footnote_backlinks` fields.
- `html::HtmlOptions`, with `Default` and `From<&MarkdownConfig>`, and `html::render_opts`, which renders with those options.
- `markdown::parse_with` and `markdown::code_ranges_with`, which take a `math` flag.
- `script::ScriptEngine::set_markdown`, which makes `markdown()` follow the site's `[markdown]` outside a page render, data scripts included.

Changed:

- `Config` has a new public field `markdown: MarkdownConfig`.
- `script::LinkContext` has a new public field `markdown: MarkdownConfig`, which `markdown()` uses during a page render.
- `sitemap::render` takes `&[(String, Option<jiff::civil::Date>)]`, each URL with its last-modified date, instead of `&[String]`.
- `OutputFile` has a new `Staged` variant for pages already written during `Site::build`; `Site::render` never returns it, and `output::write_atomic` skips it.

Changed behavior, with the same signatures:

- `html::render` no longer highlights code; use `render_with` or `render_opts` for highlighting.
- `markdown::parse` and `markdown::code_ranges` no longer read `$…$` as math; use `parse_with(…, true)` or `code_ranges_with(…, true)`.
- `page::render_body` follows `Config::markdown`, so it renders math and highlights code only when those are on.
- `html::render` and its siblings write the smaller footnote markup described under "Changed output".
