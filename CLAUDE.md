# OurCamp — контекст для Claude Code

Пет-проект Димы (не рабочий). Компактный голосовой клиент в духе TeamSpeak.
Отвечать по-русски. Подробности — в `docs/` (architecture, roadmap, decisions/ — ADR).

## Продукт
- Лагерь (= сервер) живёт **у пользователя**: он скачивает и запускает OurCamp Server,
  друзья заходят по ссылке `ourcamp://…`. Роли (админ, модератор) назначает **хозяин в окне
  сервера**; ключа админа и ссылки «для вас» больше нет. ADR-0003, 0005, 0026.
- **Без центрального сервиса** (ADR-0035): аккаунтов, регистрации, друзей и лички нет. Человек — ключ
  устройства; профиль (ник, аватарка) на устройстве, копии аватарок — на серверах. Ссылки — IP или
  домен (ADR-0036), у домашних серверов «адрес для друзей» (LAN / VPN вроде Radmin / внешний / свой).
- Главное окно 380×720: дерево голосовых каналов («костров») с участниками. Чат, стрим,
  друзья, настройки — **отдельные окна**. Минимум лишнего.
- Локальная громкость каждого участника 0–200 % и отдельно громкость звука стрима.
- Макеты (13 экранов, кликабельный прототип): https://claude.ai/artifact/A9mZCWav7VeKur4eVfdwgA
  — страница «Компактный клиент». Тема: нейтральный чёрный/тёмно-серый **без синевы**,
  фиолетовый «Ночь» (#8B7CF6, ADR-0024) — только выбранное и главные действия. Логотип —
  мультяшный костёр в той же палитре (`packages/ui/src/Campfire.tsx`, иконки — `apps/desktop/app-icon.svg`, у окна сервера своя со значком «сервер» — `apps/server/app-icon.svg`). Шрифты Onest / Nunito 800 / Fredoka.
  Скроллбары кастомные (`styles/base.css`). Иконки — Lucide в сплошном стиле (`packages/ui/src/Icon.tsx`, ADR-0027).
- **Слова в интерфейсе: сервер, канал, чат** (ADR-0027); «лагерь» и «костёр» — только имена в коде
  (`camp`, `campfire`, протокол, база), их не переименовываем.

## Стек (ADR-0002, 0004)
- Клиент: **Tauri 2 + React + TypeScript** (Vite). Звук и видео — **только в Rust**,
  через IPC идут лишь команды и уровни (~20 Гц).
- Медиа: LiveKit Rust SDK (libwebrtc), `cpal`, свой микшер `crates/oc-audio`;
  стримы — захват SDK + H.264 через Media Foundation + окно на `wgpu` (ADR-0049).
- Сервер лагеря: Rust (axum + SQLite) + дочерний livekit-server. Своих облачных сервисов нет (ADR-0035).
- Звук: Opus 48 кГц моно 64 кбит/с VBR, FEC + RED, DTX off; AEC3 → шумодав
  (DeepFilterNet/RNNoise) → AGC. ADR-0008.

## Состояние: M0, M1, M2 готовы (M2 проверен Димой на слух; CI зелёный на Windows, macOS, Linux)
- `crates/oc-audio` — lock-free микшер (громкость, mute, deafen, лимитер, детектор речи),
  отвод выхода для AEC (`OutputTap`), `pcm` (f32↔i16), 53 теста.
- `crates/oc-invite` — формат ссылок (вид `claim` — только чтобы узнать старый ключ админа), пиннинг сертификата, 16 тестов.
- `crates/oc-tls` — проверка сертификата по отпечатку + подписи рукопожатия (ADR-0014), 4 теста.
- `crates/oc-proto` — общий язык API: JSON-типы, коды ошибок, подписываемое сообщение входа, `userId`.
- `crates/oc-camp-client` — клиент лагеря без UI: ключ устройства (Ed25519 на `ring`), HTTPS с
  пиннингом через `oc-tls` (hyper), вход, токены комнат, `transport` — закреплённый
  `WsClient`/`HttpClient` для LiveKit SDK (ставится в `lib.rs` приложения). Им ходят тесты сервера.
- `crates/oc-server` — M3 шаги 1–5 (прокси `/rtc`, ADR-0019; опрос LiveKit и `/api/v1/events`, ADR-0021): headless-сервер (`pnpm server`), SQLite (v8: участники, роли, костёр AFK — ADR-0025, запрет говорить — ADR-0028, аватарки — ADR-0035, чаты и фото — ADR-0044, удаление сообщений — ADR-0048,
  сессии), TLS 7780, ссылка для друзей, вход по подписи, роли и каналы v4 (ADR-0026), токены LiveKit (ADR-0017, 0018),
  выход из лагеря `DELETE /api/v1/members/me`.
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
  `async` идут на UI-потоке без tokio). Тесты с сервером — `#[ignore]`, нужен `pnpm livekit`; тесты приложения с ним — только `--test-threads=1`, у них одна комната (ADR-0053).
