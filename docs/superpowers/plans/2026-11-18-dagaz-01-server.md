# Dagaz Server Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Получить один сервер, предоставляющий весь API личной библиотеки и координацию устройств.

**Architecture:** Rust/Axum, локальная SQLite, закрытый RustFS, долговечная очередь задач внутри приложения. Доступ к файлам проходит через API; публикация ревизии в каталоге следует только после успешной записи объекта.

**Tech Stack:** Rust, Axum/Tokio, SQLx/SQLite/FTS5, S3 SDK, Lofty, FFmpeg, WebSocket.

**Spec:** [Проектирование](../specs/2026-11-18-dagaz-music-player-design.md), [общий контракт](2026-11-18-dagaz-music-player-plan.md).

## Global Constraints

- Один сервер обслуживает одну личную библиотеку; правки метаданных записываются в аудиофайлы на сервере.
- Поддерживаются WAV, MP3, FLAC. Пользователь не задаёт ограничение на количество треков.
- Высокое: MP3 до 320 кбит/с. Низкое: MP3 до 128 кбит/с. Оригинал не перекодируется.
- Число параллельных перекодирований начально ограничено одним; старые ревизии сохраняются на 24 часа и на время действующих чтений.
- Все сетевые типы/ошибки — из P1; общий план и его ограничения обязательны.

## Review Focus

- Потеря ответа PATCH/complete/metadata не приводит к повторному импорту: S2/S5.
- Нехватка диска/сбой S3 не повреждает опубликованный файл: S2/S5/S8.
- Одноимённые альбомы разных исполнителей и кириллица не смешиваются: S4/S5.
- Новый объект и новая запись SQLite не образуют распределённую транзакцию: S3/S5/S8 проверяют восстановление.
- Старые ack/heartbeat/reconnect не переносят владение обратно: S7.

## S1. Защищённый сервер и привязка устройств

**Depends:** P1, P5.
**Files:** Create `apps/server/{Cargo.toml,src/main.rs,src/lib.rs,src/config.rs,src/auth.rs}`, `apps/server/migrations/0001_devices.sql`, `apps/server/tests/{common/mod.rs,auth.rs}`; Modify корневой `Cargo.toml`.
**Interfaces:** Produces `Config::from_env() -> Result<Config, AppError>`, `AppState { db: SqlitePool, s3: aws_sdk_s3::Client, config: Config, ... }`, `pub async fn build_app(config: Config, db: SqlitePool, s3: aws_sdk_s3::Client) -> Result<axum::Router, AppError>`. Тестовый `TestServer::spawn() -> TestServer` предоставляет `url`, `http: reqwest::Client`, `token`, `db_path`, изолированный S3-prefix и управляемое завершение. HTTP: `/healthz`, `/pair`, `/devices` из P1.

- [ ] Написать `auth_rejects_missing_revoked_and_reused_pairing_code`, включая два одновременных запроса с одним кодом. Тестовый helper запускает настоящий router на свободном loopback-порту, отдельной БД и S3-prefix.

```rust
assert_eq!(anonymous_status, 401);
assert_eq!(successful_pairings_for_one_code, 1);
assert_eq!(revoked_device_status, 401);
assert!(!stored_token_value.contains(&raw_token));
```

- [ ] Запустить `cargo test -p dagaz-server --test auth`: ожидается FAIL отсутствующего/неверного доступа.
- [ ] Реализовать точные маршруты, миграцию, одноразовые случайные коды, индивидуальные случайные токены с хранением хеша, извлечение device_id из Bearer. CLI `dagaz pair-code` печатает новый короткоживущий код; первый код создаётся установщиком. Ограничить срок кода 10 минут и попытки привязки 10/мин на IP, защищённые запросы 300/мин на устройство. Секреты не логировать. Сроки/лимиты документируются как эксплуатационные значения; отозванный токен закрывает его WebSocket.
- [ ] Повторить тест; `cargo test -p dagaz-server --test auth` и общий Rust fmt/clippy: PASS. Невалидные конфигурация/SQLite/S3 credentials приводят к диагностируемому отказу запуска, без выдачи секретов.
- [ ] Контрольная точка: `git add apps/server Cargo.toml Cargo.lock`; `git commit -m "feat: add authenticated personal server"`.

