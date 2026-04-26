# k8sdesk

> A minimal, safe Kubernetes desktop client — no system `kubectl` required.

[![MIT License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Built with Tauri](https://img.shields.io/badge/built%20with-Tauri%202-24c8d8?logo=tauri)](https://v2.tauri.app)
[![Angular 18](https://img.shields.io/badge/Angular-18-dd0031?logo=angular)](https://angular.dev)
[![Release](https://img.shields.io/github/v/release/YOUR_GITHUB_USERNAME/k8sdesk?label=latest)](https://github.com/YOUR_GITHUB_USERNAME/k8sdesk/releases/latest)

<a href='https://ko-fi.com/X8X21YHKJ9' target='_blank'>
  <img height='36' style='border:0px;height:36px;' src='https://storage.ko-fi.com/cdn/kofi6.png?v=6' border='0' alt='Buy Me a Coffee at ko-fi.com' />
</a>

---

k8sdesk is a **focused Kubernetes operations tool** built to eliminate the risks of using a shared global `kubectl` configuration.
Credentials stay encrypted on disk, commands go through a strict DSL, and destructive actions require explicit confirmation.

## Downloads

Pre-built binaries are attached to every [GitHub Release](https://github.com/YOUR_GITHUB_USERNAME/k8sdesk/releases/latest).

| Platform | Installer |
|---|---|
| **macOS** (Apple Silicon + Intel universal) | `k8sdesk_*.dmg` |
| **Windows** 64-bit | `k8sdesk_*_x64-setup.exe` · `k8sdesk_*_x64_en-US.msi` |
| **Linux** 64-bit | `k8sdesk_*_amd64.AppImage` · `k8sdesk_*_amd64.deb` |

> **macOS note:** The app is not notarized yet. On first launch, right-click → Open to bypass Gatekeeper, or run:
> ```sh
> xattr -cr /Applications/k8sdesk.app
> ```

## Features

- **No system kubectl required** — embedded `kube-rs` client talks directly to the cluster API.
- **Never touches `~/.kube/config`** — `$KUBECONFIG` and `~/.kube` are never read or written.
- **Encrypted credential store** — AES-256-GCM at rest, master key held in the OS keychain.
- **Manifest file manager** — browse a per-cluster folder, edit YAML in a Monaco editor, apply directly to the cluster.
- **Safe command DSL** — cluster + namespace are auto-applied; forbidden tokens are rejected by the parser.
- **Destructive-action guard** — `delete`, `apply`, `scale`, `rollout restart` require confirmation; `prod` clusters require typing the cluster name.
- **Environment color coding** — `dev` / `staging` / `prod` badges; persistent red window border on production clusters.
- **Theme support** — GitHub Dark, Midnight, Solarized Dark, GitHub Light, One Light.

## Stack

| Layer | Technology |
|---|---|
| Desktop runtime | Tauri 2 (Rust) |
| Frontend | Angular 18 — standalone components, signals |
| Kubernetes client | kube-rs + k8s-openapi |
| Editor | Monaco Editor (ngx-monaco-editor-v2) |
| Encryption | aes-gcm + keyring (OS keychain) |

## Prerequisites

- Node.js ≥ 20
- Rust toolchain — install via [rustup](https://rustup.rs/)
- Platform Tauri build deps: <https://v2.tauri.app/start/prerequisites/>

## Getting started

```sh
npm install
npm run dev          # Angular dev server + Tauri window
```

## Building a release binary locally

```sh
npm run package
# Outputs to src-tauri/target/release/bundle/
#   macOS  → *.dmg, *.app
#   Windows → *-setup.exe, *.msi
#   Linux  → *.AppImage, *.deb
```

## Releasing (GitHub Actions)

The repo ships a ready-made workflow at [`.github/workflows/release.yml`](.github/workflows/release.yml) that:

1. Builds on **macOS** (universal .dmg), **Windows** (.msi + .exe) and **Linux** (.AppImage + .deb) in parallel.
2. Creates a **draft GitHub Release** and attaches all binaries automatically.

**Steps to publish a new release:**

```sh
# 1. Bump the version in two places:
#    - package.json          → "version": "x.y.z"
#    - src-tauri/tauri.conf.json → "version": "x.y.z"

# 2. Commit the version bump
git add package.json src-tauri/tauri.conf.json
git commit -m "chore: release vx.y.z"

# 3. Tag and push — this triggers the workflow
git tag vx.y.z
git push origin main --tags
```

4. GitHub Actions builds all three platforms (~10 min).
5. Go to **Releases** on GitHub, review the draft, add release notes, and publish.

> **First-time setup:** Replace `YOUR_GITHUB_USERNAME` in the README badges with your actual GitHub username/org.
> For macOS code-signing and notarization, add the secrets listed (commented out) in the workflow file.

## Command DSL

You can type commands with or without the `kubectl` prefix — both work.

| Command | Example | Severity |
|---|---|---|
| `get <res> [name]` | `get pods` | safe |
| `describe <res> <name>` | `describe pod my-pod` | safe |
| `logs <pod> [-c container] [--tail N] [-f]` | `logs my-pod -f` | safe |
| `delete <res> <name>` | `delete pod my-pod` | ⚠ destructive |
| `scale <res> <name> --replicas N` | `scale deploy/api --replicas 3` | ⚠ destructive |
| `rollout restart <res> <name>` | `rollout restart deploy/api` | ⚠ destructive |
| `apply` (via file manager or paste modal) | — | ⚠ destructive |
| `help` | — | — |

Rejected tokens: `kubectl`, `--kubeconfig`, `--context`, `--server`, `--token`, `--user`, `--cluster`, `exec`, `cp`, `port-forward`, `proxy`, `auth`, `config`.

## Architecture

```
┌────────── Angular UI ──────────┐
│ cluster + namespace selectors  │
│ manifest file manager          │   <-- only whitelisted Tauri IPC
│ Monaco YAML editor             │       commands are exposed
│ DSL terminal                   │
│ confirm modal                  │
└──────────────┬─────────────────┘
               │ tauri.invoke
┌──────────────┴─────────────────┐
│ Rust backend                   │
│  ├ store.rs    AES-GCM file +  │
│  │             OS keychain key │
│  ├ kube_client builds in-mem   │
│  │             kube::Config    │
│  ├ dsl/parser  verb whitelist  │
│  ├ dsl/executor → kube API     │
│  └ safety      HMAC tokens +   │
│                classifier      │
└────────────────────────────────┘
```

## Security properties

1. **No system kubeconfig** — `kube::Config::infer` is never called. Works with `KUBECONFIG=/dev/null`.
2. **Credentials never leave Rust** — `cluster_list` returns a redacted struct; tokens/certs are never sent to the frontend.
3. **Encryption at rest** — `clusters.enc` is AES-256-GCM; the 32-byte master key is in the OS keychain. Tampering fails AEAD.
4. **HMAC confirmation tokens** — destructive commands return a challenge; the executor verifies an HMAC-SHA256 of `(cluster_id, namespace, command)`. Single-use, 30 s TTL.
5. **Kubeconfig import** — `exec` and `auth-provider` plugin entries are rejected (they would run arbitrary external programs).
6. **Namespace enforcement** — pasted YAML's `metadata.namespace` is forced to the active namespace before apply.
7. **Audit logging** — `tracing` records cluster name + verb only; tokens and apply bodies are never logged.

## Running tests

```sh
cd src-tauri && cargo test
```

Covers: encrypted store round-trip, ciphertext tampering rejection, DSL parser (forbidden tokens), destructive classifier, HMAC confirmation tokens, kubeconfig import validation.

## Out of scope (intentional)

`exec` into pods, `port-forward`, `cp`, log streaming with backpressure, RBAC explorer, plugin system, multi-context per cluster.

## Contributing

Pull requests are welcome. For larger changes please open an issue first to discuss what you'd like to change.

## Support

If k8sdesk saves you time, consider buying me a coffee ☕

<a href='https://ko-fi.com/X8X21YHKJ9' target='_blank'>
  <img height='36' style='border:0px;height:36px;' src='https://storage.ko-fi.com/cdn/kofi6.png?v=6' border='0' alt='Buy Me a Coffee at ko-fi.com' />
</a>

## License

[MIT](LICENSE)


---

k8sdesk is a **focused Kubernetes operations tool** built to eliminate the risks of using a shared global `kubectl` configuration.
Credentials stay encrypted on disk, commands go through a strict DSL, and destructive actions require explicit confirmation.

## Features

- **No system kubectl required** — uses an embedded Kubernetes client (`kube-rs`) that talks directly to the cluster API.
- **Never touches `~/.kube/config`** — reads or writes `$KUBECONFIG` or `~/.kube` are never made.
- **Encrypted credential store** — AES-256-GCM at rest, master key held in the OS keychain.
- **Manifest file manager** — browse a folder per cluster, edit YAML with a full Monaco editor, and apply directly to the cluster.
- **Safe command DSL** — cluster + namespace are auto-applied; forbidden tokens (`kubectl`, `--context`, `exec`, `port-forward`, …) are rejected by the parser.
- **Destructive-action guard** — `delete`, `apply`, `scale`, `rollout restart` require confirmation; `prod` clusters require typing the cluster name.
- **Environment color coding** — `dev` / `staging` / `prod` badges and a persistent red window border when a production cluster is active.
- **Theme support** — GitHub Dark, Midnight, Solarized Dark, GitHub Light, One Light.

## Stack

| Layer | Technology |
|---|---|
| Desktop runtime | Tauri 2 (Rust) |
| Frontend | Angular 18 — standalone components, signals |
| Kubernetes client | kube-rs + k8s-openapi |
| Editor | Monaco Editor (ngx-monaco-editor-v2) |
| Encryption | aes-gcm + keyring (OS keychain) |

## Prerequisites

- Node.js ≥ 20
- Rust toolchain — install via [rustup](https://rustup.rs/)
- Platform Tauri build deps: <https://v2.tauri.app/start/prerequisites/>

## Getting started

```sh
npm install
npm run dev          # Angular dev server + Tauri window
```

## Building a release binary

```sh
npm run package      # produces .dmg / .msi / .AppImage in src-tauri/target/release/bundle
```

## Command DSL

You can type commands with or without the `kubectl` prefix — both are accepted.

| Command | Example | Severity |
|---|---|---|
| `get <res> [name]` | `get pods` | safe |
| `describe <res> <name>` | `describe pod my-pod` | safe |
| `logs <pod> [-c container] [--tail N] [-f]` | `logs my-pod -f` | safe |
| `delete <res> <name>` | `delete pod my-pod` | destructive |
| `scale <res> <name> --replicas N` | `scale deploy/api --replicas 3` | destructive |
| `rollout restart <res> <name>` | `rollout restart deploy/api` | destructive |
| `apply` (via file manager or paste modal) | — | destructive |
| `help` | — | — |

Rejected tokens: `kubectl`, `--kubeconfig`, `--context`, `--server`, `--token`, `--user`, `--cluster`, `exec`, `cp`, `port-forward`, `proxy`, `auth`, `config`.

## Architecture

```
┌────────── Angular UI ──────────┐
│ cluster + namespace selectors  │
│ manifest file manager          │   <-- only whitelisted Tauri IPC
│ Monaco YAML editor             │       commands are exposed
│ DSL terminal                   │
│ confirm modal                  │
└──────────────┬─────────────────┘
               │ tauri.invoke
┌──────────────┴─────────────────┐
│ Rust backend                   │
│  ├ store.rs    AES-GCM file +  │
│  │             OS keychain key │
│  ├ kube_client builds in-mem   │
│  │             kube::Config    │
│  ├ dsl/parser  verb whitelist  │
│  ├ dsl/executor → kube API     │
│  └ safety      HMAC tokens +   │
│                classifier      │
└────────────────────────────────┘
```

## Security properties

1. **No system kubeconfig** — `kube::Config::infer` is never called. The app works even with `KUBECONFIG=/dev/null`.
2. **Credentials never leave Rust** — `cluster_list` returns a redacted struct; tokens/certs are never sent to the frontend.
3. **Encryption at rest** — `clusters.enc` is AES-256-GCM; the 32-byte master key is stored in the OS keychain. Tampering fails AEAD verification.
4. **HMAC confirmation tokens** — destructive commands return a challenge envelope; the executor verifies an HMAC-SHA256 fingerprint of `(cluster_id, namespace, command)`. Tokens are single-use with a 30 s TTL.
5. **Kubeconfig import** — `exec` and `auth-provider` plugin entries are rejected (they would allow an imported config to run arbitrary external programs).
6. **Namespace enforcement** — pasted YAML's `metadata.namespace` is forced to the active namespace before apply.
7. **Audit logging** — `tracing` records cluster name + verb only; tokens and apply bodies are never logged.

## Running tests

```sh
cd src-tauri && cargo test
```

Covers: encrypted store round-trip, ciphertext tampering rejection, DSL parser (forbidden tokens), destructive classifier, HMAC confirmation tokens, kubeconfig import validation.

## Out of scope (intentional)

`exec` into pods, `port-forward`, `cp`, log streaming with backpressure, RBAC explorer, plugin system, multi-context per cluster.

## Contributing

Pull requests are welcome. For larger changes please open an issue first to discuss what you'd like to change.

## Support

If k8sdesk saves you time, consider buying me a coffee ☕

<a href='https://ko-fi.com/X8X21YHKJ9' target='_blank'>
  <img height='36' style='border:0px;height:36px;' src='https://storage.ko-fi.com/cdn/kofi6.png?v=6' border='0' alt='Buy Me a Coffee at ko-fi.com' />
</a>

## License

[MIT](LICENSE)