- `apps/server` — окно сервера лагеря (Tauri, ADR-0023, `pnpm server:window`): `oc-server` в процессе,
  снимок `ServerStatus` (`src-tauri/src/status.rs`) → событие `server://status`; трей, крестик прячет окно.
  UI — `src/ui/App.tsx`, тексты в `src/status.ts` (тесты). Вёрстка в браузере — `server-ui` (порт 5175).
- `packages/ui` — общее для двух окон: токены темы, `base.css`, `Campfire.tsx`, `Icon.tsx` (Lucide), `clipboard.ts`, `useDragMove.tsx` (перетаскивание людей по каналам, ADR-0029), `Select.tsx` (свой список вместо `<select>`, ADR-0038),
  `tauri-events.ts` (исходниками, компилирует Vite приложения).
- `apps/desktop/src` — `CampsController` (`domain/camps.ts`: пустое состояние, вход по ссылке,
  переключатель, «Скопировать приглашение»), `CampApi` (Tauri / демо для браузера), состав
  костра из движка (`domain/roster.ts`), 39 тестов. Вёрстку смотреть в браузере: `.claude/launch.json`
  (`desktop-ui`, Vite + демо-режим, окно 380×752).
- Сборка на Windows: статический CRT (`.cargo/config.toml`), короткий путь (ADR-0012); там же `_HAS_EXCEPTIONS=1` для C++ `webrtc-sys`, иначе ошибки WebRTC читаются из свободной памяти (ADR-0050). На macOS там же `-ObjC` (ADR-0052).
- `vendor/livekit-0.9.7` — копия LiveKit SDK через `[patch.crates-io]`: признак стерео для звука стрима (ADR-0055) потолок соединения, запускающий пробу канала для стрима (ADR-0062), и проба по запросу (ADR-0065), убрать, когда всё это выйдет в релизе.
- Node в среде агента — через nvs: `%LOCALAPPDATA%\nvs\node\24.21.0\x64` (не в PATH).
- CI зелёный (8 задач, вкл. Miri, тесты с LiveKit в Docker и `managed` на Ubuntu + Windows). Linux-сборке нужен clang ≥ 21.
  Remote: github.com/bleedeesh/ourcamp (публичный). Известный пробел M2: нет статуса «подключаемся /
  переподключаемся» в UI.
- CI: `.github/workflows/ci.yml`, задача `voice` поднимает `dev/compose.yaml` и гоняет
  `#[ignore]`-тесты с настоящим LiveKit. Логи задач Actions без авторизации не читаются — просить Диму.

