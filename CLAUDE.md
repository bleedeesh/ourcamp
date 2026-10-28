# OurCamp — контекст для Claude Code

Пет-проект Димы (не рабочий). Компактный голосовой клиент в духе TeamSpeak.
Отвечать по-русски. Подробности — в `docs/` (architecture, roadmap, decisions/ — ADR).

## Продукт
- Лагерь (= сервер) живёт **у пользователя**: он скачивает и запускает OurCamp Server,
  друзья заходят по ссылке `ourcamp://…`. Админ — тот, кто погасил **одноразовый ключ
  админа** (сервер показывает две ссылки: «для вас» и «для друзей»). ADR-0003, 0005.
- **Общий accounts-сервис**: регистрация/вход, друзья, личка, статусы, список лагерей.
- Главное окно 380×720: дерево голосовых каналов («костров») с участниками. Чат, стрим,
  друзья, настройки — **отдельные окна**. Минимум лишнего.
- Локальная громкость каждого участника 0–200 % и отдельно громкость звука стрима.
- Макеты (13 экранов, кликабельный прототип): https://claude.ai/artifact/A9mZCWav7VeKur4eVfdwgA
  — страница «Компактный клиент». Тема: нейтральный чёрный/тёмно-серый **без синевы**,
  оранжевый из логотипа (#FF7A3D) — только выбранное и главные действия. Логотип —
  мультяшный костёр (`apps/desktop/src/ui/Campfire.tsx`). Шрифты Onest / Nunito 800 / Fredoka.
  Скроллбары кастомные (`styles/base.css`).

## Стек (ADR-0002, 0004)
- Клиент: **Tauri 2 + React + TypeScript** (Vite). Звук и видео — **только в Rust**,
  через IPC идут лишь команды и уровни (~20 Гц).
- Медиа: LiveKit Rust SDK (libwebrtc), `cpal`, свой микшер `crates/oc-audio`;
  стримы — `scap` + H.264 аппаратно + окно на `wgpu`.
- Сервер лагеря: Rust (axum + SQLite) + дочерний livekit-server. Accounts: Node + TS + PostgreSQL.
- Звук: Opus 48 кГц моно 64 кбит/с VBR, FEC + RED, DTX off; AEC3 → шумодав
  (DeepFilterNet/RNNoise) → AGC. ADR-0008.

## Состояние: M0, M1, M2 готовы (M2 проверен Димой на слух; CI зелёный на Windows, macOS, Linux)
- `crates/oc-audio` — lock-free микшер (громкость, mute, deafen, лимитер, детектор речи),
  отвод выхода для AEC (`OutputTap`), `pcm` (f32↔i16), 53 теста.
- `crates/oc-invite` — формат ссылок + ключ админа, пиннинг сертификата, 16 тестов.
- `crates/oc-tls` — проверка сертификата по отпечатку + подписи рукопожатия (ADR-0014), 4 теста.
- `crates/oc-proto` — общий язык API: JSON-типы, коды ошибок, подписываемое сообщение входа, `userId`.
- `crates/oc-camp-client` — клиент лагеря без UI: ключ устройства (Ed25519 на `ring`), HTTPS с
  пиннингом через `oc-tls` (hyper), вход, токены комнат, `transport` — закреплённый
  `WsClient`/`HttpClient` для LiveKit SDK (ставится в `lib.rs` приложения). Им ходят тесты сервера.
- `crates/oc-server` — M3 шаги 1–5 (прокси `/rtc`, ADR-0019; опрос LiveKit и `/api/v1/events`, ADR-0021): headless-сервер (`pnpm server`), SQLite (v2: участники, роли,
  сессии), TLS 7780, ссылки, вход по подписи, ключ админа, токены LiveKit (ADR-0017, 0018).
  `livekit/` — свой LiveKit (`managed` по умолчанию, ADR-0022): ключ в `livekit-keys.json`, конфиг через
  окружение, перезапуск, job object (Windows) / сторож на `sh` (unix). Бинарник — `pnpm livekit:fetch`
  (`dev/bin`, хеши в `dev/fetch-livekit.ts`); `tests/managed.rs` — `#[ignore]`, нужен бинарник.
  `pnpm server:docker` — с LiveKit из Docker (порты 7881/7882 у них общие: одновременно один).
- `apps/desktop/src-tauri`: `audio/` — устройства (`output.rs`, `input.rs`), `Engine`
  (режимы livekit / `OC_VOICE=synthetic`, громкости в `prefs`); `voice/` — `mic.rs`
  (поток микрофона: APM AEC3+NS+AGC → `NativeAudioSource`), `room.rs` (LiveKit, ADR-0008).
  `camps.rs` — лагеря устройства (`device.key`, `camps.json`, ADR-0020): вход по ссылке,
  токен комнаты от лагеря, повторный вход при истёкшей сессии. Dev-токенов нет; второй
  клиент на ПК — `OC_PROFILE=<имя>` (debug). Событие `voice://participants` (ADR-0011).
  Одна копия на профиль (`instance.lock`); разрыв комнаты сервером → `voice://disconnected`.
  Присутствие и роли: `Camps::follow` → `/api/v1/events` → `camp://state` (ADR-0021).
  Синхронные вызовы LiveKit SDK — только внутри `runtime.enter()` (команды Tauri без
  `async` идут на UI-потоке без tokio). Тесты с сервером — `#[ignore]`, нужен `pnpm livekit`.
- `apps/desktop/src` — `CampsController` (`domain/camps.ts`: пустое состояние, вход по ссылке,
  переключатель, «Скопировать приглашение»), `CampApi` (Tauri / демо для браузера), состав
  костра из движка (`domain/roster.ts`), 39 тестов. Вёрстку смотреть в браузере: `.claude/launch.json`
  (`desktop-ui`, Vite + демо-режим, окно 380×752).
- Сборка на Windows: статический CRT (`.cargo/config.toml`), короткий путь (ADR-0012).
- Node в среде агента — через nvs: `%LOCALAPPDATA%\nvs\node\24.21.0\x64` (не в PATH).
- CI зелёный (8 задач, вкл. Miri, тесты с LiveKit в Docker и `managed` на Ubuntu + Windows). Linux-сборке нужен clang ≥ 21.
  Remote: github.com/bleedeesh/ourcamp (публичный). Известные пробелы M2: нет статуса «подключаемся /
  переподключаемся» в UI; смена устройства — только перезапуском (M7).
- CI: `.github/workflows/ci.yml`, задача `voice` поднимает `dev/compose.yaml` и гоняет
  `#[ignore]`-тесты с настоящим LiveKit. Логи задач Actions без авторизации не читаются — просить Диму.

## Дальше
M3 — по шагам из `docs/roadmap.md` (ADR-0014–0022). Готовы шаги 1–5 (лагерь в приложении, присутствие
во всех кострах, свой LiveKit); дальше шаг 6 — окно сервера (Tauri: статус, ссылки, трей).
Контракт UI↔движок (`src/engine/engine.ts`, `src-tauri/src/commands.rs`) не менять без
необходимости. Roadmap: `docs/roadmap.md` (M0–M8).

## Как работать с Димой
- Фронтендер (Vue 3/Nuxt/TS, теперь React); Rust осваивает — объяснять неочевидное в Rust.
- Хочет идеальное качество и производительность, без костылей, с нормальными тестами.
- Предпочитает писать своё, а не тянуть пакет на каждую мелочь (ADR-0007).
- Каждое неочевидное решение — новая ADR в `docs/decisions/` (старые не переписывать,
  заменять новой записью). Заглушки помечать этапом roadmap («arrives in M2»).

## Команды
`pnpm livekit:fetch` (бинарник LiveKit) · `pnpm livekit` (LiveKit в Docker) · `pnpm server` (лагерь со своим LiveKit) · `pnpm dev` · `pnpm test` · `pnpm typecheck` · `pnpm lint:rust` (clippy pedantic, -D warnings + fmt).
Тесты TS — `node --test` (Node ≥ 22.18, только «стираемый» TS, импорты с `.ts`).
