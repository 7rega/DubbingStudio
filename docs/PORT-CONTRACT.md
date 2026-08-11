# Dub Studio — карта REST/SSE контракта (порт Python → Rust)

Источник истины: `backend/app.py` (FastAPI, один GPU-воркер + asyncio-очередь). Rust-сервер
(`crates/dub-server`, axum) обязан отдавать **тот же** контракт, чтобы SPA (`frontend/dist`) работал
без правок. Порт (8765) и формат ответов совпадают.

Легенда статуса: **done** — реализовано в раунде 1; **todo** — каркас/следующие раунды (нужен движок).

## Инфраструктура запросов

- Один GPU-воркер, одна очередь джоб. Тяжёлые (GPU) операции ставятся в очередь и возвращают
  `{"job_id": "..."}`; прогресс — по SSE `GET /jobs/{id}/events`. Синхронные (не-GPU) правки Project
  отвечают сразу телом Project.
- Снапшот `OPTS` берётся в момент постановки джобы (иммунитет к конкурентному `PATCH /engine/opts`).
- В Rust: `crates/dub-server/src/jobs.rs` — единый воркер (`spawn_blocking`), broadcast-канал на SSE,
  oneshot для синхронного ожидания (preview/original). Реап терминальной джобы через 300с.

## SSE — формат событий (`GET /jobs/{id}/events`)  — **done**

```
data: {"type":"progress", "msg": "...", ...}
data: {"type":"done", "result": <json|null>}
data: {"type":"error", "error": "..."}
```
Поток завершается на `done`/`error`. 404 если job_id неизвестен. Совпадает 1:1 с app.py.

## Эндпоинты

