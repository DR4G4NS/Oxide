# Contributing

<p align="center">
  <a href="#english">English</a> · <a href="#español">Español</a>
</p>

---

# English

Thanks for helping. Oxide is an independently authored Apache-2.0 server. It is **not** the official Mindustry server and is not affiliated with Anuken.

By opening a pull request you offer the change under [Apache-2.0](LICENSE) (see LICENSE §5), subject to [NOTICE](NOTICE) and [THIRD_PARTY.md](THIRD_PARTY.md). Do not paste Mindustry/Arc Java into `src/`. Do not relicense files under `third_party/mindustry-maps/` (those stay GPLv3).

If a **coding agent** will touch the tree, point it at [AGENTS.md](AGENTS.md) first. Architecture rails: [ARCHITECTURE.md](ARCHITECTURE.md). Security reports: [SECURITY.md](SECURITY.md) (not a public issue).

## Setup

Rust stable (edition 2021). Optional: official desktop JAR **159.7** for wire smokes; **158.1** only for historical scripts named `*_158`.

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets -- --test-threads=1
```

`--test-threads=1` is required. The JAR is an oracle, not a vendor tree — never commit it.

## What we want

- Fixes and small, reviewable features on the **documented 0.1 scope** (playable vanilla Serpulo on 159.7 clients).
- Tests that fail without the change when behaviour is involved.
- README updates when a **gap** is closed or newly found.

## What we do not want

- Mods, plugins, custom packets, or an event bus.
- Drive-by refactors, new doc novels, or “while I was here” dependency bumps.
- Guessed packet layouts. IDs and field widths come from `compat/` + the target JAR.
- CI skipped, clippy allowed, or `CERTIFIED_RUNTIME_SHA` rewritten to go green.

## Pull requests

Open against **`main`**. Keep one concern per PR.

Use the PR template. The useful parts are:

| Field | What to write |
|---|---|
| **What changed** | User-visible behaviour, then internals if needed. 2–8 lines is enough. |
| **Why** | Bug, gap, or issue link. |
| **Tests** | Commands you ran. For wire/save work, name the fixture or smoke. |
| **Notes** | Residual vanilla gap, or “none”. |

CI (`.github/workflows/ci.yml`) must pass: fmt, clippy `-D warnings`, tests, architecture/DashMap guards, cert ledger. You do not need to run the JAR job locally if you did not touch wire/compat; reviewers will still see it on GitHub.

Maintainers may ask for a smaller diff or a test rather than close the PR.

---

# Español

Gracias. Oxide es un servidor independiente bajo Apache-2.0. **No** es el servidor oficial de Mindustry ni está afiliado a Anuken.

Al abrir un PR ofreces el cambio bajo [Apache-2.0](LICENSE). No copies Java de Mindustry/Arc en `src/`. No relicencies `third_party/mindustry-maps/` (siguen GPLv3).

Si usas un **agente de código**, que lea [AGENTS.md](AGENTS.md). Arquitectura: [ARCHITECTURE.md](ARCHITECTURE.md). Fallos de seguridad: [SECURITY.md](SECURITY.md), no un issue público.

## Arranque

Rust stable. JAR 159.7 opcional para smokes de wire; 158.1 solo para scripts `*_158`.

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets -- --test-threads=1
```

`--test-threads=1` es obligatorio. No subas el JAR.

## Qué sí / qué no

Sí: arreglos y features pequeños del scope 0.1 (Serpulo vanilla, clientes 159.7), tests que fallen sin el cambio, README si cierra o aparece un hueco.

No: mods/plugins, refactors de pasada, layouts de paquetes inventados, saltarse CI o reescribir `CERTIFIED_RUNTIME_SHA` para poner verde el check.

## Pull requests

Contra **`main`**. Un tema por PR. Rellena la plantilla: **qué cambió**, **por qué**, **tests** (comandos), **notas** (hueco que queda). CI tiene que pasar; no hace falta el job del JAR en local si no tocaste wire/compat.
