# Dagaz Windows Client Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Выпустить полноценный Windows-клиент GPUI: библиотека, загрузка/теги, звук, офлайн и управление устройствами.

**Architecture:** Нативный GPUI-интерфейс, Rust-протокол P1, локальная SQLite и файловые кеши. Сеть/SQL/декодирование работают вне UI-потока; звуковой движок принимается по результату P2.

**Tech Stack:** Rust, GPUI Kit, Tokio/reqwest, SQLite, Rodio/Symphonia после P2, Windows API для секретов/медиауправления.

**Spec:** [Проектирование](../specs/2026-11-18-dagaz-music-player-design.md), [общий контракт](2026-11-18-dagaz-music-player-plan.md).

## Global Constraints

- Windows 10/11 x64. Поддерживаются WAV, MP3, FLAC.
- Качество по умолчанию — высокое; выбор отдельно для стриминга и скачивания.
- До 120 секунд подкачки вперёд, в пределах 64 МиБ буфера; потоковый кеш 1 ГиБ.
- Одна постоянная аудиокопия трека на устройстве используется во всех подборках; явно скачанное не вытесняется кешем.
- Загрузка на сервер, изменение тегов и плейлистов требуют подключения. Общий план обязателен.

## Review Focus

- Второй процесс приложения не создаёт второй плеер для того же device_id: W1.
- Долгий Read/Seek/пропавшая сеть не блокируют окно и пользовательскую паузу: W2.
- Старый partial и новая ревизия не смешиваются; сбой замены сохраняет готовый файл: W3.
- Скачанный трек не исчезает после удаления другой подборки/серверного tombstone: W3.
- Запоздавший reconnect и повтор start не запускают два декодера: W4.

## W1. Макет, окно и подключение

**Depends:** P1, P2, S1; макет можно подготовить после P1.
**Files:** Create `docs/ui/{layout.md,windows-layout.svg,android-layout.svg}`, `apps/windows/{Cargo.toml,src/main.rs,src/lib.rs,src/app.rs,src/connection.rs}`, `apps/windows/tests/connection.rs`; Modify workspace.
**Interfaces:** Produces `ServerConnection` с base_url, device_id, reqwest-клиентом и защищённым токеном; `async fn pair(base_url: url::Url, code: &str, device_name: &str) -> Result<ServerConnection, ClientError>`. Токен сохраняется через Windows DPAPI; незашифрованный конфиг содержит только адрес/настройки. Основной исполнитель фоновых задач — Tokio.

- [ ] Подготовить два статических макета. Windows: навигация библиотека/плейлисты/скачанное, общий поиск, центральная таблица/альбомы, раскрываемая очередь и нижнее воспроизведение. Android: библиотека/поиск/скачанное, мини-плеер и полный экран текущего трека. Использовать спокойную тёмную тему штатных компонентов, акцент, читаемые статусы; не создавать отдельную веб-продуктовую версию. Показать макеты до наполнения экранов и зафиксировать замечания пользователя в layout.md.
- [ ] Написать `pairing_and_single_instance`: корректный/неверный код, сохранение/повторный запуск, отзыв и второй процесс. `cargo test -p dagaz-windows --test connection` сначала должен FAIL.

```rust
assert_eq!(restored_device_id, paired_device_id);
assert!(!settings_text.contains(&raw_token));
assert_eq!(active_audio_processes_for_profile, 1);
assert!(revoked_token_requires_pairing);
```

- [ ] Реализовать окно согласованного макета, привязку и восстановление connection, защиту одного экземпляра Windows через именованный mutex. Отказ второго экземпляра происходит до запуска аудио. Вне тестового loopback принимать HTTPS, секреты отправлять только своему origin, ошибки подключения показывать без потери введённого адреса.
- [ ] Повторить тест, `cargo run --release -p dagaz-windows`: PASS; проверить фокус клавиатуры, масштабирование окна и кириллицу. Блокирующие операции отсутствуют в UI callbacks.
- [ ] Контрольная точка: `git add apps/windows docs/ui Cargo.toml Cargo.lock`; `git commit -m "feat: add Windows shell and server pairing"`.

## W2. Первый сквозной сценарий и воспроизведение

**Depends:** W1, S2–S4.
**Files:** Create `apps/windows/src/{library,audio,range_source,system_media}.rs`, `apps/windows/tests/{library,audio}.rs`; Modify `src/{app,connection}.rs`.
**Interfaces:** `AudioPlayer::new() -> (AudioPlayer, tokio::sync::watch::Receiver<AudioStatus>)`; `async fn prepare(&mut self, media: MediaDescriptor, position_ms: i64) -> Result<(), PlayerError>`, Остальные методы — `async fn play(&mut self) -> Result<(), PlayerError>`, `async fn pause(&mut self) -> Result<(), PlayerError>`, `async fn stop(&mut self) -> Result<(), PlayerError>` и `async fn seek(&mut self, position_ms: i64) -> Result<(), PlayerError>`. AudioStatus содержит track_id, position_ms, state: `AudioState::{Loading, Playing, Paused, Waiting, Stopped, Failed}`. Системное управление передаёт те же команды, что интерфейс, без отдельного player instance.

