# Third-party material and provenance

Read together with [LICENSE](LICENSE) and [NOTICE](NOTICE). Original implementation in this repository is offered under Apache-2.0. Apache-2.0 is **not** applied to a third-party item merely because it sits beside original code.

## Mindustry reference

[Anuken/Mindustry](https://github.com/Anuken/Mindustry) is authored by Anuken and licensed under GPLv3. The compatibility target in `compat/current.toml` is source tag `v159.7` at commit `c9686eb5d0ae5dd47ee02c40f99f7d5018ccbc8c`. The source tree, official JARs, and official Java tests are external compatibility references/oracles; they are not copied into this repository and are not relicensed here.

Optional CI checks out that exact tag and fetches Arc (`archash` `208a754044`, Apache-2.0 upstream) into a temporary runner directory. Neither checkout is a vendored repository artifact.

This project is **not** an official Mindustry server and makes no affiliation, sponsorship, or endorsement claim.

## Repository classification

| Material | Classification | Boundary |
| --- | --- | --- |
| `src/`, Rust tests, `examples/`, Python/bash tools, README / ARCHITECTURE / CONTRIBUTING / AGENTS / benchmark, `assets/` | `ORIGINAL` | Independently authored; Apache-2.0 where the contributor has the right to license it. The Oxide name and logo are project marks (Apache-2.0 does not grant trademark rights). |
| `tests/parity/probes/*.java` and `tools/**/*.java` | `ORIGINAL` | Independently authored probes that call APIs on an external Mindustry/Arc JAR. Those classes keep their own licenses. |
| `compat/**/*.json` | `BEHAVIORAL_REFERENCE_ONLY` / generated fixture | Wire/runtime facts and schemas, not GPL source. An all-Apache distribution claim still needs human review. |
| `src/game/*.tsv`, official-content portions of `content.rs` / `unit_types.rs` / `block_names.rs` | generated fixture + review | IDs and balance tables extracted or transcribed from official JAR/ContentLoader output. |
| `tools/inspect/units_158_jar_dump.tsv` | generated fixture + review | 158.1 desktop-JAR content dump; not copied Java source. |
| `tests/fixtures/msav-world-entities/*`, `src/dummy_world.dat`, `pruebas01.msav` | `GENERATED_FIXTURE` | Binary test inputs. `pruebas01.msav` is a project custom-map export. |
| `third_party/mindustry-maps/*.msav` | `THIRD_PARTY` (GPLv3) | Verbatim official Serpulo campaign maps from Anuken/Mindustry `v159.7`. **Not Apache-2.0.** License text in that folder. |
| `tests/fixtures/official-159.7/*` | generated fixture + review | Exported/validated with the official v159.7 JAR; no Java source. |
| Packages named in `Cargo.lock` | `THIRD_PARTY` | Registry dependencies keep their own licenses. |

`NEEDS_HUMAN_REVIEW` entries are retained on purpose. This file does not grant permission to redistribute third-party map/save data.

## What is not present

The tracked tree contains no official `Mindustry.jar`, `server-release.jar`, `assets.jar`, bundled Mindustry source checkout, or copied GPL Java test class. Local bench JARs under `tools/bench/` are gitignored.

Official campaign maps in `third_party/mindustry-maps/` are GPLv3 works of Anuken; they are not relicensed by Apache-2.0.
