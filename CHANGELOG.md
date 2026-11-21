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