## S2. Продолжаемая загрузка и импорт

**Depends:** S1, P4.
**Files:** Create `apps/server/src/{uploads,jobs}.rs`, `apps/server/migrations/0002_media_jobs.sql`, `apps/server/tests/uploads.rs`; Modify `src/{lib,config}.rs`.
**Interfaces:** Consumes `AppState`, P1 UploadState/Job. Produces upload/job endpoints и `pub async fn run_jobs(state: Arc<AppState>, shutdown: tokio::sync::watch::Receiver<bool>) -> Result<(), AppError>`. `Job.kind` различает import/retag/transcode/cleanup; очередь, прогресс и результат сохраняются в SQLite. Следующие задачи добавляют свои ветки обработки, без плагинного реестра.

- [ ] Написать `upload_resume_duplicate_and_failed_import`: WAV/MP3/FLAC, потеря ответа после части/complete, одинаковые имена с разными байтами, точный повтор, неверная контрольная сумма, повреждённый файл и искусственная нехватка места.

```rust
assert_eq!(resumed_offset, committed_bytes);
assert_eq!(completed_twice_track_id, first_track_id);
assert_ne!(same_filename_different_bytes_id, first_track_id);
assert_eq!(visible_partial_tracks, 0);
assert_eq!(disk_full_status, 507);
```

- [ ] Выполнить `cargo test -p dagaz-server --test uploads`: ожидается FAIL.
- [ ] Миграция S2 создаёт минимальные tracks/media_revisions/uploads/jobs/idempotency/changes, а S4 расширяет каталог. Реализовать POST/PATCH/GET/complete с частями до 8 МиБ, default max file 8 ГиБ (серверная настройка границы), проверкой размера/хеша, сериализацией записи одного upload, fsync перед коммитом смещения. После сбоя лишние байты staging обрезаются до сохранённого offset. Незавершённые upload истекают через 24 часа. Тип файла определяется содержимым. Имя файла хранится как отображаемое значение, объект получает UUID-ключ без использования клиентского пути. Точный checksum ищется по известным импортам/ревизиям действующих треков. Публикация объекта предшествует атомарной записи каталога/результата/Change. Незавершённые jobs восстанавливаются после рестарта; повторный запуск проверяет существующий результат.
- [ ] Повторить тест и перезапуск во время импорта: PASS, один логический трек и один результат операции. Отдельно проверить имена `../song.mp3`, Unicode, CR/LF и пустые теги; пути/заголовки не повреждаются.
- [ ] Контрольная точка: `git add apps/server`; `git commit -m "feat: import resumable audio uploads"`.

## S3. Неизменные ревизии и потоковая выдача

**Depends:** S2, P5.
**Files:** Create `apps/server/src/media.rs`, `apps/server/tests/media.rs`; Modify `src/lib.rs`, миграции ревизий S2.
**Interfaces:** Produces authenticated GET/HEAD `/tracks/{id}/media?revision=...&quality=...`, `MediaDescriptor`, удержание используемой ревизии на время чтения. Ключ объекта содержит `track_id/revision/quality`; ETag/sha256/длина соответствуют конкретному объекту, не изменяемому при активном чтении.

- [ ] Написать `range_if_range_and_revision_change`: полный ответ, HEAD, открытый/закрытый Range, выход за конец файла, отмена чтения, продолжение после смены тега/истечения старой ревизии. Частично отсутствующие будущие функции моделировать подготовленной ревизией в тестовой БД.

```rust
assert_eq!(range_status, 206);
assert_eq!(range_bytes, expected_slice);
assert_eq!(invalid_range_status, 416);
assert_eq!(head_content_length, original_length);
assert_ne!(old_etag, new_etag);
assert!(!mixed_revision_bytes_returned);
```