| Метод | Путь | Статус | Назначение |
|-------|------|--------|-----------|
| GET | `/engine/capabilities` | **done** | JSON: device, tts_quant, asr_model, models{asr,llm,vision,tts}, ffmpeg(bool), languages[], voice_modes[] |
| PATCH | `/engine/opts` | **done** | Валидирует непустые строки (иначе 400), возвращает {models:{...}}. Рантайм-свап слота вне scope: OPTS иммутабелен (Arc<EngineOpts>), стек задаётся при старте — форма ответа соблюдена (endpoints.rs::set_opts) |
| — | | | **Раунд 2:** analyze покрывает ТОЛЬКО транскрипт-стадию (ASR+диаризация); перевод/vision/OCR/captions — раунд 3 |
| GET | `/fonts` | **done** | {fonts: family→описание} из captions.FONTS (порядок питона, endpoints.rs::fonts) |
| GET | `/voices` | **done** | {voices: []} — у порта нет именованного voice-пака (Higgs клонирует из ref-аудио), отдаём пусто как питон при отсутствии пака (endpoints.rs::voices) |
| GET | `/presets` | **done** | {presets: TEMPLATES{reveal,plate,font,base,plate_c?,accent?}, reveals: [...]} (fresh-семейство без plate_c, endpoints.rs::presets) |
| POST | `/projects` | **done** | multipart-загрузка видео → `workspace/<pid>/source.<ext>` + `source.txt`; вернуть {project_id, filename} |
| POST | `/projects/{pid}/analyze` | **part** | Query: tgt_lang, mode, src_lang, subs, rewrite. Джоба: analyze()→project.json. Вернуть {job_id}. **Раунд 2: ASR+диаризация (транскрипт-стадия): ffmpeg extract 16k mono → Sortformer turns (при <2 спикеров штатная single-speaker ветка) → TDT int8 словные таймстемпы → Project (src_text, words в extra, speaker, mode/subs-дефолты). Раунд 3: стадии translate+vision через сайдкар Gemma (llama-server + mmproj) — ctx-проход vision layout/scene + audio-контекст + перевод всего транскрипта → tgt_text, captions.titles(+tgt)/sub_style/sub_y/brands, raw_ctx. Fail-safe: сбой перевода не валит транскрипт-стадию. OCR/captions/render — раунд 4.** SSE-фазы: probe/extract_audio/diarize/asr/vision/translate |
| POST | `/projects/{pid}/remix` | **done** | Query: instruction. Джоба: Gemma (flat_rewrite dub-translate) переписывает весь транскрипт, все dirty, audio.rewrite=instr. Вернуть {job_id} (endpoints.rs::remix_project) |
| GET | `/projects/{pid}` | **done** | Тело Project (JSON) или 404/409 |
| PATCH | `/projects/{pid}` | **part** | Синхронная правка Project (без GPU), `{op: ...}` — см. таблицу ниже. **Раунд 2: segment/subpos/mode (атомарно tmp+rename). Раунд 3: translate (смена tgt_lang + subs=translate, funny→rewrite, все dirty), rewrite (инструкция ре-дубляжа, mode=dub, все dirty). Прочие op → 400.** |
| PUT | `/projects/{pid}` | **done** | Полная замена Project (undo/redo снапшот); валидирует тело как Project, сохраняет атомарно (endpoints.rs::put_project) |
| GET | `/projects/{pid}/waveform?n=600` | **done** | Пики аудио (ffmpeg → s16le 8kHz, np.array_split-эквивалент), кэш waveform.json, CPU вне GPU-воркера (endpoints.rs::waveform + wavio::waveform_peaks) |
| GET | `/projects/{pid}/preview?t=&rev=` | **done** | Джоба preview_frame → PNG через GPU-воркер (build_ass из ТЕКУЩЕГО Project + burn_frame). Синхронное ожидание (timeout 300с), abandoned при таймауте (endpoints.rs::preview + frame.rs::preview_frame) |
| POST | `/projects/{pid}/render` | **done** | Джоба render()→output.mp4 (раунд 4): probe→extract 44.1k→separate(dub-sep)→Higgs clone TTS per-seg (кэш seg_XXX.wav, ре-TTS ТОЛЬКО dirty)→fit_to_slot(atempo)→timeline→mix(instr+dub)→build ASS(dub-captions)→burn(blur из blur_boxes)→mux. regen_dub если dirty; после — сбросить dirty. SSE-фазы probe/extract_audio/separate/tts/mix/build/burn/mux. Вернуть {job_id} |
| GET | `/projects/{pid}/output?dl=` | **done** | Отдать output.mp4 (Range через tower-http ServeFile → 206); dl=1 → Content-Disposition attachment |
| GET | `/projects/{pid}/original?t=` | **done** | ОДИН PNG-кадр оригинала на t (порт app.py.original → source_frame; ComparePane вставляет как `<img src>`). Джоба source_frame → PNG, timeout 60с. **ИСПРАВЛЕНО (раунд 5): раньше отдавал Range-видео — это ломало ComparePane (broken img). Сырое видео для плеера — /dub.** (endpoints.rs::original_frame) |
| GET | `/projects/{pid}/dub` | **done** | Проигрываемое видео: output.mp4, иначе analyzed.mp4 (Range) |
| — (fallback) | `/{spa_path}` | **done** | SPA: реальный статик-файл (с защитой от path-traversal), иначе index.html |

## PATCH `/projects/{pid}` — операции `op` (все синхронные, без GPU) — **todo**

Правки применяются к Project, затем `project.json` перезаписывается и возвращается Project. Ошибки:
неизвестная/битая правка → 400; неизвестный seg/idx → 404.

