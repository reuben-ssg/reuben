# Changelog

## Unreleased

### Added

`@/page.md` links, with an optional `#anchor`, now work in the `href` and `src` attributes of raw HTML, such as `<a class="button" href="@/quickstart.md">`.
They resolve to the page's URL, with the path of `base_url`, and are checked like Markdown `@/` links.
A link to a missing page or anchor is an error naming the file, line and column of the attribute.
Code blocks and inline code are not rewritten.
Raw HTML that a Rhai block or an include produces is not rewritten.

### Fixed

A plain root-relative link to a page Reuben built, such as `/guide/checks/#anchor`, now has its anchor checked, like an `@/page.md#anchor` link.
This covers `/guide/checks/index.html#anchor` and links that carry the path of `base_url`.
A missing anchor is reported with the file, line and column of the link.
Links to static files and to other sites are not checked.

Calling `eval` in a script now says why it is blocked, instead of Rhai's `reserved keyword 'eval' is disabled`.
The error still points at the `eval` call.

### Changed checks

Sites with a plain link to an anchor that doesn't exist will start failing to build.

## 0.3.0 (2026-11-26)

### Added

An `[assets]` table with `hash`, `headers` and `keep`, and an `asset()` template helper.
Unknown keys in the table are config errors.
`[assets]` is off by default: `hash` and `headers` default to `false` and `keep` to an empty list, so existing sites build as before.

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

A path that matches no static file is a build error naming the file and line of the call, and the build reports every such error at once.
With hashing on, each `url()` in your CSS points at the hashed name, including those inside `image-set()`, and a `url()` to a missing file is an error.
A stylesheet can't name another stylesheet, with `asset()` or `url()`, while hashing is on.

### Changed checks

The `link-target` check now also checks `<link href>`, and `src` and `srcset` on `img`, `source`, `script`, `video` and `audio`.
This applies to every site, whether or not it uses `[assets]`.
A site with a broken `src`, `srcset` or `<link href>` will start failing checks after you upgrade.
To fix it, correct the path, or turn the check off with `[a11y] disable = ["link-target"]`.

### Changed requirements

Building Reuben from source, including with `cargo install reuben`, now needs Rust 1.100 or later.
The previous minimum, 1.85, was wrong: a dependency already needed 1.89.
Run `rustup update` if `cargo install` says your Rust is too old.
The npm package and the installers ship prebuilt binaries, so they don't need Rust at all.

### Fixed

Errors and accessibility findings after a `{{ }}` or `{% %}` block that spans several lines now point at the right line and column.
Before, every position after such a block was too early by the number of lines the block spanned.
This covers blocks between paragraphs and blocks inside a paragraph, list item, table cell, heading or HTML block.

A missing `asset()` path now prints the file, line and column of the call, with a source snippet, like other located errors.

### Performance

A page whose accessibility rules for the finished HTML (`html-lang`, `page-title`, `single-h1`, `main-landmark`, `id-unique`, `form-label` and `link-target`) are all off is no longer parsed a second time.
Its ids are collected only when a link such as `@/page.md#anchor` asks for one, so broken anchors are still errors.
A build makes fewer system calls: each layout is found and read once per build instead of once per page, a page's output folder is created once however many pages share it, a sidecar `.rhai` is looked up in the folder listing instead of on disk, and a static template is opened once.
Each page's Markdown is parsed once per build instead of twice, unless the site might read `collections`.
The site might read them when the word `collections` appears anywhere in a Markdown page's body that has a `{{` or `{%` block, in a template page, in a sidecar `.rhai`, in any file in `layouts/`, or in an include `.rhai` file, the site's or the theme's, comments included, or when a sidecar or a file in `layouts/` can't be read.
Then the pages in collections are parsed a second time, while the site loads, for the `summary` and `toc` other pages read from them.
A page with `{{` or `{%` blocks finds the code they must skip once instead of twice.
On a 10,000-page site that doesn't read collections, CPU time falls by about a fifth and peak memory by about a quarter.
On a 6,000-page site that reads collections, maximum resident memory may rise by a few MB, though peak memory footprint stays at or below 0.2.0.
`--jobs` now sets the threads for every step of a build, reading sources and writing files included, so `--jobs 1` really reads, renders and writes on one thread.
Deleting the previous output after the swap isn't build work, so it runs on up to four threads of its own whatever `--jobs` says.
A stale `.dist.reuben-old` left by an interrupted build is deleted before the swap even when it is a file or a symlink, and a symlink is removed without touching its target.
What can't be deleted of the previous output after the swap is now printed as a warning, and the next build deletes it before it swaps.
Without `--jobs`, a Mac with Apple silicon builds on its performance cores (6 on an M3 Pro) instead of on every core, since a build is mostly file creation and the efficiency cores only add CPU time; elsewhere the default is still one thread per core.
On the 10,000-page benchmark on an M3 Pro, a default build uses a third to a half less CPU time and finishes about a tenth sooner over an existing output and up to a fifth sooner from scratch.
`RAYON_NUM_THREADS` no longer changes how many threads a build uses; use `--jobs`.

### Library API

Added to `reuben-a11y`: `page_ids`, `Rule::runs_on_html` and `RuleSet::any_html_rule`.
Added to `reuben-core`: `check::page_ids` and `layout::Layouts`, which finds and reads layouts once for a render.
Changed in `reuben-core`: `layout::render_layout` takes a `&Layouts` after the layout's path, and reads the layout through it.
Added to `reuben-core`: `blocks::SourceMap`, the `map` field of `blocks::Extracted`, and `markdown::parse_extracted`, which gives nodes their positions in the page before its blocks were taken out.
Changed in `reuben-core`: `splice::splice` takes how many line breaks each block spanned, and `doc::Fill` has a `newlines` field and counts its `line` in the page source.
Changed in `reuben-core`: `site::Page` no longer has the `pre_block_toc` and `summary` fields; `Site::pre_block(&page)` returns the page's `page::PreBlock`, with `toc` and `summary`.
`site::Page` has private fields now, so it can't be built with a struct literal outside the crate.
Added to `reuben-core`: `headings::toc_before_blocks`, which gives the table of contents before blocks run without changing the tree.
Added to `reuben-core`: `Site::load_and_render` and `Site::load_and_build`, which load and then render or build on one pool of `jobs` threads, and the `site::RenderResult` type.
Changed in `reuben-core`: `Site::build` writes the output files on its `jobs` pool, and a pool that can't start is reported as "can't start N build threads".

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