- [ ] Выполнить `cargo test -p dagaz-server --test media`: ожидается FAIL.
- [ ] Реализовать проксирование байтов S3 с backpressure, без загрузки целого объекта в RAM; корректные Content-Type/Length/Range/ETag/If-Range и безопасный Content-Disposition. HEAD не читает весь файл. Чтение удалённой ревизии даёт 410; несовпавший If-Range не продолжает старый partial новыми байтами. Сжатое представление до S6 отвечает 425, не выдаёт другой профиль незаметно. Ограничить одновременно открытые потоки и закрывать upstream при отмене клиента.
- [ ] Повторить тест, скачать файл с продолжением и сравнить SHA-256: PASS; памяти процесса не требуется размер всего файла.
- [ ] Контрольная точка: `git add apps/server`; `git commit -m "feat: stream immutable media revisions"`.

## S4. Каталог, поиск, плейлисты и синхронизация

**Depends:** S2.
**Files:** Create `apps/server/src/{library,sync}.rs`, `apps/server/migrations/0003_catalog.sql`, `apps/server/tests/{library,sync}.rs`; Modify `src/{lib,uploads}.rs`.
**Interfaces:** Produces операции tracks/albums/artists/playlists, `/snapshot`, `/changes`, каталоговые события `/events`. Consumes Track/Playlist/Change из P1. Полный снимок стартует с generation и high-watermark; записи идут keyset-страницами по постоянному ID, затем клиент применяет дельту после high-watermark. Каждое изменение несёт sequence; более старое состояние не перезаписывает новое. Событие WS — уведомление, HTTP-дельта — надёжное восстановление.

- [ ] Написать `unicode_search_album_identity_and_playlist_order` и `snapshot_catches_concurrent_changes_and_deletes`. Проверить кириллицу/кавычки/FTS-синтаксис как обычный текст, одноимённые альбомы разных album_artist, повторы в плейлисте, конфликт ревизии и удаление между страницами снимка.

```rust
assert_ne!(album_for_artist_a, album_for_artist_b);
assert_eq!(playlist.track_ids, vec![track_b, track_a, track_b]);
assert_eq!(stale_playlist_write_status, 409);
assert_eq!(after_snapshot_and_changes, current_server_catalog);
assert!(!deleted_track_is_visible);
```

- [ ] Запустить `cargo test -p dagaz-server --test library --test sync`: ожидается FAIL.
- [ ] Реализовать SQLite FTS5 с Unicode-поиском по title/artist/album, буквальным безопасным запросом и стабильной пагинацией. Сортировать альбом по disc/track с устойчивым fallback. CRUD плейлистов транзакционен, повтор идентификатора в track_ids допустим. Удаление записи плейлиста не удаляет Track. Записывать Change в той же транзакции; хранить журнал минимум 30 дней, после истечения — 410 `cursor_expired`. При restore менять sync generation. Удаление трека создаёт tombstone и обновляет связанные плейлисты; физический объект очищается позже.
- [ ] Повторить тесты: PASS. Проверить постраничную обработку 100 тысяч записей без загрузки всего каталога в память; пока зафиксировать цифры, окончательные цели — R2.
- [ ] Контрольная точка: `git add apps/server`; `git commit -m "feat: add searchable synchronized library"`.

## S5. Обложки и запись тегов в файлы

**Depends:** S3, S4, P4.
**Files:** Create `apps/server/src/tags.rs`, `apps/server/tests/tags.rs`; Modify `src/{library,jobs,media}.rs` и миграцию задач/ревизий.
**Interfaces:** Produces artwork, metadata и metadata-batch endpoints. MetadataPatch missing/null/value работает как в P1. Успех Job содержит новую Track.revision; batch возвращает отдельный результат каждого трека и `partial` при смешанном исходе.

- [ ] Написать `retag_is_atomic_preserves_audio_and_survives_retry`: три формата, очистка album, изменение обложки, конфликт двух клиентов, потеря ответа, сбой S3 после записи, падение до/после SQLite commit, одна плохая запись в партии.

```rust
assert_eq!(pcm_hash_before, pcm_hash_after);
assert_eq!(successful_revision_increments, 1);
assert_eq!(conflicting_edit_status, 409);
assert!(previous_revision_still_readable_after_failed_save);
assert_eq!(batch_status, "partial");
```