| op | поля | действие |
|----|------|---------|
| `caption` | seg_id, + поля стиля | edit_caption; TypeError/ValueError/KeyError → 400 |
| `segment` ✅ | id, tgt_text?, src_text?, voice?, hidden?, keep_original? | edit_segment; неизвестный id → 404 (раунд 2) |
| `del_segment` | id | удалить строку (уходит и субтитр, и дубляж) |
| `hide_segment` | id, hidden? | тоггл/установка скрытия строки |
| `del_segments` | ids[] | массовое удаление |
| `hide_segments` | ids[], hidden | массовое скрытие (явный флаг) |
| `del_titles` | idxs[] | массовое удаление титров (high→low index) |
| `del_blurs` | idxs[] | массовое удаление blur-боксов (high→low index) |
| `keep_segment` | id, keep? | тоггл «keep original audio» (без дубляжа/перевода) |
| `keep_segments` | ids[], keep | массовый keep-original |
| `blur` | idx, + поля | edit_blur; IndexError/KeyError → 404 |
| `blur_add` | x,y,w,h,t0?,t1? | add_blur |
| `blur_del` | idx | del_blur |
| `blur_enable` | on? | глобальный тоггл блюра (render.blur) |
| `preset` | name? | имя TEMPLATE-пресета (None/"match" = как оригинал); только re-burn |
| `title` | idx, + поля | edit_title |
| `title_del` | idx | del_title |
| `title_add` | text,x,y,w,h,t0?,t1?,italic?,font?,color? | add_title |
| `subpos` ✅ | sub_y | перетащить полосу субтитров; ставит sub_y_locked=true (раунд 2) |
| `mode` ✅ | value | set_mode (subtitles/dub/funny → nodub/dub/dub+rewrite); неизвестное → 400 (раунд 2) |
| `translate` ✅ | lang?, mode? | translate: tgt_lang=lang, subs=translate, funny→rewrite; все dirty (раунд 3) |
| `rewrite` ✅ | instruction | rewrite: audio.rewrite=instruction, mode=dub; пустая→400; все dirty (раунд 3) |
| `recast` | voice_mode?, voice_name? | recast (сменить режим/голос дубляжа) |
| `regen` | id | пометить сегмент dirty → ре-TTS только его на /render |
| `regen_all` | — | пометить все dirty → ре-TTS всего дубляжа |

## Защита от path-traversal (SPA)  — **done**

app.py контейнит отдачу web-root: `f.is_file() and (f == WEB_R or WEB_R in f.parents)` на
канонизированном пути — `..%2f`-сегменты не должны вычитывать произвольные файлы. В Rust
(`spa.rs::serve_spa`) то же: `canonicalize()` запрошенного пути обязан начинаться с канон. web-root,
иначе → index.html. pid дополнительно ограничен `[A-Za-z0-9]` до касания ФС.

## Что реализуют движки-крейты (для analyze/render/preview следующих раундов)

- `crates/dub-asr` — Parakeet-TDT-v3 (словные таймстемпы, `TimestampMode::Words`) + Sortformer-диаризация.
  Порт `dubengine/asr.py` (`_segment`, `transcribe`, `transcribe_turns`) и `diarize.py` (turns/assign).
- `crates/audiocpp` — Higgs Audio v3 TTS/клон голоса (FFI над audiocpp_engine.dll). Порт `tts.py`/`voices.py`.
- `crates/dub-core` — типы Project (serde, extra="allow" round-trip) и EngineOpts. Порт `project.py`/`opts.py`.
- `crates/dub-sep` — вокал/инструментал сепарация. Порт `dubengine/separate.py`, но ДВИЖОК ЗАМЕНЁН
  приказом юзера (2026-07-11): вместо UVR-MDX (audio-separator) — **Mel-Band Roformer voc_fv6-Q8_0**
  через нативный сайдкар **BSRoformer.cpp** (C++/ggml, CUDA). CLI отдаёт ТОЛЬКО вокал-стем (num_stems=1);
  инструментал = `mix − vocals` во временной области (выход выровнен по семплам, реконструкция точная).
  Движок — `tools/bsroformer/` (bs_roformer-cli.exe + 4 ggml-DLL, CUDA), модель — `models/bsroformer/`
  (в .gitignore, копируются с диска, не качаются). Вход движка 44.1кГц.
  **ВРЕЗКА (раунд 4):** analyze audio-context (Gemma «слышит» вокал) сейчас получает vocals-стем
  сепарации, а не полный микс (в раунде 3 подавался `vocals16` из полного микса — теперь перед
  извлечением 16k mono микс прогоняется через dub-sep). Рендер использует instrumental как фон.
