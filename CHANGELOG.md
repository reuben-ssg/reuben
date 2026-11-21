# Changelog

## 0.2.0 (unreleased)

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

`OutputFile` has a new `Staged` variant for pages already written during `Site::build`; `Site::render` never returns it.
