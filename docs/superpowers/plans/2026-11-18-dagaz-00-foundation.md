# Dagaz Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Зафиксировать общий контракт и проверить главные технические риски до разработки полной системы.

**Architecture:** Маленький общий Rust-пакет протокола и независимые одноразовые проверки. Прототипы не становятся автоматически продуктовыми модулями; их результат — доказательство совместимости и выбранные версии.

**Tech Stack:** Rust/serde, GPUI Kit, Rodio/Symphonia, Kotlin/Media3, Lofty, RustFS/S3, FFmpeg.

**Spec:** [Проектирование](../specs/2026-11-18-dagaz-music-player-design.md), [общий контракт и маршрут](2026-11-18-dagaz-music-player-plan.md).

## Global Constraints

- Поддерживаются WAV, MP3, FLAC. Правки метаданных записываются в аудиофайлы на сервере.
- Windows 10/11 x64; Android 9+ arm64 как начальные целевые платформы.
- Техническая проверка не снижает требований к стримингу, перемотке, офлайну и сохранности файлов.
- Все ограничения общего плана применяются и здесь. Вне `experiments/` создаются только контракт и необходимые ему проверки/фикстуры.

## Review Focus

- Сборка успешна, но звук начинает играть только после полного скачивания: P2/P3 проверяют начало при ограниченной скорости.
- Теги записались, но аудиоданные изменились: P4 сравнивает хеш декодированного аудио.
- WAV принимает текстовые теги, но теряет обложку: P4 проверяет оба вида данных отдельно.
- S3 продолжил чтение другой ревизии: P5 проверяет Range/ETag на двух объектах.
- После перекодирования перемотка систематически смещается: P6 сравнивает позиционирование на двух движках.

## P1. Рабочее место и проверяемый сетевой контракт

**Depends:** нет.

**Files:** Create `Cargo.toml`, `rust-toolchain.toml`, `.gitignore`, `crates/protocol/Cargo.toml`, `crates/protocol/src/{lib,auth,library,media,sync,playback}.rs`, `crates/protocol/tests/wire.rs`, `api/openapi.yaml`, `fixtures/contract/{track-null-tags,metadata-clear,playback-duplicates,errors}.json`, `docs/development.md`.

**Interfaces:** Consumes общий контракт. Produces DTO `Track`, `MetadataPatch`, `Quality`, `Playlist`, `MediaDescriptor`, `UploadState`, `Job`, `Change`, `SyncPage`, `PlaybackState`, `PlaybackCommand`, `PlaybackReport`, `PlaybackAck`, `ApiError` и модели доступа из общего плана. Все типы сериализуются serde; SDK/сеть/БД в пакет не входят.

- [x] Добавить минимальный пакет `dagaz-protocol` и тест `wire_preserves_metadata_clear_and_duplicate_queue`, ещё без типов. Зафиксировать примеры всех DTO и ошибок в JSON; тестовые идентификаторы — корректные постоянные UUID.

```rust
let patch: MetadataPatch = serde_json::from_str(r#"{"album":null}"#).unwrap();
assert_eq!(serde_json::to_value(patch).unwrap(), serde_json::json!({"album": null}));
let state: PlaybackState = serde_json::from_str(include_str!("../../../fixtures/contract/playback-duplicates.json")).unwrap();
assert_eq!(state.queue[0], state.queue[1]);
assert_eq!(state.queue_index, 1);
```

- [x] Выполнить `cargo test -p dagaz-protocol --test wire`: ожидается ошибка отсутствующих типов/поведения, а не зависимость от внешнего сервера.
- [x] Описать DTO и OpenAPI, включая nullable-поля, значения enum, пути, ошибки и ключи повторов. Добавить тест round-trip всех примеров и отклонения отрицательной позиции/неизвестного качества на границе запроса. OpenAPI и примеры проверять одним тестом схемы; не поддерживать две несовпадающие версии вручную.
- [x] Выполнить тот же тест, `cargo fmt --all -- --check`, `cargo clippy -p dagaz-protocol --all-targets -- -D warnings`; ожидаются PASS и ноль предупреждений. В `docs/development.md` записать ОС, команды и фактические версии.
- [x] Контрольная точка: `git add Cargo.toml Cargo.lock rust-toolchain.toml .gitignore crates/protocol api fixtures/contract docs/development.md`; `git commit -m "feat: define shared player protocol"`, если запись Git разрешена.

## P2. GPUI и потоковое аудио Windows

**Depends:** P1, P4 (аудиофикстуры).

**Files:** Create `experiments/windows-audio/{Cargo.toml,src/main.rs,src/range_source.rs,tests/network.rs}`, `docs/validation/windows-audio.md`.

