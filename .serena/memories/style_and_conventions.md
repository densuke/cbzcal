# Code Style and Conventions

## Language
- Rust (Edition 2024)

## Architecture
- Layered architecture: CLI -> App -> Backend (Trait)
- Traits: `CalendarBackend` for abstraction.
- Backends: `fixture` (local JSON) and `cybozu-html` (scraping).

## Error Handling
- Use `anyhow` for high-level error reporting.

## Serialization
- Use `serde` with `serde_json`, `serde_yaml`, and `toml`.

## CLI
- Use `clap` with `derive` feature.

## Naming
- Standard Rust snake_case for variables and functions, PascalCase for types and traits.
