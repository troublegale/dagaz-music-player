# Dagaz Music Player

Личный self-hosted музыкальный плеер. Сейчас реализуется общий Rust-протокол;
сервер и клиентские приложения ещё не готовы.

- [Проектирование](docs/superpowers/specs/2026-11-18-dagaz-music-player-design.md)
- [План реализации](docs/superpowers/plans/2026-11-18-dagaz-music-player-plan.md)
- [Разработка и текущий статус](docs/development.md)

Проверки из корня репозитория:

```text
cargo test --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```
