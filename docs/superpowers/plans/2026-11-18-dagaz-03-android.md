# Dagaz Android Client Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Выпустить Android-клиент с управлением библиотекой, надёжным фоновым воспроизведением и офлайном.

**Architecture:** Compose для интерфейса, один MediaSessionService с ExoPlayer, DownloadService для загрузок, Room для каталога и ссылок на скачанное. HTTP/WebSocket — OkHttp, wire-модели Kotlin проверяются теми же примерами, что Rust. Rust FFI в первой версии отсутствует.

**Tech Stack:** Kotlin, Compose, Media3, Room, OkHttp, kotlinx.serialization, Android Keystore, WorkManager только для ограниченных отложенных задач.

**Spec:** [Проектирование](../specs/2026-11-18-dagaz-music-player-design.md), [общий контракт](2026-11-18-dagaz-music-player-plan.md).

## Global Constraints

- Android 9+ arm64 как начальные целевые платформы; minSdk 28, без самовольного повышения при проблемах зависимостей.
- Поддерживаются WAV, MP3, FLAC. Качество выбирается отдельно для стриминга и скачивания.
- До 120 секунд подкачки вперёд, в пределах 64 МиБ буфера; потоковый кеш 1 ГиБ.
- Загрузка на сервер, изменение тегов и плейлистов требуют подключения; холодный запуск скачанного работает без сервера.
- Для переноса приложение должно быть доступно; force-stopped приложение не обещает принимать удалённый старт. Общий план обязателен.

## Review Focus

- Уход Activity с экрана/повторный запуск не создаёт ещё один ExoPlayer: A2.
- SAF-провайдер отдаёт поток без обычного пути и постоянного доступа: A5.
- DownloadService после рестарта не помечает partial готовым: A3.
- Серверная ревизия/качество/удаление не стирает единственную готовую копию: A3.
- После метро старые сообщения и очередь не перехватывают владение: A4.

Пути Kotlin ниже находятся в `apps/android/app/src/main/java/io/dagaz/player/`; пути тестов указаны полностью относительно репозитория. Команды Gradle выполняются из `apps/android`.

## A1. Приложение, контракт и привязка

**Depends:** P1, P3, S1, W1 (согласованные общие макеты).
**Files:** Create `apps/android/{settings.gradle.kts,build.gradle.kts,gradle/libs.versions.toml,gradlew,gradlew.bat,gradle/wrapper/}`, `app/build.gradle.kts`, `app/src/main/AndroidManifest.xml`, `MainActivity.kt`, `api/{Models,DagazApi}.kt`, `auth/Credentials.kt`, `apps/android/app/src/test/java/io/dagaz/player/ContractTest.kt`, `apps/android/app/src/androidTest/java/io/dagaz/player/PairingTest.kt`.
**Interfaces:** Produces `DagazApi` с `suspend fun pair(code: String, deviceName: String): PairResponse` и методами по P1; `Credentials` сохраняет server URL/device_id и токен, зашифрованный ключом Android Keystore. DTO точно следуют nullable/missing правилам, Quality и ошибкам P1. Общие JSON fixtures подключаются к test resources из корня репозитория.

- [ ] Написать JVM-тест `ContractTest.fixtureRoundTrip` и instrumented `PairingTest.credentialRestore` с настоящим Keystore: все wire-примеры, nullable patch, неверный код, повтор привязки и отозванный токен.

```kotlin
assertEquals(JsonNull, encodedPatch["album"])
assertFalse(encodedPatch.containsKey("artist"))
assertEquals(1, decodedPlayback.queueIndex)
assertEquals(pairedDeviceId, restoredDeviceId)
assertFalse(preferencesText.contains(rawToken))
```