- `crates/dub-captions` — CapCut-субтитры через ffmpeg+libass (ASS). Порт `captions.py`: build() (титры
  localized-in-place + дублированные субтитры, 26 пресетов/реверсов) + burn()/burn_frame() (gblur боксов
  + оверлей ASS, NVENC). Метрики глифов — ab_glyph (замена PIL). Шрифты в `fonts/`.
- `crates/dub-ocr` — экранный OCR (PP-OCR DBNet det + CRNN rec + cls, `models/ocr/`) для блюр-боксов
  вшитого текста. Порт `text_detect.py` (detect_regions: семплинг→det+(cls)+rec→merge→IoU-трекинг) +
  `compose.py` (analyze_layout: субтитр-полоса vs титры). Свой ort-пайплайн (rc.12 load-dynamic api-24,
  как dub-asr — один OrtApi; БЕЗ download-binaries, ndarray 0.16 — единый набор фич по воркспейсу).
  Детекция — полный DBPostProcess: connected-components → min-area-rect → box_score_fast → **unclip
  истинным edge-normal offset** (равномерно растит тонкие широкие боксы сабов по высоте; радиальный
  сдвиг от центроида резал глифы по высоте → rec шумел). Словарь rec — из **метадаты ONNX** (ключ
  `character`, как RapidOCR v3; blank префиксуется, токены КАК ЕСТЬ — лишний пробел сдвигал индексы
  CTC). Живой прогон example_original.mp4: rec читает вшитые сабы чисто («КОРОЧЕ ОН» 0.81, «ПОДКЛЮЧЕН»
  0.86–0.90, «У МЕНЯ CLAUDE» 0.90, «сигнал» 0.98, «получается» 0.94 — score>0.8).
  **spoken-гейт analyze_layout — питон-смысл, устойчивый к rec:** геометрия дословно питоновская
  (`nt>=3` distinct-строк в нижней полосе; A-шный доп. ratio `nt>=0.3*len` снят — питон убрал его как
  fps-хрупкий). Питоновский порог `spoken_frac>=0.5` структурно недостижим на СКЛЕЕННЫХ read'ах
  PP-OCR-CRNN (строка выходит без пробелов: «онотправляеТ») — проверено запуском самого питоновского
  `compose.analyze_layout` на наших raw: он тоже отдаёт 0 боксов (frac полосы ~0.11). Поэтому spoken
  служит РАЗЛИЧИТЕЛЕМ сцен-графики (снек-паки/вывески НЕ говорят сказанного → frac==0) от субтитр-полосы
  (говорит → frac>0); измерение усилено фолдингом гомоглифов (латиница↔кириллица, К↔K и т.п. — модель
  их путает) и матчем по подстроке. Полосу отбрасываем лишь если spoken есть И frac==0 — это верный
  питоновский смысл (блюрить только реальные субтитры). Живой analyze: sub_y=640, полоса сабов накрыта
  (fps=2 → 89 caption_boxes; fps=4 → 184), сцен-графика (cy≈464) исключена в localize.
- Рендер-ядро — `crates/dub-server/src/render.rs`: порт render-половины `pipeline.run` + `_build_dub`
  TTS-ветки + `assemble.py` + `compose.py`(mix)/`media.mix`. OCR-стадия analyze — `ocr.rs`.
- Остаётся (раунд 5): preview?t&rev, waveform, undo/Cmd-K, remix, прочие PATCH (caption/blur*/title*/
  preset), портативная упаковка. (Rec-качество OCR доведено: edge-normal unclip + словарь-из-меты —
  вшитые сабы читаются чисто, score>0.8; поглощена ветка r4-ocr.)

## Раунд 5 — caption-композит pipeline.run:388-643 (parity-аудит закрыт)

