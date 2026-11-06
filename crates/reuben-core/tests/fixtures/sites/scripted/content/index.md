---
title: Home
tags: [a, b]
---
# {{ page.title }} by {{ site.params.author }}

{% let shown = page.tags.len(); %}
Tags: {{ shown }}

{{ card("About", "/about/") }}

`{{ not code }}`
