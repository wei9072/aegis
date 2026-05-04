(import_statement source: (string (string_fragment) @import))
(import_statement source: (string) @import)
(export_statement source: (string (string_fragment) @import))
(export_statement source: (string) @import)
(call_expression
  function: (identifier) @_fn
  arguments: (arguments (string (string_fragment) @import))
  (#eq? @_fn "require"))
(call_expression
  function: (import)
  arguments: (arguments (string (string_fragment) @import)))
(call_expression
  function: (import)
  arguments: (arguments (string) @import))