- [ ] Выполнить `cargo test -p dagaz-server --test tags`: ожидается FAIL.
- [ ] Реализовать рабочую копию, Lofty-политику из P4, проверку результата и неизменяемый новый объект. Переключать revision/связи/FTS/Change одной SQLite-транзакцией с CAS. Удалять только осиротевшие версии после защитного интервала; действующие чтения удерживают старую версию. Принимать JPEG/PNG до 10 МиБ и 25 мегапикселей, проверять декодирование и размеры до генерации миниатюр. Сетевые URL обложек не загружать. Партия до 100 треков за запрос; клиент делит более крупное выделение на партии.
- [ ] Повторить тест и прочитать физический файл из S3 независимым разбором: PASS. Неизменённые теги/чанки из P4 сохранены; неправильная картинка даёт явную ошибку, рабочий аудиофайл не повреждён.
- [ ] Контрольная точка: `git add apps/server`; `git commit -m "feat: persist safe audio metadata edits"`.

## S6. Качества и снимки скачиваемых подборок

**Depends:** S3–S5, P6.
**Files:** Modify `apps/server/src/{jobs,media,library}.rs`; Create `apps/server/tests/{qualities,manifests}.rs`, `apps/server/migrations/0004_download_manifests.sql`.
**Interfaces:** Produces готовые original/high/low, события готовности Job и download-manifests из P1. Снимок состава хранит ссылки на конкретные ревизии и quality, читается страницами и истекает через 24 часа; клиент сохраняет снимок у себя и при истечении получает новый для явного обновления, не смешивая ревизии.

- [ ] Написать `quality_generation_is_deduplicated_and_manifest_is_snapshot`: MP3 с битрейтом ниже целевого, одновременные запросы одного представления, FFmpeg failure/restart, изменение состава плейлиста после создания manifest, отсутствие готового качества.

```rust
assert_eq!(effective_quality_for_low_bitrate_mp3, Quality::Original);
assert_eq!(transcodes_for_same_track_revision_profile, 1);
assert_eq!(manifest_before_edit.track_ids, manifest_after_edit.track_ids);
assert_eq!(pending_media_status, 425);
assert_eq!(peak_ffmpeg_processes, 1);
```

- [ ] Выполнить `cargo test -p dagaz-server --test qualities --test manifests`: ожидается FAIL.
- [ ] Добавить долговечные задания FFmpeg с профилями P6, уникальностью track/revision/profile, пределом одного процесса, таймаутом и очисткой partial output. После импорта/retag ставить новые качества в очередь, оригинал доступен сразу. Передавать аргументы процессу массивом, без shell-интерполяции. Теги/обложка включаются в производные. В manifest хранить снимок и возвращать дескриптор после готовности качества; клиент не объявляет файл готовым раньше финальной длины/checksum.
- [ ] Повторить тесты: PASS. Рестарт не создаёт два процесса для одной задачи; ошибка одного представления не блокирует оригинал и другие задачи навсегда.
- [ ] Контрольная точка: `git add apps/server`; `git commit -m "feat: prepare quality variants and download manifests"`.

## S7. Координатор воспроизведения и перенос

**Depends:** S1, S4, S6.
**Files:** Create `apps/server/src/playback.rs`, `apps/server/migrations/0005_playback.sql`, `apps/server/tests/playback.rs`; Modify `src/{lib,sync}.rs`.
**Interfaces:** Consumes PlaybackState/Command/Report/Ack. Produces `/playback`, commands/reports/acks и фазы WebSocket prepare/stop/start. Сервер сериализует операции одной библиотеки; новые управляющие команды могут приходить с любого привязанного устройства, фактический прогресс — только от владельца.

- [ ] Написать `transfer_fences_old_owner_and_is_idempotent` с двумя тестовыми WebSocket-клиентами и управляемым временем Tokio: успешный перенос, неподготовленный target, потеря source, два одновременных переноса, повтор operation_id, устаревший ack/report, повторяющийся трек в очереди, remote pause при том же владельце и старый heartbeat, сохранение play_order после shuffle/переноса.

