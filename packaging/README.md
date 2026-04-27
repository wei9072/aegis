# V2.0 packaging templates

Distribution scaffolding for shipping `aegis` + `aegis-mcp` to end
users. Per `docs/v1_rust_port_plan.md` V2.0 — code-side ready;
real-world activation needs CI credentials + a tag push.

## Layout

```
.github/workflows/
├── ci.yml                Build + test on every push/PR (Linux x86_64).
└── release.yml           Tag-triggered cross-platform build (5 targets).
                          Uploads tarballs to the auto-created GitHub Release.

packaging/
├── homebrew/aegis.rb     Homebrew formula. Copy to a tap repo.
└── npm/                  npm wrapper that downloads the right
                          per-platform binary at install time.
    ├── package.json
    └── scripts/install.js
```

## V2.0 activation checklist

Each step is mechanical; the bottleneck is real-world credential
setup, not code authorship.

1. **Tag a release** — `git tag v0.1.0 && git push origin v0.1.0`.
   `release.yml` runs and uploads `.tar.gz` / `.zip` artifacts to
   `https://github.com/wei9072/aegis/releases/v0.1.0`.

2. **Verify artifacts download + extract + run** — pull each
   tarball, untar, run `./aegis languages` and the MCP smoke
   test. (CI does this on tag push too — see release.yml.)

3. **Create the Homebrew tap repo** at `wei9072/homebrew-aegis`.
   Copy `packaging/homebrew/aegis.rb` to `Formula/aegis.rb`.
   Run `shasum -a 256 *.tar.gz` on the released artifacts and
   replace the four `REPLACE_WITH_*_SHA256` placeholders. Commit.

4. **Verify Homebrew install** — `brew tap wei9072/aegis && brew
   install aegis && aegis languages`.

5. **Publish the npm wrapper** — `cd packaging/npm && npm publish
   --access public`. Verify with `npm install -g @aegis/cli &&
   aegis languages`.

6. **Publish the rlibs to crates.io** in dep order:
   ```
   for crate in aegis-trace aegis-decision aegis-ir aegis-providers \
                aegis-runtime aegis-core aegis-cli aegis-mcp; do
     (cd crates/$crate && cargo publish)
     sleep 30  # let the registry settle before the next dep
   done
   ```
   Users can then `cargo install aegis-cli` directly.

## Why these specific surfaces

- **GitHub Releases** is the source of truth for binaries. The
  Homebrew formula + npm wrapper both point at release URLs.
- **Homebrew tap** (vs. core Homebrew submission) — keeps the
  release cycle independent of the homebrew-core review queue.
  Promote to homebrew-core after V2.0 stabilises.
- **npm wrapper** — makes Aegis installable for JS-tooling users
  who don't want to install Rust or Homebrew. Postinstall script
  fetches the right per-platform binary instead of compiling.
- **crates.io** — for downstream Rust consumers (plugin authors
  building on the `aegis-runtime` traits, third-party language
  adapters extending `aegis-core::ast::registry`).

## Replacing the hand-rolled `release.yml` with `cargo dist`

Per the V2.0 plan note, `cargo dist`
(https://opensource.axo.dev/cargo-dist/) automates the cross-
compile matrix + Homebrew formula + npm wrapper generation. The
hand-rolled templates here are there as a working baseline +
escape hatch — if `cargo dist` doesn't fit some future need
(custom signing, internal artifact registry, etc.), the matrix
in `release.yml` is the fallback.