- [ ] Выполнить `./gradlew testDebugUnitTest` и `./gradlew connectedDebugAndroidTest -Pandroid.testInstrumentationRunnerArguments.class=io.dagaz.player.PairingTest`: ожидается FAIL контракта/сохранения credentials; Keystore не проверяется JVM-тестом.
- [ ] Реализовать Gradle-проект с проверенными P3 версиями, сетевой клиент и экраны подключения/ошибок по макету. Запретить cleartext за исключением debug loopback/тестового окружения; токен отправляется только origin своего сервера. Ключ Keystore не копируется в бэкапы приложения; ошибка расшифровки предлагает новую привязку, без бесконечных ретраев.
- [ ] Повторить тест, `./gradlew lintDebug assembleDebug`: PASS. На устройстве выполнить привязку, закрыть и открыть приложение, проверить сохранение подключения и обработку отзыва.
- [ ] Контрольная точка: `git add apps/android`; `git commit -m "feat: add Android app and server pairing"`.

## A2. Библиотека и единственный фоновый плеер

**Depends:** A1, S2–S4, P3.
**Files:** Create `library/{LibraryScreen,LibraryViewModel}.kt`, `playback/{PlaybackService,PlayerConnection,NowPlayingScreen}.kt`, `apps/android/app/src/androidTest/java/io/dagaz/player/PlaybackServiceTest.kt`; Modify manifest/MainActivity/api.
**Interfaces:** `PlaybackService : MediaSessionService` владеет единственным ExoPlayer/MediaSession; UI использует `PlayerConnection`, не создаёт свой плеер. `suspend fun prepare(media: MediaDescriptor, positionMs: Long)` подготавливает без запуска; start/pause/seek/stop доступны координатору A4. Состояние UI — StateFlow; MediaItem.mediaId соответствует track_id, вхождение определяется queue_index.

- [ ] Написать `PlaybackServiceTest.libraryAndBackgroundPlayback`: поиск кириллицы, проигрывание трёх форматов, повтор/перемешивание, блокировка экрана, пересоздание Activity, loss/gain audio focus, отключение наушников, ожидание сети и пользовательская пауза.

```kotlin
assertEquals(1, activePlayerCount)
assertEquals(expectedTrackId, controller.currentMediaItem?.mediaId)
assertFalse(playsAfterHeadphonesUnplugged)
assertFalse(autoResumedAfterUserPause)
assertTrue(firstAudioBeforeFullDownload)
```

- [ ] Выполнить `./gradlew connectedDebugAndroidTest -Pandroid.testInstrumentationRunnerArguments.class=io.dagaz.player.PlaybackServiceTest`: ожидается FAIL.
- [ ] Реализовать порционный каталог/поиск, оригинал/качества, мини-плеер/текущий трек/очередь, уведомление и штатные медиакнопки. Сервис переживает закрытие Activity; корректно освобождает player/session при остановке. Настроить ограничение буфера по времени и байтам, подготовку следующего трека, сетевые повторы по общему расписанию. Недоступность качества показывает подготовку; ошибки доступа/файла не повторяются бесконечно. До A4 управление — промежуточное, не финальная межустройственная координация.
- [ ] Повторить тест, вручную проверить экран блокировки и гарнитуру на реальном телефоне: PASS. Разрыв соединения с сервером не блокирует UI и не создаёт второй сервис.
- [ ] Контрольная точка: `git add apps/android`; `git commit -m "feat: add Android library and background playback"`.

## A3. Офлайн, каталог и shared downloads

**Depends:** A2, S4, S6.
**Files:** Create `downloads/{DagazDownloadService,DownloadRepository,DownloadsScreen}.kt`, `sync/{LocalDatabase,CatalogSync}.kt`, `apps/android/app/src/test/java/io/dagaz/player/DownloadPolicyTest.kt`, `apps/android/app/src/androidTest/java/io/dagaz/player/OfflineTest.kt`; Modify playback/api/manifest.
**Interfaces:** Room хранит catalog/membership, snapshots, local_media, pins, transfer state и cursor/generation. `SelectionKind = track | album | playlist` (wire); `DownloadRepository.enqueue(kind: SelectionKind, id: String, quality: Quality)` создаёт/читает server manifest; `removeSelection(localSelectionId: String)` снимает ссылки; явные refresh и changeQuality следуют правилу W3. Общие JSON-примеры download policy добавляются в `fixtures/contract/download-policy.json` и читаются обоими клиентами.

