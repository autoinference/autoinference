# autoinference

**Status:** Work in progress — placeholder release reserving the name across major package registries.

A library for autoinference. Full functionality coming soon.

## Install

| Ecosystem | Command |
| --- | --- |
| Python (pip) | `pip install autoinference` |
| Python (uv) | `uv add autoinference` |
| Node.js (npm) | `npm install autoinference` |
| Node.js (npx) | `npx autoinference` |
| Rust (cargo) | `cargo add autoinference` |
| Go | `go get github.com/autoinference/autoinference` |

## Repo layout

```
.
├── go.mod, autoinference.go   # Go module (root, so `go get` works)
├── python/                    # PyPI package (pip + uv)
├── node/                      # npm package (npm + npx)
└── rust/                      # crates.io crate
```

See [PUBLISH.md](PUBLISH.md) for the publishing checklist.

## License

MIT — see [LICENSE](LICENSE).