```rust
assert_eq!(playing_online_devices, 1);
assert!(new_state.epoch > old_state.epoch);
assert_eq!(accepted_old_owner_reports, 0);
assert_eq!(replayed_operation_result, first_operation_result);
assert_eq!(new_state.queue_index, expected_occurrence);
assert!(position_error_ms <= 500);
```

- [ ] Выполнить `cargo test -p dagaz-server --test playback`: ожидается FAIL.
- [ ] Реализовать prepare target → stop source + фактическая позиция → persist new epoch/owner → start target. Начальные таймауты: prepare 10 секунд, stop-ack 3 секунды; обычный перенос при неподтверждённой остановке завершается ошибкой. `play_here` — явный захват с предупреждением о возможном офлайне старого устройства. До prepare failure исходный продолжает; после остановки и failure — согласованная пауза. Epoch и operation_id переживают рестарт; незавершённый перенос восстанавливается в паузу с fencing, без автоматического старта двух клиентов. Каждая принятая управляющая команда увеличивает epoch; reports её не меняют, принимаются только от владельца с актуальной epoch и возрастающим sequence. Порядок shuffle материализуется в play_order и передаётся без повторного генератора на клиентах; queue_index сохраняет правильное вхождение дублированного трека. Replace_queue после офлайна допустим только для прежней epoch/владельца, без опережающего старого report.
- [ ] Повторить тест: PASS. Вариант «телефон офлайн → компьютер играет → телефон reconnect» сначала выдаёт серверное состояние, затем отклоняет старую очередь/прогресс. Таймаут WebSocket не считается доказательством остановки аудио.
- [ ] Контрольная точка: `git add apps/server`; `git commit -m "feat: coordinate playback and device handoff"`.

## S8. Перезапуск, уборка и административные операции

**Depends:** S2–S7.
**Files:** Create `apps/server/src/maintenance.rs`, `apps/server/tests/maintenance.rs`; Modify `src/{main,jobs,config}.rs`; Create `docs/backup.md` (техническая часть, эксплуатационная проверка в R3).
**Interfaces:** Produces CLI `dagaz doctor`, `dagaz backup --output <directory>`, `dagaz restore --input <directory>`, `dagaz pair-code`; startup recovery и периодическую уборку. Backup: manifest с checksum, SQLite backup API, текущие оригиналы/обложки, конфигурация и защищённые секреты; качества пересоздаются. Restore выполняется при остановленном приложении.

- [ ] Написать `backup_recovery_and_cleanup_preserve_referenced_objects`: активное чтение старой версии, orphan после failed publish, истёкший upload/manifest, рестарт running-job, ENOSPC, восстановление в чистую БД.

```rust
assert_eq!(restored_playlist_order, original_playlist_order);
assert_eq!(restored_original_hashes, backup_original_hashes);
assert!(active_read_revision_retained);
assert!(!unreferenced_expired_object_retained);
assert_ne!(restored_sync_generation, previous_sync_generation);
```

- [ ] Выполнить `cargo test -p dagaz-server --test maintenance`: ожидается FAIL.
- [ ] Реализовать startup recovery, диагностируемые failed jobs и закрытие сервера без повреждения staging. Для backup остановить публикацию изменений/GC на время снимка и копирования связанных объектов; чтение музыки продолжает работать. Экспорт секретов имеет права только владельца и не выводится в лог. Restore проверяет manifest до замены рабочего состояния, отказывает на непустой рабочей установке без явного режима восстановления, меняет sync generation и сбрасывает владельца/незавершённые переносы в безопасную паузу. При новой sync generation клиенты принимают серверное состояние заново, сохраняя локально скачанное; восстановление старой БД не возобновляет прежнее воспроизведение. GC работает только внутри собственного bucket/prefix, учитывает ссылки и active readers.
- [ ] Повторить тесты и полный `cargo test -p dagaz-server`: PASS. Реальное внешнее копирование, обновление и восстановление проверить в R3, не объявлять готовыми по одному unit-тесту.
- [ ] Контрольная точка: `git add apps/server docs/backup.md`; `git commit -m "feat: recover server state and export backups"`.