- [ ] Написать `DownloadPolicyTest.oneTrackOneRepresentation` и `OfflineTest.restartRevisionAndSharedPins`: один трек в двух плейлистах/альбоме, смена качества, удаление одной подборки, partial/restart, ENOSPC, новый ETag, server tombstone, холодный запуск в авиарежиме.

```kotlin
assertEquals(1, readyRepresentationsForTrack)
assertTrue(availableAfterRemovingOneSelection)
assertTrue(oldRepresentationReadableAfterFailedUpdate)
assertTrue(serverDeletedTrackRemainsInDownloads)
assertEquals(1, offlineSearchResultCount)
assertFalse(mixedRevisionPublished)
```

- [ ] Выполнить `./gradlew testDebugUnitTest` и instrumentation OfflineTest: ожидается FAIL.
- [ ] Реализовать постоянный DownloadService/DownloadManager и отдельный постоянный кеш явных загрузок без LRU-вытеснения. Cache key содержит track_id/revision/effective_quality и одинаков во всех подборках; несколько файлов-чанков Media3 считаются одной логической аудиокопией. Потоковый кеш ограничен 1 ГиБ, чтение сначала проверяет готовый явный кеш. После завершения явной загрузки убрать дубли этого ключа из потокового кеша. Проверить конечную длину/SHA-256 чтением кеша без сети; только затем публиковать ready. Старое представление держать до полной проверки нового. Room и DownloadIndex согласовываются после рестарта, pending не считается ready. Синхронизировать метаданные и membership на диск, искать офлайн только доступное, сохранять обложки и server-deleted копии. Обновление состава/файла и понижение качества явные; более высокое существующее качество переиспользуется.
- [ ] Повторить тесты: PASS. На устройстве скачать альбом и плейлист, отключить сеть, обычным образом перезапустить приложение: музыка, поиск и обложки работают; force-stop требует ручного открытия. Возврат сети продолжает unfinished download, не переобъявляя два разных качества готовыми одновременно.
- [ ] Контрольная точка: `git add apps/android fixtures/contract/download-policy.json apps/windows/tests`; `git commit -m "feat: add shared offline downloads on Android"`. Обновить Windows-тест чтением общих policy-примеров, без изменения правил W3.

## A4. Перенос и безопасное переподключение

**Depends:** A3, S7.
**Files:** Create `playback/DevicePlaybackController.kt`, `apps/android/app/src/test/java/io/dagaz/player/DevicePlaybackTest.kt`, `apps/android/app/src/androidTest/java/io/dagaz/player/DeviceHandoffTest.kt`; Modify service/PlayerConnection/CatalogSync.
**Interfaces:** DevicePlaybackController — единственная точка Play/Pause/Seek/Next/Transfer для UI, уведомления и гарнитуры. Consumes P1 PlaybackState/Command/Ack/Report; prepare не запускает звук, stop подтверждается после фактической остановки, start идемпотентен по operation_id.

- [ ] Написать `DevicePlaybackTest.reconcileBeforeProgress` и `DeviceHandoffTest.offlinePhoneYields`: сеть отсутствует при игре телефона; другой клиент получает новую epoch; после reconnect сначала читается состояние, старый прогресс не публикуется. Включить повтор start, stale ack, неподготовленный target и повтор трека в очереди.

```kotlin
assertEquals(0, oldEpochReportsBeforeReconcile)
assertFalse(phonePlayingAfterNewOwnerReceived)
assertEquals(1, startsForSameOperation)
assertEquals(expectedQueueOccurrence, reportedQueueIndex)
assertTrue(positionErrorMs <= 500)
```