- [ ] Написать `search_upload_play_and_seek_without_full_download`, `user_pause_survives_reconnect`, `next_track_is_prefetched`: загрузка коротких трёх форматов, поиск, начало/пауза/перемотка и очередь. Использовать детерминированный HTTP-транспорт с управляемым разрывом, не только живой сервер.

```rust
assert!(audio_started_before_download_complete);
assert_eq!(status_after_reconnect.state, AudioState::Paused);
assert!(position_error_ms <= 500);
assert!(buffer_bytes <= 64 * 1024 * 1024);
```

- [ ] Выполнить `cargo test -p dagaz-windows --test library --test audio`: ожидается FAIL.
- [ ] Реализовать постраничный поиск/каталог, минимальную загрузку файла с продолжением по offset, original/high/low и звуковой источник из проверенного подхода P2. Ограничить буфер временем/байтами, следующий трек готовить заранее. HTTP Range отменяется при seek/stop; зависшее чтение не мешает остановке. При 425 показывать подготовку, при сетевом сбое — ожидание/повторы 1,2,4,8,16,30 секунд с jitter. Ошибки 401/403/404/415/422 не повторять бесконечно. Добавить очередь, повтор, перемешивание с сохраняемым seed и материализованным play_order и системные медиакнопки. До W4 воспроизведение — промежуточный локальный режим, не готовая межустройственная функция.
- [ ] Повторить тесты и живой сценарий против S1–S4: PASS; музыка начинается до полного скачивания, UI реагирует во время сетевого ожидания, seek продолжает нужную ревизию.
- [ ] Контрольная точка: `git add apps/windows`; `git commit -m "feat: browse and play the server library on Windows"`.

## W3. Локальный каталог и единая копия скачанного

**Depends:** W2, S4, S6.
**Files:** Create `apps/windows/src/{downloads,sync}.rs`, `apps/windows/migrations/0001_local.sql`, `apps/windows/tests/downloads.rs`; Modify `src/{app,library,audio}.rs`.
**Interfaces:** Produces локальные таблицы catalog, playlist snapshots, local_media, pins, transfers, sync cursor/generation. `LocalMedia { track_id, revision, quality, path, sha256, byte_length }`, уникальный track_id среди готовых копий. `enum DownloadDecision { Reuse, Fetch }`; `fn resolve_download(existing: Option<&LocalMedia>, requested: Quality) -> DownloadDecision` переиспользует равное/лучшее, новый высокий профиль заменяет общий файл. Явное понижение — отдельная команда, влияющая на все подборки.

- [ ] Написать `downloads_share_copy_survive_revisions_and_tombstones`: два плейлиста и альбом с одним треком; снять один pin, сменить качество, прервать скачивание, обновить теги/ETag, удалить серверный трек, закрыть/открыть приложение без сети, смоделировать ENOSPC.

```rust
assert_eq!(ready_local_copies_for_track, 1);
assert!(file_exists_after_removing_one_group);
assert!(old_file_usable_after_failed_replacement);
assert!(server_deleted_track_visible_in_downloads);
assert_eq!(offline_search_hits_for_cyrillic_title, 1);
assert!(!new_revision_contains_old_partial_bytes);
```

- [ ] Выполнить `cargo test -p dagaz-windows --test downloads`: ожидается FAIL.
- [ ] Реализовать порционную синхронизацию metadata/playlist membership на диск, применение sequence/tombstones, локальный поиск по скачанному, manifests и pins. Скачивать во временный файл с проверкой hash/length, затем транзакционно переключать локальную запись; на Windows учитывать открытый аудиодескриптор и отложенное удаление старого файла. Готовая копия имеет приоритет перед сетью. Изменение тегов обновляет отображение, аудиофайл помечает устаревшим до явного обновления. Скачанный плейлист — снимок; обновление явное, снятие одного pin не удаляет другие. Серверный tombstone сохраняет файл/обложку/метаданные в «Скачанном» и отменяет лишь незавершённое скачивание удалённого файла. Потоковый LRU-кеш отдельно ограничен 1 ГиБ; при завершении явного скачивания его дублирующие байты убираются.
- [ ] Повторить тест, затем холодный запуск Windows-клиента с отключённой сетью: PASS. Статусы partial/ready/outdated/deleted_on_server и фактическое качество видны во всех контекстах. Смена streaming-quality не перескачивает готовый offline-файл.
- [ ] Контрольная точка: `git add apps/windows`; `git commit -m "feat: add durable shared offline downloads on Windows"`.

