# Synthetic-Survey-Respondents-

Windows desktop app (Tauri v2 + Rust + React) that runs market-research surveys against LLM-generated synthetic respondents. Everything runs locally except calls to the Gemini API.

## Docs

- [Spec](docs/SPEC.md)
- [Data and backend flow (wizard UI)](docs/DATA_FLOW.md)
- [Implementation plan](docs/IMPLEMENTATION_PLAN.md)
- [Test plan](docs/TEST_PLAN.md)
- [SQLite schema](docs/schema.sql)

## Layout

| Path | What |
| --- | --- |
| `crates/core` | `survey-core`: database, sampling, Gemini adapter, shared types. No Tauri; builds and tests on any OS |
| `src-tauri` | `survey-app`: Tauri shell and commands. Builds on Windows (Linux needs WebKitGTK) |
| `src` | React UI, one folder per wizard step under `src/features` |
| `src/types/gen` | TypeScript types generated from `survey-core`. Don't edit by hand |

## Develop

Requires Rust (stable), Node 22 and pnpm 10.

```sh
pnpm install

# Rust core: tests also regenerate src/types/gen
cargo test -p survey-core
cargo clippy -p survey-core --all-targets -- -D warnings

# UI in a browser, against an in-memory mock of the backend
pnpm dev            # http://localhost:1420
pnpm typecheck && pnpm test

# Full desktop app (Windows)
pnpm tauri icon src-tauri/app-icon.png   # once, generates src-tauri/icons
pnpm tauri dev
pnpm tauri build                          # installers in target/release/bundle
```

The Gemini API key is entered in the app and stored in Windows Credential Manager. CI's live tests read it from the `GEMINI_API_KEY` repository secret.