## Дальше
M3: готовы шаги 1–6 (ADR-0014–0025). Порядок дальше — раздел «После проверки M3 Димой» в
`docs/roadmap.md`: мелочи ✅, управление лагерем в окне ✅ (роли владелец/админ/модератор, название, каналы
Voice/Chat/AFK, `manage.rs`, ADR-0026); модерация из окна ✅ (выгнать, перенести, запретить говорить —
адресные события `moved`/`kicked`, `silenced` в базе v5 и права LiveKit, ADR-0028); перенос перетаскиванием в окне и клиенте ✅ (`useDragMove`, `POST /members/{id}/move`, ADR-0029); пинг и качество
связи в панели (ADR-0030), клиент в трее (ADR-0031), своя иконка окна сервера (ADR-0032); модерация из клиента ✅ (меню участника,
выгнать/запретить — только младших по рангу, `Role::governs`, ADR-0033); правый клик — свои меню, меню браузера выключено (ADR-0034); дальше M4 по ADR-0035/0036 (раздел «Без центрального
сервиса» в roadmap): ник и аватарка ✅ (`avatar` на устройстве, `avatars/` — кэш по хешу, `makeAvatar` — canvas), «адрес для друзей» и VPN ✅ (`network.rs` — сети по маршруту, `window.json`, `--host radmin`, ADR-0037), шаг 7 ✅ (свой UPnP `upnp.rs`, `ports.rs`, «Интернет через роутер», «серый» CGNAT, ADR-0039), открытые серверы ✅ (`Link::Open`, `--open`, ADR-0040); дальше домен с настоящим сертификатом (ссылка без отпечатка), сборка headless под Linux, команды владельца для headless ✅ (`control.rs` — loopback + `control.json`, `owner.rs`, ADR-0041); домен с сертификатом от центра ✅ (ссылка без `fp`, `oc_tls::Trust`, `certificate.rs` — `--tls-cert`/`--tls-key`, перечитывание, SNI; вход подписан на `домен:порт`, ADR-0042); сервер для Linux ✅ (`packaging/linux/` — systemd, `install.sh`, `sudo ourcamp …`; `dev/pack-server.ts` — статический musl + LiveKit; CI `server-linux` ставит и проверяет, ADR-0043). M5 (чат, ADR-0044, «M5 по шагам» в roadmap): шаг 1 ✅ сервер — `store/chat.rs` (база v7), `api/chat.rs`, событие `message` всем (`Directive.to`), `chats` в снимке; шаг 2 ✅ клиент без окна — `camps/chat.rs` (команды `chat_*`, кэш `photos/`, `read` в `camps.json`, событие `camp://chat`), `domain/chat.ts`, счётчик на «Чат» (ADR-0045); шаг 3 ✅ окно чата — `chat.html` + `chat-main.tsx`, `ChatController` (`domain/chat-window.ts`), `domain/chat-feed.ts`, `ui/chat/`, `windows.rs` (окно рядом с главным, крестик закрывает), `chat_context` (ADR-0046); шаг 4 ✅ модерация чата — удаление (база v8, `message_deleted`), лимит частоты (ADR-0048). M5 готов. M6 (стримы, ADR-0049, «M6 по шагам» в roadmap): захват SDK, H.264 через Media Foundation (`new_encoded`), без `scap` и simulcast; шаг 1 ✅ стрим без окон — `src-tauri/src/stream/` (`mf.rs` — кодировщик, отложенный `mfplat.dll`; `pipeline.rs`), команды `stream_*`; шаг 2 ✅ подписка по запросу и LIVE — `auto_subscribe` выключен, микрофоны просит `voice/room.rs`, `streaming` в снимке (`PresentMember`), значок из `applyCampState`; шаг 3 ✅ окно просмотра — `stream/view/` (дочернее окно с `wgpu` поверх страницы `stream.html`, `stream_watch`, `stream_place`, атрибут `oc.watching`, ADR-0053); шаг 2 ✅ подписка по запросу и LIVE; шаг 3 ✅ окно просмотра (`stream/view/`, ADR-0053); шаг 4 ✅ окно «Начать стрим» (`stream-start.html`, `domain/stream-start.ts`), 1440p, событие `voice://stream`, конец стрима при потере источника (ADR-0054); шаг 5 ✅ звук стрима — `stream/sound.rs` (process loopback, захват в формате устройства и своё сведение в стерео), `SourceKey::Stream`, `stream_volume` (ADR-0055); захват в своём потоке `oc-stream-capture` (ADR-0058), окно напрямую через Windows.Graphics.Capture — `stream/wgc.rs` (ADR-0060), окон OurCamp в списке источников нет (ADR-0059), профиль High (ADR-0061), стрим сразу выходит на свой битрейт (ADR-0062), потолки битрейта (ADR-0063), громкость стрима до 200 % и мини-панели во весь экран (ADR-0064), лёгкий поток для слабого канала (`screen-light`, `oc.light`, `stream/view/ladder.rs`, видео стрима идёт как `CAMERA`, `allow_pause` у LiveKit, ADR-0065); проба канала иногда не срабатывает, и `a_stream_gets_its_bitrate_at_once` падает сериями: причина не найдена (ADR-0067); выключать лёгкий поток без просьб не нужно, его доля живёт ~3,5 с (ADR-0068); шаг 6 (macOS и Linux) отложен: проверить вживую не на чем, стримы пока только на Windows (ADR-0056). M7 (настройки, ADR-0057, «M7 по шагам» в roadmap): шаг 1 ✅ окно настроек — `settings.html`, `ui/settings/`, смена имени (`set_name`, `name_sent`, `PUT /members/me/name`), событие `profile://changed`; шаг 2 ✅ устройства — `audio/devices.rs` (`sound.json`, выбор по имени, лёгкий взгляд раз в 3 с), новый микшер при смене вывода и новая дорожка при смене микрофона (`replace_output`, `replace_mic`), вкладка «Звук», ADR-0066; дальше шаг 3 — как включается микрофон.
Контракт UI↔движок (`src/engine/engine.ts`, `src-tauri/src/commands.rs`) не менять без
необходимости. Roadmap: `docs/roadmap.md` (M0–M8).

## Проверка
Демо в браузере не проходит через Tauri. Окна, команды и события проверять в настоящем приложении
через отладочный порт WebView2 (ADR-0047). Окна Tauri создавать только из `async`-команд: из синхронной
на Windows всё приложение зависает. Vite на 5173/5175 мешает `pnpm dev` Димы — держать только на время проверки.

## Как работать с Димой
- Фронтендер (Vue 3/Nuxt/TS, теперь React); Rust осваивает — объяснять неочевидное в Rust.
- Хочет идеальное качество и производительность, без костылей, с нормальными тестами.
- Предпочитает писать своё, а не тянуть пакет на каждую мелочь (ADR-0007).
- Каждое неочевидное решение — новая ADR в `docs/decisions/` (старые не переписывать,
  заменять новой записью). Заглушки помечать этапом roadmap («arrives in M2»).

## Команды
`pnpm livekit:fetch` (бинарник LiveKit) · `pnpm livekit` (LiveKit в Docker) · `pnpm server` (лагерь со своим LiveKit) · `pnpm dev` · `pnpm test` · `pnpm typecheck` · `pnpm lint:rust` (clippy pedantic, -D warnings + fmt).
Тесты TS — `node --test` (Node ≥ 22.18, только «стираемый» TS, импорты с `.ts`).
