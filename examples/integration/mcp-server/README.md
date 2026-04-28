# MCP client integration example

This example shows how a client-side agent loop can call `aegis-mcp`
before it writes a file.

The important boundary is control:

- The agent or orchestrator owns generation and branching.
- `aegis-mcp` only observes a proposed file state and returns a verdict.
- A `BLOCK` verdict is a stop signal. The client should halt, surface,
  or drop the candidate change.
- Do not feed Aegis reasons back into the LLM as retry hints.

## What it demonstrates

`client_smoke.py` starts the MCP server over stdio JSON-RPC and calls
the single shipped tool:

```text
validate_change(path, new_content, old_content?)
```

It checks two cases:

- A syntactically valid Python proposal should return `PASS`.
- A syntactically invalid Python proposal should return `BLOCK`.

For `PASS`, the example prints a "proceed" client action. For `BLOCK`,
it prints a "halt_and_surface" client action. It intentionally does
not build a retry prompt from the returned reasons.

## Run it

From the repository root:

```bash
python3 examples/integration/mcp-server/client_smoke.py
```

By default the smoke test starts the server with:

```bash
cargo run --quiet --package aegis-mcp
```

If you already built or installed `aegis-mcp`, point the example at
that command instead:

```bash
AEGIS_MCP_COMMAND="target/debug/aegis-mcp" \
  python3 examples/integration/mcp-server/client_smoke.py
```

Expected shape:

```text
PASS case: decision=PASS -> client_action=proceed
BLOCK case: decision=BLOCK -> client_action=halt_and_surface
MCP client smoke test passed.
```

## Client pattern

A real agent loop should branch on the verdict before applying a
candidate write:

```text
if decision == "PASS":
    apply candidate change
elif decision == "WARN":
    surface for review or require a human policy decision
else:
    halt, surface, or drop the candidate change
```

The `reasons` field is useful for traces and operator visibility.
It is not a coaching channel back into the model.