**Interfaces:** Consumes три аудиофикстуры и локальный HTTP-сервер с Range. Produces решение о пригодности Rodio/Symphonia, совместимые версии GPUI Kit и звука, результаты кириллического ввода/выбора файлов/системных медиакнопок. `RangeSource` прототипа реализует `Read + Seek`; это не обещание использовать его как общий фреймворк.

- [ ] Собрать отдельный маленький GPUI-проект: таблица, строка поиска, выбор файла, воспроизведение/пауза/перемотка. Не создавать экраны библиотеки. Выбрать стабильный MSVC toolchain, совместимый с GPUI Kit, и записать его в проверку.
- [ ] Добавить самотест `starts_before_eof_and_resumes_same_revision`: ограниченный HTTP-транспорт, разрыв после части файла, продолжение по Range и перемотка. Проверить также VBR MP3 и файл с отсутствующим Content-Length/неподдержанным Range: понятная ошибка вместо зависания.

```rust
assert!(first_audio_at < download_finished_at);
assert_eq!(resumed_revision, initial_revision);
assert!(position_error_ms <= 500);
```

- [ ] Выполнить `cargo test --manifest-path experiments/windows-audio/Cargo.toml --test network` и `cargo run --release --manifest-path experiments/windows-audio/Cargo.toml` на Windows. Ожидаются PASS, слышимый звук трёх форматов, работоспособные ввод кириллицы/диалог файлов/медиакнопки; при разрыве UI остаётся отзывчивым.
- [ ] Записать результаты и ограничения. Если Rodio требует полной загрузки или не обеспечивает нужную перемотку, остановить W2, отдельно выбрать и проверить другой движок до продуктового кода. Плеер, который работает лишь с полностью скачанными файлами, проверку не проходит.
- [ ] Сохранить контрольную точку: `git add experiments/windows-audio docs/validation/windows-audio.md`; `git commit -m "test: validate Windows UI and network audio"`.

## P3. Android: сервис, кеш и восстановление

**Depends:** P1, P4 (аудиофикстуры).

**Files:** Create `experiments/android-media/` — минимальный Gradle-проект с `PlaybackService.kt`, `MainActivity.kt`, `src/androidTest/.../MediaProbeTest.kt`; `docs/validation/android-media.md`.

**Interfaces:** Consumes контракт качества и аудиофикстуры; produces проверенные версии Compose/Media3, minSdk 28 и результаты на физическом arm64-устройстве. Медиа-сессия живёт в `MediaSessionService`, загрузки — в `DownloadService`.

- [ ] Собрать мини-приложение с кнопкой проигрывания, перемоткой и загрузкой одного файла. Установить `minSdk = 28`; target/compile SDK выбрать по актуальной стабильной цепочке инструментов и зафиксировать Gradle wrapper/версии.
- [ ] Добавить `MediaProbeTest.backgroundAndOfflinePlayback`: загрузить короткую фикстуру, закрыть Activity, заблокировать экран, отключить сеть, перезапустить приложение и воспроизвести скачанное. Дождаться состояния player/service через callback с таймаутом, без произвольных больших sleep.

```kotlin
assertTrue(player.isPlaying)
assertEquals(expectedTrackId, player.currentMediaItem?.mediaId)
assertEquals(1, completedDownloadsForTrack)
```

- [ ] Выполнить из проекта `./gradlew testDebugUnitTest assembleDebug connectedDebugAndroidTest`; ожидается PASS на устройстве. Вручную проверить уведомление, гарнитуру, потерю аудиофокуса и отключение наушников. Разрыв сети не должен запускать второй экземпляр сервиса.
- [ ] Отдельно проверить обычное убийство процесса и принудительную остановку приложения: после открытия состояние скачиваний восстанавливается, обещания самопроизвольного пробуждения force-stopped приложения нет.
- [ ] Сохранить проект проверки и результаты: `git add experiments/android-media docs/validation/android-media.md`; `git commit -m "test: validate Android background and offline media"`.

## P4. Чтение и безопасная запись метаданных

**Depends:** P1; FFmpeg доступен локально для генерации и PCM-сравнения, его профильная проверка выполняется в P6.

**Files:** Create `experiments/metadata/{Cargo.toml,src/main.rs,tests/roundtrip.rs}`, `fixtures/audio/README.md`, короткие `fixtures/audio/*.{wav,mp3,flac}`, `fixtures/artwork/cover.png`, `docs/validation/metadata.md`.

**Interfaces:** Consumes Lofty и оригинальные фикстуры. Produces проверенную политику MP3 ID3v2, FLAC Vorbis/PICTURE, WAV RIFF/ID3, результаты сохранения иных тегов/чанков. Проверка не меняет пользовательскую библиотеку.

- [ ] Создать свободные короткие фикстуры с/без тегов, VBR MP3, PCM и float WAV, FLAC, кириллицей, большой обложкой и повреждённым заголовком. Записать происхождение и команды генерации; хранить малый объём файлов.
- [ ] Добавить самотест `tag_roundtrip_preserves_audio_and_unmodified_fields`: копировать файл во временный каталог, изменить альбом/исполнителя/обложку, прочитать снова, сравнить аудио через одинаковый FFmpeg PCM-вывод до/после.

