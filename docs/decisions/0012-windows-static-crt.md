# 0012. Windows: статический CRT и короткий путь к проекту

**Дата:** 2026-10-27 · **Статус:** принято · **Код:** `.cargo/config.toml`

## Контекст
Пробная сборка LiveKit Rust SDK 0.9.7 на Windows (MSVC 14.44) упала дважды:
1. `C1083: Cannot open include file 'api/video/encoded_image.h'`. Файл на месте, но путь
   к нему — 292 символа, а `cl.exe` без `LongPathsEnabled` не открывает пути длиннее 260.
   `webrtc-sys` распаковывает libwebrtc в `target/…/build/scratch-*/out/livekit_webrtc/…`
   — это само по себе около 100 символов.
2. `LNK2005 … already defined in msvcprt.lib`: готовый libwebrtc собран со
   статическим рантаймом (`/MT`, `libcpmt`), а Rust по умолчанию линкуется с
   динамическим (`/MD`).

## Решение
- В `.cargo/config.toml` для `*-pc-windows-msvc`: `-C target-feature=+crt-static`
  (так же настроен репозиторий LiveKit). `cc` подхватывает флаг и собирает C++-мост
  `webrtc-sys` тоже с `/MT`.
- Проект держим по короткому пути (как сейчас: `C:\Users\<you>\Desktop\ourcamp`).
  Если сборка снова упадёт с `C1083`, включить длинные пути в Windows
  (`LongPathsEnabled = 1`) — это системная настройка, её меняет сам разработчик.

## Почему
- Другого варианта нет: libwebrtc поставляется только в `/MT`, а пересобирать его
  самим — часы сборки и тулчейн Chromium (`gn`/`ninja`/`depot_tools`).
- Побочный плюс: установщику не нужен Visual C++ Redistributable.

## Последствия
- Все крейты воркспейса на Windows собираются со статическим CRT (Tauri это поддерживает).
- Проверено: пробный бинарник с APM, `NativeAudioSource` и подписью токена
  собирается и запускается (debug, 2 мин на холодную, libwebrtc качается один раз).