- [ ] Выполнить `./gradlew testDebugUnitTest` и instrumentation DeviceHandoffTest: ожидается FAIL.
- [ ] Реализовать WebSocket во время активного использования/сервиса, reconnect state-first, reports раз в секунду и на действиях, очередь/режимы/epoch. При сохранении прежнего владельца и epoch сначала согласовать фактические queue/play_order/режимы/позицию, затем reports; при новой epoch сначала применить серверное решение. Media3 следует материализованному play_order, стандартный независимый shuffle ExoPlayer не пересоздаёт порядок из другого алгоритма. Уведомление/гарнитура отправляют те же команды; offline pause/seek работают локально. Постоянно держать приложение активным при простое не нужно; закрытое force-stop приложение показывается недоступным для переноса.
- [ ] Повторить тесты: PASS; реальное переключение Windows ↔ Android и сетевые гонки проверяются R1, без заявления готовности по одному emulator-тесту.
- [ ] Контрольная точка: `git add apps/android`; `git commit -m "feat: add Android playback handoff and reconnect"`.

## A5. Загрузка файлов, редактор и полное управление

**Depends:** A2, S5. Редактор/мобильная загрузка выполняются до A3/A4 по маршруту и не требуют их реализации.
**Files:** Create `editor/{UploadScreen,MetadataEditorScreen,LibraryEditor}.kt`, `library/PlaylistScreen.kt`, `apps/android/app/src/test/java/io/dagaz/player/LibraryEditorTest.kt`, `apps/android/app/src/androidTest/java/io/dagaz/player/SafUploadTest.kt`; Modify library screens.
**Interfaces:** `suspend fun upload(uri: android.net.Uri, operationId: String): Job` использует ContentResolver; `suspend fun edit(trackId: String, expectedRevision: Long, patch: MetadataPatch): Job`; batch хранит ожидаемые ревизии каждого выбранного трека. Клиент не преобразует content URI в выдуманный файловый путь.

- [ ] Написать `SafUploadTest.multipleUrisResumeWithoutFilesystemPath`, `LibraryEditorTest.batchConflictAndPlaylistOrder`: доступ к URI после перезапуска, неseekable provider, отозванный URI permission, сменившийся исходный файл, конфликт тега, partial batch, неправильная обложка и офлайн.

```kotlin
assertEquals(expectedBytes, serverReceivedBytes)
assertEquals(expectedUniqueTracks, uploadedTrackCount)
assertEquals(1, conflictedEdits)
assertEquals(requestedTrackOrder, playlistAfterReload)
assertFalse(editingEnabledWithoutServer)
```

- [ ] Выполнить unit-тесты и instrumentation SafUploadTest: ожидается FAIL.
- [ ] Реализовать ACTION_OPEN_DOCUMENT с multiple selection и persistable URI permission, если провайдер поддерживает. При отсутствии сохраняемого/seekable источника использовать ограниченную временную копию или повторно открываемый поток с проверкой offset; перед продолжением проверять идентичность содержимого. Показывать понятную необходимость выбрать файл снова при отзыве доступа, без потери остальных загрузок. Добавить одиночный/групповой редактор, выбор JPEG/PNG, playlist CRUD/reorder. Статусы/download refresh/change quality/delete реализуются вместе с A3. Большие выделения делить по серверному максимуму 100. Различать clear/unmodified поля. В офлайне не имитировать сохранение серверных изменений.
- [ ] Повторить тесты, `./gradlew lintDebug assembleDebug`: PASS. На телефоне пройти загрузку нескольких файлов из системного проводника и редактор с TalkBack/крупным шрифтом; сенсорные действия и сообщения соответствуют макету. Финальная оценка — R2.
- [ ] Контрольная точка: `git add apps/android`; `git commit -m "feat: complete Android library management"`.