Аудит (задача #26) нашёл, что гигантская «склейка» pipeline.run (388-643) в порту отсутствовала:
титры не рисовались (bbox=None -> emit_title скипал), утекал EN-оригинал, не было cover-plate,
band-коалесценции, per-segment y-райдинга, auto-nodub гейта. Порт живёт в
`crates/dub-server/src/compose.rs` (модуль `compose`), вызывается из `ocr.rs::stage` ПОСЛЕ
translate-стадии (raw_ctx готов) и OCR-детекции (localize/caption_boxes готовы) — точное место в питоне.

| # | Расхождение | Статус | Где | Доказательство |
|---|-------------|--------|-----|----------------|
| 1 | bbox титров (y_frac -> OCR-box match) | **done** | compose.rs `run` (порт 497-543) | юнит `ctitle_bbox_matches_localize_boxes` + `_fallback_center_band` сверены со standalone-прогоном питон-блока; E2E: `verify_project.json` титр получил bbox (был None) |
| 2 | блюр титров/таглайнов/leftover/group/cap + band-коалесценция | **done** | compose.rs `run` (443-452, 579-589) + ocr.rs `stage` (416-442: `blur::straddles_center`+`blur::band_blur`, раньше мёртвый код) | E2E: title-регион в blur_boxes (EN не ликует) |
| 3 | sub_px (OCR-размер оригинала) | **done** | compose.rs (608-610) -> `raw_plan[sub_px]`; render build берёт | юнит `sub_px_median_on_band` (медиана высот на sub_y±7%vh) |
| 4 | sub_style mirror-from-captions + cap_px | **done** | compose.rs `ensure_sub_style_mirror` (466-484) | код-путь; fires при пустом sub_style + captions из raw_ctx |
| 5 | white-card fallback (scene_color/scene_flat) | **done** | compose.rs `white_card_fallback` (490-493) | юнит `white_card_activates_cover` |
| 6 | auto->nodub гейт (_has_speech) | **done** | analyze.rs `has_speech` + флип mode (66-78,124-129) | uniq>=0.35, coverage>=0.10; пустой транскрипт -> nodub |
| 7 | next.start слот по индексу i+1 полного списка | **done** | render.rs `build_dub` (питон 207) | seg-файл и слот по fi полного списка, не по времени |
| — | per-segment y-райдинг субтитр-полосы | **done (сосед 61b6158)** | render.rs `seg_y` по band-боксам blur_boxes | эквивалентный источник (band = покадровые caption_boxes) |

Таглайн-MT (перевод оставшихся надписей титр-карты, питон 564-578) в композите опционален и
**fail-safe**: поднимает text-only Gemma лишь при непустых tcard_rows; без весов/сбоя — регион всё
равно блюрится (EN не ликует), просто без перевода. Не-ctx ветка (ctx off / нет ctx_extra) не
портирует plain-MT title-fallback (loc_blocks там пусты) — на живом стеке ctx_translate ВКЛ, это
мёртвый путь; блюр (таглайны+group_blur+band) оригинал накрывает. Осознанное упрощение.

E2E-харнесс: `verify_captions_e2e` (пример `cargo run -p dub-server --example verify_captions`) —
берёт кэш project.json (transcript+raw_ctx), заново гонит OCR+compose+build_ass+burn БЕЗ ASR/Gemma/TTS,
даёт captioned.mp4 для покадрового сравнения порт-vs-эталон (docs/example_dub.mp4).

## Раунд 5 — ПОДЛОЖКА САБОВ ДЕФОЛТОМ (продуктовое отклонение, приказ юзера 2026-07-12)

**ОСОЗНАННОЕ ОТКЛОНЕНИЕ ОТ ТЕКУЩЕГО ПИТОНА** — прямой приказ юзера (2026-07-12). Продукт по
умолчанию рисует дублированные субтитры **на СПЛОШНОЙ ПЛАШКЕ** (визуально как эталонная версия
продукта `docs/example_dub.mp4` — текст-хаг плита за строками), а не питоновским тонким outline.

- **Где:** `crates/dub-captions/src/lib.rs::build_s_style`. Новый флаг `SubStyle.plate: Option<bool>`
  (+ `plate_color`), приходит из `extra` через `render.rs::map_sub_style` (PATCH `caption` его кладёт).
- **Поведение:** при `bg`=none/пусто И БЕЗ явной полосы vision И `plate`≠Some(false) → светлый текст
  (lum>0.45) рисуется веткой A (`BorderStyle=3, Outline=11`) на плашке `plate_color` (**дефолт чёрный
  `#000000`**, как эталон). Это ровно та ветка, которой отрисован `example_dub.mp4`.
- **Приоритеты:** явная полоса vision (bg solid, контраст ≥0.20) — как ветка A и раньше, ГЛАВНЕЕ
  продуктовой плашки. `outline_w`-оверрайд редактора — тоже главнее. Тёмный текст (lum≤0.45) сохраняет
  питоновскую белую плиту (тоже сплошную, читаемую). `plate=Some(false)` (тумблер PATCH `caption`/`preset`
  юзера) → fall-through к питоновской outline-ветке (`BorderStyle=1`) — **отключение доступно**.
- **Приёмка (задача #33):** рендер `example_original.mp4` (workspace/ae61067), кадр t=2.5 — за текстом
  сабов «MY CHATGPT SUBSCRIPTION.» СПЛОШНАЯ ЧЁРНАЯ плашка (S-style `,3,11,0,2,` + `&H00000000` плита),
  глазами как эталон. Кадры порт|эталон рядом. S-строка ASS верифицирована: `Style: S,Oswald,44,…,3,11,0,2,…`.
- **Регресс-якоря** (`dub-captions/src/lib.rs`): `greedy_example_original_default_black_plate` (дефолт →
  `3,11` чёрная плита), `plate_disabled_falls_back_to_border1` (`plate=false` → `1,2` outline),
  `textured_scene_has_default_plate_but_no_cover_kp`.

Старый вывод раунда 5 (ниже, «расхождений НЕТ») относился к паритету с captions.py на greedy-входе —
теперь СОЗНАТЕЛЬНО перекрыт этим продуктовым дефолтом по приказу юзера. Паритетная ветка сохранена и
доступна тумблером (`plate=false`).

## Раунд 5 — РЕГРЕСС ТИТРА починен (задача #34, приказ юзера 2026-07-12)

**Корень найден:** прогон `ae61067` отрисовал 0 титр-Dialogue НЕ из-за флаки-матчинга, а потому что
его `project.json` создан **00:59**, а compose-фикс (bbox-матчинг, f6fd476) закоммичен **01:51** — тот
артефакт СТАРШЕ фикса. Титр шёл из `raw_ctx` с `bbox=null` (`start=0/end=0`), `emit_title` его скипал
(ass.rs:322 — `bbox.len()<4 → return`). Текущий `compose::run` матчит `ctitles→localize` и ВСЕГДА даёт
валидный bbox: матч OCR-боксов ИЛИ **fallback центр-полоса** (`near_idx.is_empty()` → центр 8%..92%).
Пути к «bbox=null из run» нет — каждый loc_block имеет конкретный bbox.

- **Доказательство:** свежий прогон OCR+compose на том же `example_original.mp4` (verify_captions) даёт
  `композит: титров=1 (bbox)` и `KT`-Dialogue «THAT VERY "PROGRESSIVE" ACQUAINTANCE» в ASS. Стабильно
  на **3 прогонах подряд** (все: титров=1, блюр 33, sub_px=24 — детерминизм). Титр ВИДЕН на кадре t=2.5.
- **Регресс-тест:** `compose.rs::ae61067_title_always_drawn_with_bbox` — ТОЧНЫЙ титр ae61067 в ДВУХ
  входах (с localize-боксами и с ПУСТЫМ localize) → оба дают ровно 1 нарисованный титр с непустым tgt и
  `bbox.len()==4` (никогда 0). Гварда против «титр молча пропал».

## PATCH-хвост (раунд 5, задача #35) — все op портированы 1:1 с app.py

`caption` (глобальный sub_style ИЛИ per-seg override; тумблер `plate`), `del_segment`, `hide_segment`,
`del_segments`, `hide_segments`, `del_titles`, `del_blurs`, `keep_segment`, `keep_segments`, `blur`,
`blur_add`, `blur_del`, `blur_enable`, `preset`, `title`, `title_del`, `title_add` — `crates/dub-server/
src/patch.rs`. Коды ошибок как питон: битый op/поле → 400; неизвестный idx/seg → 404. Юнит-тесты на
caption(global+per-seg), del_segment(404), blur-цикл, title-цикл, preset, del_titles/del_blurs.

**CaptionOverride per-seg в build_ass:** `render.rs::build_ass` теперь читает `overrides[seg_id].text`
и рисует ЕГО вместо `tgt_text` (продуктовое требование «научить build_ass читать»). Стилевые per-seg
поля (`style/x/y/w/fs`) сохраняются round-trip, но в ASS-строку пока не вплетаются — это совпадает с
питоном (`write_artifacts` overrides в план НЕ прокидывает; dub-captions строит субтитр из общего
sub_style). Осознанное ограничение: текст-оверрайд (главный кейс редактора) работает.

## Раунд 5 — паритет плашек: семплинг vision + заливка полос (задача #28)

**Итог: расхождений НЕТ. Порт уже дословно совпадает с источником истины; «плашка эталона» —
артефакт УСТАРЕВШЕГО пайплайна.** Доказано живым прогоном Gemma + честным прогоном `captions.py`.
**ВНИМАНИЕ:** этот вывод перекрыт продуктовым дефолтом «подложка сабов» выше (приказ юзера 2026-07-12).

### 1. Семплинг LLM — сверен построчно, УЖЕ верный (не «temp 0.3 всюду» из отчёта р3)
Каждый вызов `dub-llm` шлёт РОВНО питоновские per-call параметры (проброшены через `Sampling`):

| Вызов | Питон (источник) | Порт | temp / top_p / top_k / rep_pen |
|-------|------------------|------|--------------------------------|
| ctx `ask`/`imsg` дефолт | ctx_translate.py:67-69 | vision.rs:125, ctx.rs:134 | 0.2 / 0.95 / 64 / — |
| **sub-style SP (GREEDY)** | ctx_translate.py:163 | vision.rs:242 | **0.0** / 0.95 / 64 / — |
| VP мега-промпт | ctx_translate.py:168 | vision.rs:253 | 0.2 / 0.95 / 64 / — |
| scene-контекст | ctx_translate.py:256 | vision.rs:404 | 0.2 / 0.95 / 64 / — |
| audio-контекст | ctx_translate.py:276 | ctx.rs:203 | 0.2 / 0.95 / 64 / — |
| ctx TRANSLATE (TP) | ctx_translate.py:302 | ctx.rs:134 | 0.2 / 0.95 / 64 / — |
| MT glossary `_chat` | translate.py:55-59 | translate.rs:109 | 0.2 / 0.9 / — / — |
| MT `_translate_one` fallback | translate.py:92 | translate.rs:74 | 0.7 / 0.6 / 20 / 1.05 |
| MT batch `_run_hunyuan` | translate.py:144 | translate.rs:176 | 0.3 / 0.9 / 20 / 1.05 |
| MT `rewrite` | translate.py:193 | translate.rs:248 | 0.85 / 0.95 / 40 / 1.05 |

### 2. Ветки плашек `captions.py` build() — дословный порт (`build_s_style`, lib.rs)
Заливка полосы управляется ИСКЛЮЧИТЕЛЬНО тем, что vision вернул в `sub_style.background`:
- `bg=solid hex`, контраст к тексту ≥0.20 → **BorderStyle=3, Outline=11**, плита цвета полосы
  (captions.py 507-510). Это ветка ЭТАЛОННОЙ чёрной полосы.
- `bg=none`, светлый текст (lum>0.45) → **BorderStyle=1, Outline≈2**, тонкий outline, БЕЗ плашки
  (captions.py 511-515).
- `bg=none`, тёмный текст → BorderStyle=3, Outline=10, почти-белая плита (516-518).
- «boxed»-preset с плашкой (519-521) — **МЁРТВЫЙ путь**: фолбэк `if not sub_style` (462-469) ВСЕГДА
  назначает `sub_style` (bg=none) до `if sub_style:` (488), поэтому preset-плита недостижима. Порт
  повторяет это точно (default_style → light-ветка BorderStyle=1). Проверено `sub_style=None`-прогоном
  питона: тоже BorderStyle=1,2.
`_emit_title.has_plate` (ass.rs:382) = `bg && bg!="none" && |lum(bg)-lum(txt)|>=0.20` — идентично 421.

### 3. Живой greedy-vision `example_original.mp4` (пример `vision_probe`, temp=0.0)
Все 10 кейфреймов дают согласованно: `background=""`(→none), `background_color=null`,
`scene_color=#E0E0E0/#D3D3D3`, `scene_flat=false`, титр `bg=null`. Т.е. Gemma ЧЕСТНО читает субтитр
оригинала как БЕЛЫЙ-С-ЧЁРНЫМ-OUTLINE поверх сцены — сплошной полосы В ОРИГИНАЛЕ НЕТ. Агрегация bg в
порту (vision.rs:284-286,351) дословно = питон (ctx_translate.py:197-199,228): majority-vote «none».

### 4. Приёмка пикселями (кадры 464×824, PIL Counter, доминанта зоны, квант 16)
- **Субтитр порт vs питон-источник-истины** (тот же greedy sub_style): S-строка **байт-в-байт**
  `Style: S,Oswald,66,…,1,2,0,2,…` (порт ae61067 caps.ass == pygreedy/greedy.ass). Зона субтитра
  x[120:350]y[615:665]: порт `#F0F0F0 31.1% / #000000 14.4%` ≈ питон `31.3% / 15.0%` — совпадает.
- **Эталон `example_dub.mp4`**: полосы ЕСТЬ (текст-хаг тёмные плиты) — субтитр inter-line x[180:280]
  доминанта `#000000` 23% (в оригинале там `#B06050` — сцена, плиты нет ⇒ эталон её РИСОВАЛ).
  НО эти плиты `#000000`-класс воспроизводит ТОЛЬКО ветка `bg=solid` (candidate A: py_build.py →
  A_solid_black), которая на текущем greedy-входе НЕ активируется ни в питоне, ни в порту.
- **Вывод**: эталон `example_dub.mp4` отрисован УСТАРЕВШИМ пайплайном (старый «boxed везде» до
  LLM-driven-плашек). Текущий источник истины (`ctx_translate.py`+`captions.py`), накормленный живым
  greedy-чтением этого видео, даёт outline-субтитры БЕЗ плашки — ровно как порт. **Приводить порт к
  «плашке эталона» = отклониться от источника истины** (запрещено контрактом). Порт оставлен как есть.

### Регресс-якоря (dub-captions/src/lib.rs)
- `greedy_example_original_substyle_is_border1_no_plate` — точный живой greedy sub_style → BorderStyle=1,
  без KP-плашки (паритет с captions.py на том же входе).
- `solid_band_reproduces_reference_black_plate` — `bg=#000000` → BorderStyle=3,11 чёрная плита
  (ветка, которой отрисован эталон; активна ТОЛЬКО при solid-чтении vision).
- Диагностика: `DUB_VISION_DEBUG=1` печатает per-keyframe raw sub-style read (vision.rs).

### ЗАКРЫТО (задача #34) — регресс титра «0 титр-Dialogue»
Причина: артефакт ae61067 СТАРШЕ compose-фикса (см. раздел «РЕГРЕСС ТИТРА починен» выше). Свежий
compose всегда даёт титру bbox (матч ИЛИ fallback центр-полоса); титр «THAT VERY…» ВИДЕН, стабильно
на 3 прогонах. Регресс-тест `ae61067_title_always_drawn_with_bbox` гвардит от рецидива.
