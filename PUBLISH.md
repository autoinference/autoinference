# Publishing checklist — claim the `autoinference` name

Author metadata is set to `Auto Inference <hello@autoinference.org>` (the public, external-comm mailbox). Critical service ownership (npm/PyPI/crates account login, GitHub org owner, Cloudflare, registrar, billing, search console, analytics) should sit on `admin@autoinference.org` — do NOT use `hello@` for high-privilege account recovery.

Each section below is independent — do them in any order.

---

## 1. GitHub org + repo (do first — also claims the Go module path)

**Step 1a — create the org** (web UI only, GitHub doesn't expose org creation via the CLI for personal accounts):

1. Go to https://github.com/account/organizations/new
2. Pick the **Free** plan
3. Org account name: `autoinference`
4. Confirm your email

**Step 1b — authenticate the gh CLI**

```bash
gh auth login
# choose: GitHub.com → HTTPS → "Login with a web browser"
```

**Step 1c — push the repo and tag** (run from `/Users/cau/Desktop/autoinference`):

```bash
gh repo create autoinference/autoinference --public --source=. --remote=origin --push
git tag v0.0.1
git push origin v0.0.1

# Warm the Go module proxy so `go get` resolves immediately
curl "https://proxy.golang.org/github.com/autoinference/autoinference/@v/v0.0.1.info"
```

Verify Go works:

```bash
mkdir /tmp/aitest && cd /tmp/aitest
go mod init test
go get github.com/autoinference/autoinference@v0.0.1
```

---

## 2. PyPI (covers `pip install` and `uv add`)

Needs: PyPI account at https://pypi.org/account/register/ and an API token from https://pypi.org/manage/account/token/ (scope: "Entire account" for first upload).

Artifacts are already built in [python/dist/](python/dist/). To publish:

```bash
cd /Users/cau/Desktop/autoinference/python
# username is literally __token__, password is your pypi-XXXX token
~/Library/Python/3.9/bin/twine upload dist/*
```

Verify:

```bash
pip install autoinference
# or
uv pip install autoinference
```

> **Note on uv:** uv resolves from PyPI by default — the same upload claims both.

---

## 3. npm (covers `npm install` and `npx`)

Needs: npm account at https://www.npmjs.com/signup, then auth on this machine.

```bash
npm adduser    # interactive login (opens browser)

cd /Users/cau/Desktop/autoinference/node
npm publish --access public
```

Verify:

```bash
npm view autoinference
```

> **Note on npx:** `npx autoinference` works automatically for any published npm package. If you want `npx autoinference <command>` to do something, add a `"bin"` field to `package.json`.

---

## 4. crates.io (Rust) — **PERMANENT, do last**

Needs: account at https://crates.io (sign in with GitHub — set this up after step 1), then a token from https://crates.io/settings/tokens.

```bash
cargo login <YOUR_CRATES_IO_TOKEN>

cd /Users/cau/Desktop/autoinference/rust
cargo publish
```

> crates.io publishes are **permanent** — versions cannot be deleted, only yanked. Double-check `Cargo.toml` before running.

Verify:

```bash
cargo search autoinference
```

---

## After all 4 are claimed

Push a 0.0.2 anywhere you change package metadata. Squatting-policy reminder: PyPI (PEP 541), npm, and crates.io can reclaim names that stay empty long-term, so push real code as soon as you have it.