## W4. Устройство воспроизведения и перенос

**Depends:** W3, S7.
**Files:** Create `apps/windows/src/playback.rs`, `apps/windows/tests/playback.rs`; Modify `src/{app,audio,system_media,sync}.rs`.
**Interfaces:** `PlaybackController` использует ServerConnection и AudioPlayer; все Play/Pause/Seek/Next/Transfer из UI/медиакнопок проходят через него. Produces обработку prepare/stop/start, ack/report, экран списка устройств. Epoch/sequence/operation_id — из P1, собственный второй формат не вводится.

- [ ] Написать `reconnect_reconciles_before_reporting_and_duplicate_start_is_safe`: офлайн-исполнитель, другой владелец, устаревшее событие, повтор start/ack, перенос из гарнитуры/UI, продолжение тем же владельцем после простого сетевого разрыва.

```rust
assert_eq!(local_audio_starts_for_same_operation, 1);
assert_eq!(old_epoch_reports_sent_before_reconcile, 0);
assert_eq!(state_after_new_owner_received, AudioState::Stopped);
assert_eq!(reported_queue_index, expected_occurrence);
```

- [ ] Выполнить `cargo test -p dagaz-windows --test playback`: ожидается FAIL.
- [ ] Подключить WebSocket, сначала получать PlaybackState после reconnect, затем разрешать reports. prepare только буферизует, start запускает звук после серверного решения, stop подтверждается после фактической остановки. Отправлять прогресс раз в секунду во время активного воспроизведения и при seek/pause; sequence монотонен в epoch. При отсутствии сети продолжать локальное воспроизведение и сохранять play_order/режимы. После reconnect при прежних epoch/владельце сначала согласовать локальную очередь, затем reports; при новой epoch применить серверную команду, при новом владельце остановиться. Следовать переданному play_order, а не повторно строить shuffle из seed. Доступность устройства определяется соединением, недоступное устройство не показывать как успешно принявшее перенос. Локальная громкость остаётся настройкой устройства.
- [ ] Повторить тест и перенос между двумя тестовыми Windows-процессами с разными профилями: PASS. Основная двухплатформенная проверка остаётся в R1.
- [ ] Контрольная точка: `git add apps/windows`; `git commit -m "feat: control playback devices from Windows"`.

## W5. Полное управление библиотекой и доступность

**Depends:** W2, S5. Задача выполняется до W3/W4 по маршруту; новых downloads/playback модулей для редактора не требует.
**Files:** Create `apps/windows/src/editor.rs`, `apps/windows/tests/editor.rs`; Modify `src/{app,library,connection}.rs`, `docs/ui/layout.md`.
**Interfaces:** Uses P1 metadata/batch/artwork, playlists и upload APIs. `async fn upload_file(connection: &ServerConnection, path: &std::path::Path, operation_id: uuid::Uuid) -> Result<Job, ClientError>` возобновляется по UploadState; выбранные элементы групповой правки сохраняют expected_revision каждого трека. Все подтверждения показывают разницу удаления из плейлиста, с устройства и с сервера.

- [ ] Написать `batch_editor_preserves_selection_and_reports_partial_success`: две папки с одинаковыми именами файлов, дерево с симлинком/циклом, групповая правка, очистка поля, конфликт одного трека, невалидная обложка, playlist reorder с повтором, сетевой разрыв. Обход папок не следует циклическим ссылкам; ошибка файла не отменяет всю партию.

```rust
assert_eq!(unique_uploaded_files, expected_regular_files);
assert_eq!(successful_edit_count, 2);
assert_eq!(conflict_count, 1);
assert!(failed_items_still_selected_for_retry);
assert_eq!(playlist_order_after_reload, requested_order);
```

- [ ] Выполнить `cargo test -p dagaz-windows --test editor`: ожидается FAIL.
- [ ] Реализовать drag-and-drop/выбор файлов и папок, прогресс и повтор отдельных ошибок, многовыделение, редактор тегов и обложек, playlist CRUD/reorder. Действия обновления/понижения качества/удаления скачанного реализуются вместе с W3. Применение batch только к выбранным записям и их ревизиям; конфликт показывает актуальные данные, без молчаливой перезаписи. При офлайне действия изменения библиотеки недоступны с понятной причиной, просмотр/поиск/очередь работают. Окна следуют согласованным макетам, кнопки имеют имена доступности; поиск/списки виртуализированы, обложки загружаются асинхронно.
- [ ] Повторить тесты и вручную пройти только клавиатурой: поиск, выделение, контекстное действие, редактор и плейлист. Клавиатурный путь к выбору устройства проверяется после W4 в R2. PASS; отсутствие сети и прогресс не скрывают текущий трек. Финальные задержки и внешность проверяются R2.
- [ ] Контрольная точка: `git add apps/windows docs/ui/layout.md`; `git commit -m "feat: complete Windows library management"`.