```rust
assert_eq!(after.album, Some("Тестовый альбом".into()));
assert_eq!(after.picture_bytes, expected_picture);
assert_eq!(pcm_sha256_before, pcm_sha256_after);
assert_eq!(after.unmodified_fields, before.unmodified_fields);
```

- [ ] Выполнить `cargo test --manifest-path experiments/metadata/Cargo.toml --test roundtrip`; ожидается PASS для поддержанных разновидностей и явная ошибка повреждённого файла. Проверить WAV-обложку в нашем разборе и отдельно в AIMP; несовместимость стороннего плеера записать, не объявлять файл без обложки поддержанным молча.
- [ ] Записать версии и принятую таблицу разновидностей WAV. Если обязательный формат нельзя безопасно редактировать, остановить S5 и выбрать проверяемое решение; не переходить на хранение правок только в SQLite.
- [ ] Контрольная точка: `git add experiments/metadata fixtures/audio fixtures/artwork docs/validation/metadata.md`; `git commit -m "test: validate lossless metadata updates"`.

## P5. RustFS: загрузки, ревизии и Range

**Depends:** P1.

**Files:** Create `deploy/compose.test.yaml`, `experiments/storage/{Cargo.toml,src/main.rs,tests/s3.rs}`, `docs/validation/storage.md`.

**Interfaces:** Consumes отдельный тестовый RustFS и S3-клиент Rust. Produces проверенную версию образа/SDK, семантику незавершённой загрузки, Range, ETag и очистки. Учетные данные берутся из тестового окружения, в Git их нет.

- [ ] Поднять RustFS с отдельным именем проекта/тома и локальными портами, задать случайные тестовые секреты; добавить самотест `multipart_and_revision_ranges` с файлом `abcdefghij` и второй неизменной ревизией под другим ключом.

```rust
assert_eq!(requested_range, b"cdef");
assert_ne!(revision_one_key, revision_two_key);
assert_eq!(old_revision_bytes, b"abcdefghij");
assert!(!incomplete_upload_is_public);
```

- [ ] Выполнить `docker compose -f deploy/compose.test.yaml up -d --wait`, затем `cargo test --manifest-path experiments/storage/Cargo.toml --test s3`. Ожидается PASS: перезапуск не теряет завершённый объект, Range возвращает требуемые байты, незавершённая передача очищается без удаления готовой ревизии.
- [ ] Проверить сценарий «новый объект сохранён, ссылка в базе не опубликована» и правило очистки: объект с действующей ссылкой не удаляется. ETag RustFS не считать SHA-256 файла; вычислять checksum самостоятельно.
- [ ] Сохранить версии, параметры тестовой сети и измерения: `git add deploy/compose.test.yaml experiments/storage docs/validation/storage.md`; `git commit -m "test: validate RustFS object lifecycle"`.

## P6. Качества и ресурсы перекодирования

**Depends:** P2, P3, P4, P5.

**Files:** Create `experiments/transcode/{Cargo.toml,src/main.rs,tests/profiles.rs}`, `docs/validation/transcode.md`.

**Interfaces:** Consumes аудиофикстуры и выбранный FFmpeg; produces проверенные профили MP3 320/128 кбит/с, сведения CPU/RAM/времени обработки и позиционирование на P2/P3. Параллельных FFmpeg-процессов первоначально один.

- [ ] Создать самотест `profiles_preserve_duration_and_tags`: формировать 320/128 из WAV/FLAC/MP3; сравнить duration/теги/обложку, проверить выход обоими клиентскими движками. Для MP3 ниже целевого битрейта ожидается повторное использование исходника, без сжатия.

```rust
assert_eq!(effective_profile_for_128k_mp3, Quality::Original);
assert!(duration_error_ms <= 500);
assert_eq!(output_artist, "Тестовый исполнитель");
assert!(peak_ffmpeg_processes <= 1);
```

- [ ] Выполнить `cargo test --manifest-path experiments/transcode/Cargo.toml --test profiles`. На тестовом VPS отдельно измерить cold transcode и запись в RustFS; результаты всех трёх форматов сохранить в `docs/validation/transcode.md`, без выдуманных гарантий по тарифу VPS.
- [ ] Записать выбранные версии FFmpeg, GPUI, звука, SDK и Android-зависимостей в отчётах; по результатам обновить проектирование только фактическими уточнениями, сохраняя требования.
- [ ] Контрольная точка: `git add experiments/transcode docs/validation/transcode.md`; `git commit -m "test: validate audio quality profiles"`. Переход к серверу/клиентам допускается после успешных обязательных проверок.

