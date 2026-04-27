; `use foo;`, `use foo::bar;`, `use foo::{bar, baz};`, `use foo as f;`
; — capture the whole argument node and let `normalize_import` take
; the leftmost `::`-separated segment. Coarser than per-segment
; capture but covers every shape uniformly.
(use_declaration argument: (_) @import)

; `mod foo;` and `mod foo { ... }` — the file-include and inline
; module forms. For cycle detection on a multi-file crate the
; file-include is what we actually want.
(mod_item name: (identifier) @import)

; `extern crate foo;` — legacy 2015-edition crate import.
(extern_crate_declaration name: (identifier) @import)
