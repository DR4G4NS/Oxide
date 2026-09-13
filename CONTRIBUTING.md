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

Rust stable (edition 2021). Wire and client checks also need Java 17 and the official desktop JAR **159.7**. The JAR is an external oracle; never commit it. Historical `*_158` harness names do not imply that the current binary supports 158.1.

## Validation

Choose local checks by the changed surface. This matrix does not change the gates in [CI](.github/workflows/ci.yml).

| Changed surface | Required local evidence |
|---|---|
| Markdown only | Check relative links and anchors, verify commands/symbols against their owners, keep English/Spanish claims aligned, and run `git diff --check`. No Rust/JAR run is needed unless the edit changes a technical claim that needs it. |
| Rust, runtime configuration, or build inputs | Run the Rust checks below. |
| Executable tooling | Run checks for the affected tool; add the Rust checks when the tool changes runtime or build inputs. |
| Module boundaries or concurrency | Also run the architecture and DashMap guards below. |
| `compat/`, packet IDs, wire, save or stream codecs | Also run matching fixtures and target-JAR checks; follow [minimum wire evidence](ARCHITECTURE.md#minimum-evidence-for-a-wire-change). Report only evidence actually executed. |
| Client-predicted block behaviour | Include a regression spanning **> 360 game ticks** and crossing a `BlockSnapshot`, plus a matching real-client scenario; see the [harness guide](navigation.md#client-in-the-loop-harness). |

Rust checks:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets -- --test-threads=1
git diff --check
```

Use `--test-threads=1` for Rust tests: DashMap shard collisions can hang parallel runs. Do not increase test parallelism to mask a hang.

Module/concurrency guards:

```bash
bash tools/architecture_guard.sh
cargo run --quiet --manifest-path tools/dashmap_guard/Cargo.toml -- check . --paths src,tests --deny-warnings
```

For JAR setup, smoke commands, and their success markers, use [navigation.md](navigation.md#client-in-the-loop-harness). Report checks as passed, failed, or not run (with the reason); an exit-zero `skip` is not validation. Fix failures caused by the change and rerun the affected checks. Report unrelated failures without broadening the patch or weakening a gate.

## What we want

- Fixes and small, reviewable features on the **documented 0.1 scope** (playable vanilla Serpulo on 159.7 clients).
- Tests that fail without the change when behaviour is involved.
- Update README for product limitations and [gaps.md](gaps.md) for located divergences. Remove a closed gap rather than keeping a changelog.

## What we do not want

- Mods, plugins, custom packets, or an event bus.
- Drive-by refactors, new doc novels, or “while I was here” dependency bumps.
- Guessed packet layouts. IDs and field widths come from `compat/` + the target JAR.
- CI skipped or clippy allowed to go green.

## Pull requests

Open against **`main`**. Keep one concern per PR.

Use the PR template. The useful parts are:

| Field | What to write |
|---|---|
| **What changed** | User-visible behaviour, then internals if needed. 2–8 lines is enough. |
| **Why** | Bug, gap, or issue link. |
| **Tests** | Commands and outcomes, including not-run checks. For wire/save work, name the fixture or smoke. |
| **Notes** | Residual vanilla gap, or “none”. |

CI (`.github/workflows/ci.yml`) must pass: fmt, clippy `-D warnings`, tests, architecture/DashMap guards, and compatibility checks. You do not need to run the JAR job locally if you did not touch wire/compat; reviewers will still see it on GitHub.

Maintainers may ask for a smaller diff or a test rather than close the PR.

---

# Español

Gracias. Oxide es un servidor independiente bajo Apache-2.0. **No** es el servidor oficial de Mindustry ni está afiliado a Anuken.

Al abrir un PR ofreces el cambio bajo [Apache-2.0](LICENSE). No copies Java de Mindustry/Arc en `src/`. No relicencies `third_party/mindustry-maps/` (siguen GPLv3).

Si usas un **agente de código**, que lea [AGENTS.md](AGENTS.md). Arquitectura: [ARCHITECTURE.md](ARCHITECTURE.md). Fallos de seguridad: [SECURITY.md](SECURITY.md), no un issue público.

## Arranque

Rust stable (edición 2021). Para validar protocolo o cliente: Java 17 y JAR oficial **159.7**, externo al repositorio. Los nombres históricos `*_158` no implican compatibilidad del binario actual con 158.1.

La [matriz de validación](#validation) centraliza los comandos y cuándo ejecutarlos: documentación → enlaces, anclas, comandos y coherencia entre idiomas; runtime → fmt, clippy y suite Rust; módulos/concurrencia → guards; protocolo/save/stream → fixtures y JAR; bloques que el cliente predice → más de 360 ticks cruzando un `BlockSnapshot` y escenario con cliente real.

Usa `--test-threads=1` en los tests Rust por las colisiones de shards de DashMap. Informa resultados y verificaciones pendientes; un smoke omitido con `skip` no cuenta como aprobado. La selección local no modifica los gates de CI.

## Qué sí / qué no

Sí: arreglos y features pequeños del scope 0.1 (Serpulo vanilla, clientes 159.7), tests que fallen sin el cambio, README para limitaciones del producto y [gaps.md](gaps.md) para divergencias localizadas.

No: mods/plugins, refactors de pasada, layouts de paquetes inventados o saltarse CI para poner verde el check.

## Pull requests

Contra **`main`**. Un tema por PR. Rellena la plantilla: **qué cambió**, **por qué**, **tests** (comandos), **notas** (hueco que queda). CI tiene que pasar; no hace falta el job del JAR en local si no tocaste wire/compat.
