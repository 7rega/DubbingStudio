//! Рендер-ядро (порт render-половины pipeline.run + _build_dub TTS-ветки + assemble + compose/mix +
//! captions.build/burn + mux). От Project с переводом до готового дублированного MP4.
//!
//! Конвейер (последовательный, GPU-стадии по очереди — VRAM-инвариант):
//!   probe -> extract 44.1k -> separate (vocals/instrumental через dub-sep) ->
//!   per-segment Higgs clone TTS (реф из вокала) -> fit_to_slot (atempo) -> timeline ->
//!   mix (instrumental + dub) -> build ASS (титры/субтитры/sub_style из Project) ->
//!   burn (blur боксы из project.captions.blur_boxes) -> mux.
//!
//! regen (на экспорте): ре-TTS ТОЛЬКО dirty-сегментов. Кэш per-segment WAV в work_dir (seg_XXX.wav):
//! не-dirty переиспользуются, dirty пере-синтезируются (улучшение против питон-_regen_dub, что гнал
//! весь дубляж заново — правка #10). tgt-текст пустой -> сегмент молчит (как в питоне).

use audiocpp::AudiocppEngine;
use dub_captions::{BlurBox, Sub, SubStyle as CapSubStyle, Title as CapTitle};
use dub_core::{Project, SubStyle as CoreSubStyle, Title as CoreTitle};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use crate::media;
use crate::wavio;

/// Транскрибировать реф-клипы спикеров выбранным ASR-движком, заполнив ref_texts (уже заданные — пропуск).
/// Пустой транскрипт не пишем. Общий шаг pack-рефов и обрезанных клон-рефов.
fn fill_ref_texts(
    asr: &mut dyn dub_asr::AsrEngine,
    refs: &std::collections::BTreeMap<String, PathBuf>,
    ref_texts: &mut std::collections::BTreeMap<String, String>,
) {
    for (spk, refp) in refs {
        if ref_texts.contains_key(spk) {
            continue;
        }
        if let Ok(rsegs) = asr.transcribe(refp, "auto") {
            let txt = rsegs
                .iter()
                .map(|s| s.text.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            // токены уже trim'нуты и непусты, join(" ") не даёт краевых пробелов -> повторный trim не нужен.
            if !txt.is_empty() {
                ref_texts.insert(spk.clone(), txt);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineKey {
    pub dll: PathBuf,
    pub model_root: PathBuf,
    pub backend: String,
    pub device: i32,
    pub threads: i32,
    pub quant: String,
}

/// Пути к моделям/движкам для рендера (резолвятся из AppState).
pub struct RenderPaths {
    pub input: PathBuf,        // исходное видео
    pub work_dir: PathBuf,     // workspace/<pid>
    pub output: PathBuf,       // output.mp4
    pub bsroformer_cli: PathBuf,
    pub bsroformer_model: PathBuf,
    pub higgs_dll: PathBuf,
    pub higgs_model_root: PathBuf,
    pub higgs_quant: String,   // квант выбранного варианта Higgs (q8_0/q6_k/q4_k_m) — для audiocpp load_model
    pub fonts_dir: PathBuf,
    pub higgs_backend: String, // "cuda" | "cpu"
    pub higgs_device: i32,
    pub higgs_threads: i32,
    pub max_stretch: f64,
    pub voices_dir: PathBuf,   // каталог голосов-паков + записей с микрофона
    pub asr: crate::models::AsrChoice, // выбранный ASR-движок — авто-транскрипция реф-клипа (ref_text клона)
    pub bench: bool,           // пер-стадийный бенчмарк (галка настроек, ВЫКЛ по умолчанию)
    pub ref_secs: f64,         // длина реф-клипа клона голоса, сек (настройка «Экономия RAM», дефолт 12.0)
    pub models_root: PathBuf,  // каталог моделей (active.json) — читаем настройки облачного TTS OpenRouter
    pub tts_cache: Arc<Mutex<Option<(EngineKey, Arc<AudiocppEngine>)>>>,
}

pub type Progress<'a> = dyn Fn(Value) + Send + Sync + 'a;

fn emit(progress: &Progress, stage: &str, msg: &str) {
    progress(json!({ "stage": stage, "msg": msg }));
}

/// Готовый результат рендера.
#[allow(dead_code)]
pub struct RenderResult {
    pub output: PathBuf,
}

/// Отрендерить Project -> output.mp4. `regen_dub` — ре-синтез dirty-сегментов дубляжа (иначе кэш).
pub fn run(
    proj: &Project,
    paths: &RenderPaths,
    regen_dub: bool,
    progress: &Progress,
) -> Result<RenderResult, String> {
    dub_captions::set_fonts_dir(&paths.fonts_dir);
    let wd = &paths.work_dir;
    std::fs::create_dir_all(wd).map_err(|e| e.to_string())?;
    // Бенчмаркинг рендера (галка настроек, ВЫКЛ по умолчанию): probe / dub_audio / burn / mux -> bench.json.
    let mut bench = crate::bench::Bench::start(wd, "render", paths.bench);
    bench.stage("probe");

    // probe: длительность/размеры.
    let meta = media::probe(&paths.input)?;
    let total = if proj.meta.duration > 0.0 {
        proj.meta.duration
    } else {
        meta.duration
    };
    let (vw, vh) = (
        if proj.meta.width > 0 { proj.meta.width } else { meta.width },
        if proj.meta.height > 0 { proj.meta.height } else { meta.height },
    );
    let src_codec = if !proj.meta.src_codec.is_empty() {
        proj.meta.src_codec.clone()
    } else {
        meta.src_codec.clone()
    };
    emit(progress, "probe", &format!("вход {}x{} dur={:.1}s", vw, vh, total));

    // voiceover (закадровый) = как dub, но оригинал слышно приглушённым ПОД переведённым голосом.
    let is_voiceover = proj.mode == "voiceover";
    let is_dub = proj.mode == "dub" || is_voiceover;
    let keep_music = proj.audio.keep_music;

    // ── АУДИО ──────────────────────────────────────────────────────────────────
    // Готовим финальную аудио-дорожку new_audio: dub (клон) поверх инструментала, либо оригинал.
    bench.stage("dub_audio");
    let new_audio: PathBuf = if is_dub {
        build_dub(proj, paths, total, keep_music, is_voiceover, regen_dub, progress)?
    } else {
        // nodub/transcribe: оставляем оригинальную дорожку — mux возьмёт её из исходного видео.
        emit(progress, "mix", "nodub: оригинальная аудиодорожка");
        paths.input.clone()
    };

    // ── АУДИО-РЕЖИМ (вход без видео) ────────────────────────────────────────────
    // Нет видеокадра (vw/vh<=0) -> результат = сведённый дубляж как WAV. Ни бёрна, ни титров, ни mux
    // (нечего накладывать/муксить). Пачка WAV -> пачка озвученных WAV.
    if vw <= 0 || vh <= 0 {
        let out_wav = paths.output.with_extension("wav");
        media::to_wav(&new_audio, &out_wav)?;
        // Прибрать stale output.mp4/.mkv от прошлого прогона: find_output отдаёт их приоритетнее wav (#116).
        for ext in ["mp4", "mkv"] {
            let stale = paths.output.with_extension(ext);
            if stale.is_file() {
                let _ = std::fs::remove_file(&stale);
            }
        }
        emit(progress, "done", &format!("готово (только аудио) -> {}", out_wav.display()));
        return Ok(RenderResult { output: out_wav });
    }

    // ── КАПШЕНЫ + BURN (только если subs.burn) ─────────────────────────────────
    // subs.burn=false -> НИКАКИХ наложений (ни субтитров, ни титров/блюра): чистое видео + новая
    // дорожка. Композируемость: дубляж/закадр без субтитров на картинке.
    // Есть ли ВООБЩЕ что накладывать? subs=none + нет титров + нет блюр-боксов (band уже исключён при
    // subs=none) -> накладывать нечего, полный ffmpeg-транскод бессмыслен (экономия времени, баг-репорт).
    let has_overlay = proj.subs.mode != "none"
        || !proj.captions.titles.is_empty()
        || !collect_blur_boxes(proj).is_empty();
    bench.stage("burn");
    let captioned = if proj.subs.burn && has_overlay {
        emit(progress, "build", "сборка ASS (титры + дублированные субтитры)");
        let ass_path = wd.join("caps.ass");
        let sub_covers = build_ass(proj, &ass_path, vw, vh, total)?;
        emit(progress, "burn", "вжигание субтитров + блюр (ffmpeg + libass, NVENC)");
        let mut blur_boxes = collect_blur_boxes(proj);
        blur_boxes.extend(sub_covers.iter().map(cover_to_blur)); // блюр-подложка ПОД нашим текстом
        let captioned = wd.join("captioned.mp4");
        dub_captions::burn(
            &paths.input,
            &ass_path,
            &captioned,
            &blur_boxes,
            Some((vw, vh)),
            proj.render.blur,
            true, // gpu_encode (NVENC)
            true, // gpu_decode
            proj.render.burn_cq,
            Some(&src_codec),
            proj.render.blur_sigma,
        )?;
        captioned
    } else {
        emit(progress, "burn", "субтитры/титры отключены (subs.burn=off)");
        paths.input.clone()
    };

    // ── MUX ────────────────────────────────────────────────────────────────────
    bench.stage("mux");
    emit(progress, "mux", "муксирование видео + аудио");
    // Экспорт с ОРИГИНАЛЬНОЙ дорожкой (#113): дубляж (default, 1-я) + оригинал (2-я). Только dub/voiceover
    // (в nodub/transcribe оригинал уже основной — вторая дорожка ни к чему). Контейнер mp4|mkv из настроек.
    // Выход — output.<container>; при ошибке мультитрек-mux — фолбэк на обычный одинодорожечный mux.
    let mut out_path = paths.output.clone();
    // voiceover тоже может нести оригинал 2-й дорожкой: 1-я = микс (перевод поверх приглушённого ориг.),
    // 2-я = ЧИСТЫЙ оригинал без перевода — своя ценность, НЕ дубль (бай-дизайн). Требует аудио в источнике.
    let two_track = is_dub && proj.audio.keep_original_track && media::has_audio(&paths.input);
    let mkv = two_track && proj.audio.container == "mkv";
    let mut muxed = false;
    if two_track {
        let container = if mkv { "mkv" } else { "mp4" };
        out_path = paths.output.with_extension(container);
        let dub_lang = media::iso639_1_to_2(&proj.tgt_lang);
        let src_code = proj
            .meta
            .extra
            .get("src_lang")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let orig_lang = media::iso639_1_to_2(src_code);
        let dub_title = format!("{} (дубляж)", lang_display(&proj.tgt_lang));
        let orig_title = format!("{} (оригинал)", lang_display(src_code));
        emit(progress, "mux", &format!("две дорожки: {dub_title} + {orig_title} -> {container}"));
        match media::mux_multitrack(
            &captioned, &new_audio, &paths.input, &out_path,
            dub_lang, orig_lang, &dub_title, &orig_title,
        ) {
            Ok(()) => muxed = true,
            Err(e) => {
                emit(progress, "mux", &format!("мультитрек-mux не удался ({e}) -> одна дорожка"));
                out_path = paths.output.clone();
            }
        }
    }
    if !muxed {
        if is_dub || new_audio != paths.input {
            media::mux(&captioned, &new_audio, &out_path)?;
        } else {
            // nodub/транскрипт: дубляжа нет — оригинальную дорожку КОПИРУЕМ без перекода (каналы 5.1/
            // частота/битрейт как есть). Раньше mux() форсил stereo+AAC и портил звук на ровном месте.
            media::mux_keep_audio(&captioned, &paths.input, &out_path)?;
        }
    }
    // MKV-компаньон (#116, находка [3]): WebView2 не играет Matroska -> плеер редактора мёртв. Всегда
    // держим playable output.mp4 (дубляж-дорожка, copy без перекода) РЯДОМ с output.mkv: плеер тянет mp4,
    // «Сохранить» отдаёт mkv. Успешный mkv-mux -> ремукс лёгкого mp4; иначе mp4 уже основной выход.
    let mp4_companion = paths.output.with_extension("mp4");
    if mkv && muxed {
        match media::remux_playable_mp4(&out_path, &mp4_companion) {
            Ok(()) => {} // валидный playable-компаньон рядом с output.mkv
            Err(e) => {
                // Ремукс упал -> компаньон битый/частичный ИЛИ остался stale mp4 от прошлого прогона.
                // Обязательно удалить (find_output отдаёт mp4 приоритетнее mkv -> иначе плеер получит
                // битьё/старьё вместо свежего mkv, регресс #116 находки [0][1]). VLC играет mkv напрямую.
                let _ = std::fs::remove_file(&mp4_companion);
                emit(progress, "mux", &format!("mp4-компаньон не собран ({e}) — плеер откроет mkv (VLC ок)"));
            }
        }
    } else {
        // не-mkv выход: прибрать stale output.mkv от прошлого экспорта (find_output отдаёт mkv приоритетнее).
        let stale_mkv = paths.output.with_extension("mkv");
        if stale_mkv != out_path && stale_mkv.is_file() {
            let _ = std::fs::remove_file(&stale_mkv);
        }
    }

    emit(progress, "done", &format!("готово -> {}", out_path.display()));
    bench.finish(|m| emit(progress, "bench", m));

    // Авто-выгрузка Higgs TTS из VRAM на видеокартах < 20 ГБ, чтобы не держать память занятой
    let snap = crate::hw::snapshot();
    if snap.total_vram > 0 && snap.total_vram < 20 * 1024 * 1024 * 1024 {
        crate::evict_tts_cache();
    }

    Ok(RenderResult { output: out_path })
}

/// Человекочитаемое имя языка для title дорожки. Нативных имён в проекте нет — берём английское имя из
/// dub_translate::WHISPER_LANGS (единый источник языков), для незнакомого/auto — код заглавными.
fn lang_display(code: &str) -> String {
    let lc = code.trim().to_lowercase();
    dub_translate::WHISPER_LANGS
        .iter()
        .find(|(k, _)| *k == lc.as_str())
        .map(|(_, v)| v.to_string())
        .unwrap_or_else(|| {
            if lc.is_empty() || lc == "auto" {
                "Original".to_string()
            } else {
                code.to_uppercase()
            }
        })
}

/// Только ДУБ-АУДИО (без бёрна субтитров и mux видео): TTS+fit+timeline+mix -> work_dir/dub_audio.m4a.
/// Нужно, чтобы озвучку можно было СЛУШАТЬ в редакторе сразу после анализа, НЕ собирая финальное видео
/// (само видео на превью не нужно — кадры показывает per-frame preview). Порт _build_dub-ветки без бёрна.
pub fn dub_audio(
    proj: &Project,
    paths: &RenderPaths,
    regen_dub: bool,
    progress: &Progress,
) -> Result<PathBuf, String> {
    let wd = &paths.work_dir;
    std::fs::create_dir_all(wd).map_err(|e| e.to_string())?;
    let meta = media::probe(&paths.input)?;
    let total = if proj.meta.duration > 0.0 { proj.meta.duration } else { meta.duration };
    let src: PathBuf = if proj.mode == "dub" || proj.mode == "voiceover" {
        build_dub(proj, paths, total, proj.audio.keep_music, proj.mode == "voiceover", regen_dub, progress)?
    } else {
        paths.input.clone() // nodub/transcribe -> оригинальная дорожка
    };
    let out = wd.join("dub_audio.m4a");
    // Если build_dub уже выдал final_audio.m4a, копируем его в dub_audio.m4a для плеера
    if src.is_file() && src.extension().and_then(|x| x.to_str()) == Some("m4a") {
        let _ = std::fs::copy(&src, &out);
    } else {
        let _ = media::extract_audio(&src, &out, 44_100, 2);
    }
    emit(progress, "done", "дуб-аудио готово");
    Ok(out)
}

/// Нижний предел приглушения оригинала в режиме voiceover (закадровый): -40 dB ≈ почти тихо.
/// Само значение регулирует пользователь (proj.audio.voiceover_gain_db, дефолт -6 dB).
const VOICEOVER_DUCK_MIN_DB: f64 = -40.0;

/// Анти-артефактный ретрай синтеза фразы: до MAX_TTS_ATTEMPTS попыток; детект дефектов — synth_defect
/// (in-memory по сэмплам voice_clone, ~мкс, без ffmpeg-субпроцесса). Если ПОДРЯД больше CONSECUTIVE_ABORT
/// артефакт-фраз ИЛИ суммарно ретраев больше бюджета (по длине ролика) — стоп с ошибкой (стенд/рефы).
/// Лестница: 1 — дефолт; 2-3 — temp-бамп (0.9/1.2); 4-5 — АЛЬТЕРНАТИВНЫЙ реф спикера (+temp).
/// Смена рефа выбивает вырожденный гул там, где сэмплинг бессилен (QC-вывод: битые кучкуются
/// по коротким фразам с конкретными рефами).
const MAX_TTS_ATTEMPTS: usize = 5;
const CONSECUTIVE_ABORT: usize = 8;

/// Грейс после cancel(): ждём, пока отменённая генерация РЕАЛЬНО выйдет из DLL, прежде чем движок снова
/// тронут ДРУГИМ вызовом. Контракт audiocpp::Engine — single-caller (Send+Sync годен ТОЛЬКО под сериализацией
/// джоб-очереди, engine.rs). Параллельно тронуть движок, пока прошлый вызов в DLL, = гонка/порча.
const GUARD_GRACE_SECS: u64 = 20;
/// Префикс ошибки «движок застрял в DLL после cancel»: вызывающий НЕ ретраит (это был бы конкурентный FFI —
/// гонка), а ОБРЫВАЕТ рендер. Единственный воркер разблокируется (ошибка), состояние движка не портим.
const ENGINE_STUCK: &str = "ENGINE_STUCK";

/// voice_clone с ЖЁСТКИМ таймаутом. Higgs (прекомпил-DLL) СТОХАСТИЧЕСКИ виснет на редких сегментах; in-process
/// FFI не убить, но есть C-ABI `cancel()`. Гоним синтез в отдельном потоке с таймаутом. По таймауту: cancel()
/// + ЖДЁМ grace, пока поток реально ВЫЙДЕТ из DLL (нельзя трогать движок конкурентно — single-caller контракт).
/// Вышел -> движок свободен, Err(таймаут) => вызывающий ретраит. Не вышел -> Err(ENGINE_STUCK) => вызывающий
/// обрывает рендер (НЕ гоняет движок параллельно с зависшим потоком). Так воркер не блокируется навечно и
/// не ловит гонку FFI.
fn voice_clone_guarded(
    engine: &Arc<AudiocppEngine>,
    text: &str,
    ref_wav: &str,
    ref_text: Option<&str>,
    opts: &str,
    timeout: Duration,
) -> Result<(Vec<f32>, i32), String> {
    let (tx, rx) = mpsc::channel();
    let eng = engine.clone();
    let (t, rw, rt, op) = (
        text.to_string(),
        ref_wav.to_string(),
        ref_text.map(|s| s.to_string()),
        opts.to_string(),
    );
    std::thread::spawn(move || {
        let r = eng
            .voice_clone(&t, &rw, rt.as_deref(), &op)
            .map_err(|e| e.to_string());
        let _ = tx.send(r); // получателя уже нет по таймауту — send вернёт Err, не паникуем
    });
    match rx.recv_timeout(timeout) {
        Ok(r) => r,
        Err(_) => {
            engine.cancel(); // сигнал движку остановить текущую генерацию
            // Ждём фактического выхода отменённого вызова из DLL — только тогда движок снова можно трогать.
            match rx.recv_timeout(Duration::from_secs(GUARD_GRACE_SECS)) {
                Ok(_) => Err(format!("таймаут синтеза >{}с — отменён, движок свободен", timeout.as_secs())),
                Err(_) => Err(format!(
                    "{ENGINE_STUCK}: синтез не отменяется >{}с — рендер прерван (движок завис в DLL)",
                    timeout.as_secs() + GUARD_GRACE_SECS
                )),
            }
        }
    }
}

/// Детект TTS-артефакта «гудение» по сэмплам фразы (in-memory, прямо из voice_clone). Речь на клипе >0.4с
/// всегда имеет паузы (тихие кадры) и большой размах громкости; непрерывный гул — почти без пауз и с
/// плоской огибающей. Покадровый RMS (окно 25мс): silent_frac (доля кадров тише peak−30дБ = паузы) и
/// range_db (размах peak↔trough). Артефакт, если silent_frac < 5% И range_db < 14дБ. Эмпирика: 4 реальных
/// гудящих фразы → тишина 0%, размах 3-11дБ; 90 нормальных → размах в среднем 53дБ (0 ложных). Быстрее и
/// проще ffmpeg-субпроцесса: сэмплы уже в руках, один проход арифметики, ноль зависимостей.
/// Детектор дефектов синтеза v2. Пороги подобраны ПО ДАННЫМ QC-прогона (69 битых / 80 чистых,
/// ноль ложных на чистой выборке, 2026-07-17):
/// - "runaway": клип длиннее max(6с, 0.4с×символ) — модель ушла в гул до токен-капа (факт: фраза
///   2.5с → клип 40.7с; таких найдено 7+, часть с ПРАВИЛЬНЫМ началом — ASR-sim их не ловил);
/// - "обрыв": ≥4 символов, а клип < 0.045с/симв (факт: «Погнали!» за 0.36с);
/// - "тишина": пик покадрового RMS < 0.02 (минимум чистых 0.0213; провалы 0.014-0.0198 —
///   СТАРЫЙ детектор их намеренно пропускал гейтом «peak<0.0056 = не судим»);
/// - "гул": размах < 16дБ при почти нулевых паузах (уточнённый старый паттерн).
/// None ≠ гарантия чистоты: финальную правду даёт ASR-верификация (QC-пасс после синтеза).
fn synth_defect(samples: &[f32], sr: i32, tgt_chars: usize) -> Option<&'static str> {
    if sr <= 0 || samples.is_empty() {
        return None;
    }
    let srn = sr as usize;
    let dur = samples.len() as f64 / srn as f64;
    if tgt_chars >= 1 && dur > (tgt_chars as f64 * 0.4).max(6.0) {
        return Some("runaway");
    }
    if tgt_chars >= 4 && dur < tgt_chars as f64 * 0.045 {
        return Some("обрыв");
    }
    let w = srn / 40;
    if w == 0 || samples.len() < w * 4 {
        return None;
    }
    let frames = samples.len() / w;
    let mut rms: Vec<f64> = Vec::with_capacity(frames);
    for f in 0..frames {
        let mut acc = 0.0f64;
        for i in 0..w {
            let v = samples[f * w + i] as f64;
            acc += v * v;
        }
        rms.push((acc / w as f64).sqrt());
    }
    let peak = rms.iter().cloned().fold(0.0f64, f64::max);
    if peak < 0.02 {
        return Some("тишина");
    }
    let thr = peak * 0.0316;
    let silent = rms.iter().filter(|&&r| r < thr).count() as f64 / frames as f64;
    let trough = rms.iter().cloned().filter(|&r| r > 1e-9).fold(peak, f64::min);
    let range = 20.0 * (peak / trough.max(1e-9)).log10();
    if range < 16.0 && silent < 0.03 {
        return Some("гул");
    }
    None
}

/// Удаляет управляющие теги Higgs (<|...|>) и нормализует пробелы
pub fn strip_higgs_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut i = 0;
    while i < len {
        if !in_tag && i + 1 < len && chars[i] == '<' && chars[i + 1] == '|' {
            in_tag = true;
            i += 2;
        } else if in_tag && i + 1 < len && chars[i] == '|' && chars[i + 1] == '>' {
            in_tag = false;
            i += 2;
        } else if !in_tag {
            out.push(chars[i]);
            i += 1;
        } else {
            i += 1;
        }
    }
    out.split_whitespace().collect::<Vec<&str>>().join(" ")
}

/// Похожесть ожидаемого перевода и услышанного ASR: нормализация (lowercase, ё→е, только буквы/цифры)
/// + доля общих слов от максимума. Мягкая метрика: ловим «совсем не то/тишину», не орфографию.
fn qc_similarity(expected: &str, heard: &str) -> f64 {
    let clean_expected = strip_higgs_tags(expected);
    let norm = |s: &str| -> Vec<String> {
        s.to_lowercase()
            .replace('ё', "е")
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .map(|w| w.to_string())
            .collect()
    };
    let a = norm(&clean_expected);
    let b = norm(heard);
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    // Односложный обрывок («При...», «Не»): точное словное сравнение даёт ложный брак на правильном
    // синтезе («Пре-», «Ниии...») и жжёт пересинтезы. Сравниваем ПРЕФИКСЫ (3 буквы); совсем короткое
    // (≤2 букв) с пустым ASR — «сомнительно» (0.5), не брак: VAD системно молчит на клипах <0.5с,
    // а настоящую тишину ловит акустический префильтр (synth_defect).
    if a.len() == 1 && a[0].chars().count() <= 6 {
        if b.is_empty() {
            return if a[0].chars().count() <= 2 { 0.5 } else { 0.0 };
        }
        // Вой-паттерн: услышанное кратно длиннее ожидания («Ну» -> «Нуууу…», «О,» -> «ОООО…») —
        // префикс совпадает, но это артефакт, не речь.
        let total_b: usize = b.iter().map(|w| w.chars().count()).sum();
        if total_b > a[0].chars().count() * 3 + 4 {
            return 0.0;
        }
        let ap: String = a[0].chars().take(3).collect();
        return if b.iter().any(|w| w.starts_with(ap.as_str())) { 1.0 } else { 0.0 };
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let bs: std::collections::HashSet<&str> = b.iter().map(|s| s.as_str()).collect();
    let hit = a.iter().filter(|w| bs.contains(w.as_str())).count();
    hit as f64 / a.len().max(b.len()) as f64
}

/// Динамический размах фразы (дБ, peak↔trough покадрового RMS 25мс) — скалярный «скор чистоты» для
/// выбора наименее плохой попытки, когда ВСЕ ретраи с артефактом: речь ~53дБ, гул 3-11дБ (та же
/// эмпирика, что у гул-ветки synth_defect). Невалидный/короткий клип -> 0.0 (хуже всех).
fn hum_range_db(samples: &[f32], sr: i32) -> f64 {
    if sr <= 0 {
        return 0.0;
    }
    let sr = sr as usize;
    let w = sr / 40;
    if w == 0 || samples.len() < w * 4 {
        return 0.0;
    }
    let frames = samples.len() / w;
    let mut rms: Vec<f64> = Vec::with_capacity(frames);
    for f in 0..frames {
        let mut acc = 0.0f64;
        for i in 0..w {
            let v = samples[f * w + i] as f64;
            acc += v * v;
        }
        rms.push((acc / w as f64).sqrt());
    }
    let peak = rms.iter().cloned().fold(0.0f64, f64::max);
    let trough = rms.iter().cloned().filter(|&r| r > 1e-9).fold(peak, f64::min);
    20.0 * (peak / trough.max(1e-9)).log10()
}

/// Полный аудио-конвейер дубляжа -> путь к new_audio. Порт _build_dub/_regen_dub (TTS+fit+timeline+mix).
fn build_dub(
    proj: &Project,
    paths: &RenderPaths,
    total: f64,
    keep_music: bool,
    voiceover: bool,
    regen_dub: bool,
    progress: &Progress,
) -> Result<PathBuf, String> {
    let wd = &paths.work_dir;
    // Сегменты с непустым tgt (как в питоне: только строки с текстом синтезируются). Несём индекс в
    // ПОЛНОМ списке proj.segments — слот next.start считается по индексу i+1 полного списка (порт
    // pipeline.py:207: nxt = segs[i+1].start, где segs — весь транскрипт, пустые пропускаются continue,
    // но индекс i+1 идёт по полному списку).
    // Порт project.write_artifacts: HIDDEN строки исключаются целиком (нет ни дубляжа, ни субтитра);
    // keep_original — остаются (сплайсим оригинал, без TTS), даже если tgt непустой.
    let seg_hidden = |s: &dub_core::Segment| s.extra.get("hidden").and_then(|v| v.as_bool()).unwrap_or(false);
    let seg_keep = |s: &dub_core::Segment| s.extra.get("keep_original").and_then(|v| v.as_bool()).unwrap_or(false);
    let segs: Vec<(usize, &dub_core::Segment)> = proj
        .segments
        .iter()
        .enumerate()
        .filter(|(_, s)| !seg_hidden(s) && (seg_keep(s) || !s.tgt_text.trim().is_empty()))
        .collect();
    if segs.is_empty() {
        emit(progress, "tts", "нет строк с переводом -> тишина, оригинальная дорожка");
        return Ok(paths.input.clone());
    }

    // 1) extract 44.1k stereo.
    emit(progress, "extract_audio", "извлечение аудио (ffmpeg 44.1k stereo)");
    let audio_hq = wd.join("audio_hq.wav");
    media::extract_audio(&paths.input, &audio_hq, 44100, 2)?;

    // 2) сепарация (vocals/instrumental) через dub-sep
    //    Даже если keep_music=false или voiceover=true (нам не нужен инструментал для финального микса),
    //    сепарация всё равно НУЖНА для клонирования голоса, чтобы референсы были без музыки.
    let stems = wd.join("stems");
    let cached_voc = stems.join("vocals.wav");
    let cached_inst = stems.join("instrumental.wav");

    let _needs_clone = proj.audio.voice.mode == "clone" || proj.audio.voice.name.as_deref().unwrap_or("").split(',').any(|s| s.trim() == crate::voice_slots::CLONE_SLOT);
    let want_inst = keep_music && !voiceover;
    let needs_clean_vocals = true; // ВСЕГДА пытаемся получить чистый вокал для референса (TTS/клонирование работает лучше)
    let mut did_sep = false;

    if want_inst || needs_clean_vocals {
        if cached_voc.is_file() && cached_inst.is_file() {
            emit(progress, "separate", "сепарация из кэша (stems уже посчитаны)");
            did_sep = true;
        } else if paths.bsroformer_cli.is_file() && paths.bsroformer_model.is_file() {
            emit(progress, "separate", "сепарация (Mel-Band Roformer voc_fv6)");
            let sep_mode = crate::models::sep_mode(&paths.models_root);
            match dub_sep::separate_with_mode(&audio_hq, &stems, &paths.bsroformer_cli, &paths.bsroformer_model, sep_mode) {
                Ok(_) => did_sep = true,
                Err(e) => emit(progress, "separate", &format!("ошибка сепарации: {e} -> без чистого вокала")),
            }
        } else if want_inst {
            emit(progress, "separate", "движок сепарации не найден -> без фона (keep_music off)");
        }
    }

    let instrumental = if want_inst && did_sep {
        Some(cached_inst.clone())
    } else {
        None
    };
    let vocals = if did_sep {
        cached_voc.clone()
    } else {
        audio_hq.clone()
    };

    // ref_vocals16 -> mono 16k (референсы клона). Для клонирования нужен максимально ЧИСТЫЙ вокал,
    // иначе нейросеть захватит шумы и музыку из фона.
    let vocals16 = wd.join("ref_vocals16.wav");
    let clean16 = wd.join("vocals16_clean.wav"); // from analyze.rs

    if cached_voc.is_file() {
        media::to_16k_mono(&cached_voc, &vocals16)?;
    } else if clean16.is_file() {
        std::fs::copy(&clean16, &vocals16).map_err(|e| e.to_string())?;
    } else {
        media::to_16k_mono(&vocals, &vocals16)?;
    }

    // 3) Подсчёт сегментов к синтезу. Кэш: seg_XXX.wav; если все сегменты уже в кэше и не dirty
    // (например, при экспорте готового проекта или пересведении) — мы ПОЛНОСТЬЮ пропускаем
    // долгий поиск референсов, ASR-проверки и тяжелую загрузку Higgs в VRAM!
    let dirty_count = segs
        .iter()
        .filter(|&(_, s)| {
            if seg_keep(s) || s.tgt_text.trim().is_empty() {
                return false;
            }
            let sid: String = s.id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
            let sid = if sid.is_empty() { format!("i{}", s.id) } else { sid };
            let raw = wd.join(format!("seg_{sid}.wav"));
            (regen_dub && s.dirty) || !raw.is_file()
        })
        .count();

    if dirty_count == 0 {
        emit(progress, "tts", &format!("синтез не требуется: все {} сегментов уже готовы (кэш)", segs.len()));
    } else {
        emit(progress, "tts", &format!("синтез {dirty_count} из {} сегментов", segs.len()));
    }

    // клон-референс. voice.mode="voice" -> реф из пака/записи (voices/<name>.wav|mp3) НА КАЖДОГО спикера.
    // Имя — CSV; позиция = отсортированный спикер (как во фронте: speaker ?? "0", лексикографически),
    // пустой слот берёт первое непустое имя. Иначе (clone) — длиннейшая реплика спикера из вокала.
    // Слот "-" (CLONE_SLOT, авто-распределение #114) — этот спикер остаётся на КЛОНИРОВАНИИ: пак-реф
    // не строим, его identity-реф добавляется ниже из вокала (spk_refs). Существующие CSV без "-"
    // ведут себя как раньше.
    let mut clone_slot_spks: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let pack_refs: std::collections::BTreeMap<String, PathBuf> = if dirty_count > 0 && proj.audio.voice.mode == "voice" {
        let names: Vec<&str> = proj.audio.voice.name.as_deref().unwrap_or("").split(',').map(|s| s.trim()).collect();
        let first_named = names
            .iter()
            .copied()
            .find(|n| !n.is_empty() && *n != crate::voice_slots::CLONE_SLOT);
        let mut map = std::collections::BTreeMap::new();
        if first_named.is_some() {
            let sorted: Vec<String> = proj
                .segments
                .iter()
                .map(|s| s.speaker.clone().unwrap_or_else(|| "0".to_string()))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            for (i, spk) in sorted.iter().enumerate() {
                let nm = names.get(i).copied().filter(|s| !s.is_empty()).or(first_named).unwrap_or("");
                if nm == crate::voice_slots::CLONE_SLOT {
                    clone_slot_spks.insert(spk.clone()); // спикер на клоне — identity-реф ниже
                    continue;
                }
                let src = ["wav", "mp3"].iter().map(|e| paths.voices_dir.join(format!("{nm}.{e}"))).find(|p| p.is_file());
                if let Some(src) = src {
                    let out = wd.join(format!("ref_pack_{i}.wav"));
                    // реф КАПИТСЯ до paths.ref_secs (дефолт 12с; на слабой RAM юзер уменьшает в настройках —
                    // длинный реф раздувает prefill-граф Higgs -> OOM на 32ГБ).
                    if media::trim(&src, &out, 0.0, paths.ref_secs, 16_000).is_ok() {
                        map.insert(spk.clone(), out);
                    }
                }
            }
        }
        map
    } else {
        std::collections::BTreeMap::new()
    };
    let use_pack = !pack_refs.is_empty();
    // ref_texts: расшифровка реф-клипа НА СПИКЕРА (Higgs клонирует качественнее с ref_text). Клон-режим —
    // src_text выбранного сегмента; пак-режим — АВТОТРАНСКРИПЦИЯ 12с-клипа (как Higgs build_speaker_reference;
    // пак-.txt = полный 3-мин транскрипт, к 12с не подходит). ASR best-effort: сбой -> None (не хуже прежнего).
    let (spk_refs, mut ref_texts, alt_refs) = if dirty_count == 0 {
        (std::collections::BTreeMap::new(), std::collections::BTreeMap::new(), std::collections::BTreeMap::new())
    } else if use_pack {
        if clone_slot_spks.is_empty() {
            (std::collections::BTreeMap::new(), std::collections::BTreeMap::new(), std::collections::BTreeMap::new())
        } else {
            // Клон-слоты "-" (#114): identity-рефы из вокала ТОЛЬКО для спикеров на клоне —
            // build_speaker_refs строит рефы по спикерам переданных сегментов, фильтруем их.
            let segs_clone: Vec<(usize, &dub_core::Segment)> = segs
                .iter()
                .filter(|(_, s)| clone_slot_spks.contains(s.speaker.as_deref().unwrap_or("0")))
                .cloned()
                .collect();
            let mut asr = crate::models::build_engine(&paths.asr);
            build_speaker_refs(&segs_clone, &vocals16, wd, paths.ref_secs, asr.as_mut(), progress)?
        }
    } else {
        // Скоринг кандидатов + REF-QC (транскрипт каждого кандидата сверяется с его src_text,
        // брак -> следующий) — ref_text выставляется ВНУТРИ по фактически услышанному.
        let mut asr = crate::models::build_engine(&paths.asr);
        build_speaker_refs(&segs, &vocals16, wd, paths.ref_secs, asr.as_mut(), progress)?
    };
    if dirty_count > 0 && use_pack {
        // Реф-транскрипция выбранным движком (Parakeet/Whisper), а НЕ захардкоженным Parakeet — иначе у
        // Whisper-only юзера (без Parakeet-модели) ref_text молча не считался бы. build_engine сам решает.
        let mut asr = crate::models::build_engine(&paths.asr);
        fill_ref_texts(asr.as_mut(), &pack_refs, &mut ref_texts);
    }
    let first_ref = pack_refs
        .values()
        .next()
        .cloned()
        .or_else(|| spk_refs.values().next().cloned())
        .unwrap_or_else(|| wd.join("ref.wav"));
    let ref_of = |s: &dub_core::Segment| -> PathBuf {
        let key = s.speaker.as_deref().unwrap_or("0");
        if use_pack {
            // клон-слот "-" (#114): спикер вне pack_refs берёт свой identity-реф из вокала (spk_refs).
            return pack_refs
                .get(key)
                .or_else(|| spk_refs.get(key))
                .cloned()
                .unwrap_or_else(|| first_ref.clone());
        }
        s.speaker
            .as_deref()
            .and_then(|k| spk_refs.get(k))
            .cloned()
            .unwrap_or_else(|| first_ref.clone())
    };
    let reftext_of = |s: &dub_core::Segment| -> Option<String> {
        let key = s.speaker.as_deref().unwrap_or("0");
        if let Some(t) = ref_texts.get(key) {
            return Some(t.clone());
        }
        // у спикера есть СВОЁ реф-аудио, но текста нет -> None (чужой текст к чужому аудио хуже, чем без
        // текста). Спикер без своего аудио (fit -> first_ref) берёт текст первого спикера (совпадает с ним).
        let has_own = if use_pack {
            pack_refs.contains_key(key) || spk_refs.contains_key(key) // клон-слот "-" тоже «своё» аудио
        } else {
            spk_refs.contains_key(key)
        };
        if has_own {
            None
        } else {
            ref_texts.values().next().cloned()
        }
    };

    // Per-segment ЭМОЦ-реф (BORROWINGS #2 «локальный эмоц-реф» / идея «скользящее окно для голоса»):
    // клон наследует крик/шёпот/плач оригинала В ЭТОТ момент, если реф взять из САМОГО сегмента, а не из
    // одного ровного identity-клипа на весь фильм. Гейт: (1) clone-режим (пак — фикс-голос юзера, эмоцию
    // источника не переносим); (2) реплика ЧИСТАЯ (нет оверлапа чужого спикера -> в реф не попадёт чужой
    // голос); (3) длина ≥REF_MIN_AFTER_TRIM (Higgs нужен минимум тембра). Иначе -> стабильный identity-реф
    // спикера (ref_of). Файл — свой на сегмент (по id), не конфликтует с seg_*.wav дубляжа. ref_text для
    // эмоц-рефа = src_text ЭТОГО же сегмента (совпадает с аудио по построению, перетранскрипция не нужна).
    // Порт коротких/1-спикер путей неизменен: при паке и на грязных/коротких репликах ведём себя как раньше.
    let emo_ref_on = crate::models::load_selection(&paths.models_root)
        .get("emo_ref_on")
        .and_then(|v| v.as_str())
        .map(|v| v != "0")
        .unwrap_or(true);
    let emo_enabled = emo_ref_on;
    let emo_ref_of = |s: &dub_core::Segment, sid: &str| -> Option<PathBuf> {
        if !emo_enabled || dirty_count == 0 {
            return None;
        }
        if use_pack {
            return None; // пак — фикс-голос юзера, эмоцию источника не переносим
        }
        let key = s.speaker.as_deref().unwrap_or("0");
        if (s.end - s.start) < 1.0 {
            return None; // слишком коротко (< 1.0с) для отдельного рефа
        }
        if !seg_is_clean(s, key, &segs) {
            return None; // оверлап чужого спикера -> не чистый эмоц-реф
        }
        let out = wd.join(format!("emoref_{sid}.wav"));
        // кап длины сверху ref_secs (не раздувать prefill-граф Higgs), как для identity-рефа.
        let cap = paths.ref_secs.min(REF_IDEAL_HI).max(1.0);
        let end = s.end.min(s.start + cap);
        match media::trim(&vocals16, &out, s.start, end.max(s.start + 0.05), 16_000) {
            Ok(()) => Some(out),
            Err(_) => None, // сбой обрезки -> тихо на identity-реф
        }
    };

    // 4) TTS каждый сегмент через Higgs (audiocpp).
    // Облачный TTS (OpenRouter) вместо локального Higgs: тяжёлую DLL + модель НЕ грузим вовсе — в этом и
    // смысл (снять самую тяжёлую часть). engine=None; синтез идёт по облачной ветке ниже.
    let cloud_tts_on = crate::models::openrouter_stage_on(&paths.models_root, "tts");
    let engine: Option<Arc<AudiocppEngine>> = if dirty_count == 0 {
        None
    } else if cloud_tts_on {
        emit(progress, "tts", "TTS через облако (OpenRouter) — локальный Higgs не загружаем");
        None
    } else {
        let key = EngineKey {
            dll: paths.higgs_dll.clone(),
            model_root: paths.higgs_model_root.clone(),
            backend: paths.higgs_backend.clone(),
            device: paths.higgs_device,
            threads: paths.higgs_threads,
            quant: paths.higgs_quant.clone(),
        };
        let mut cache = paths.tts_cache.lock().unwrap();
        if let Some((ref cached_key, ref eng)) = *cache {
            if cached_key == &key {
                emit(progress, "tts", "Higgs TTS: используем уже загруженную модель из кэша");
                Some(eng.clone())
            } else {
                emit(progress, "tts", "Higgs TTS: параметры изменились, выгружаем старую модель...");
                *cache = None; // выгружаем старую
                let e = Arc::new(
                    AudiocppEngine::load(&paths.higgs_dll).map_err(|e| format!("загрузка Higgs DLL: {e}"))?,
                );
                emit(progress, "tts", "Higgs TTS: загружаем новую модель...");
                e.load_model(
                    &paths.higgs_model_root,
                    &paths.higgs_backend,
                    paths.higgs_device,
                    paths.higgs_threads,
                    Some(paths.higgs_quant.as_str()),
                )
                .map_err(|e| format!("Higgs load_model: {e}"))?;
                *cache = Some((key, e.clone()));
                Some(e)
            }
        } else {
            emit(progress, "tts", "Higgs TTS: загружаем модель в память/VRAM...");
            let e = Arc::new(
                AudiocppEngine::load(&paths.higgs_dll).map_err(|e| format!("загрузка Higgs DLL: {e}"))?,
            );
            e.load_model(
                &paths.higgs_model_root,
                &paths.higgs_backend,
                paths.higgs_device,
                paths.higgs_threads,
                Some(paths.higgs_quant.as_str()),
            )
            .map_err(|e| format!("Higgs load_model: {e}"))?;
            *cache = Some((key, e.clone()));
            Some(e)
        }
    };

    // Автокастинг облачных голосов по полу спикера: мужскому спикеру — мужской голос, женскому — женский,
    // разным спикерам — разные. Пол — F0-замер (как в кастинге), голоса модели — динамически из API, пол
    // голоса — из спеки провайдера. Только в облачном режиме (локальный Higgs клонирует реальные рефы).
    // Пусто -> облачный TTS уйдёт на дефолтный голос настроек (or_tts_voice).
    let cloud_voice_map: std::collections::HashMap<String, String> = if dirty_count > 0 && cloud_tts_on {
        let mut m: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        // (1) РУЧНОЙ выбор голосов из кастинга: proj.audio.voice — CSV, позиционный по отсортированным
        // спикерам (как локальный пак-путь выше). В облачном режиме имена = ОБЛАЧНЫЕ голоса (casting_apply
        // их не валидирует против локальных voices/). "-"/пусто -> не задан, добьёт автокаст.
        if proj.audio.voice.mode == "voice" {
            let names: Vec<&str> =
                proj.audio.voice.name.as_deref().unwrap_or("").split(',').map(|s| s.trim()).collect();
            let sorted: Vec<String> = proj
                .segments
                .iter()
                .map(|s| s.speaker.clone().unwrap_or_else(|| "0".to_string()))
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
            for (i, spk) in sorted.iter().enumerate() {
                if let Some(nm) = names.get(i).copied() {
                    if !nm.is_empty() && nm != crate::voice_slots::CLONE_SLOT {
                        m.insert(spk.clone(), nm.to_string());
                    }
                }
            }
        }
        // (2) АВТОКАСТ по полу спикера добивает тех, кому голос вручную не назначен.
        if crate::models::openrouter_autocast(&paths.models_root) {
            let mut spk_ids: Vec<String> = proj
                .segments
                .iter()
                .map(|s| s.speaker.clone().unwrap_or_else(|| "0".into()))
                .collect();
            spk_ids.sort();
            spk_ids.dedup();
            let genders = crate::casting::speaker_genders_wd(&paths.work_dir, proj, &spk_ids);
            for (spk, v) in crate::cloud_voices::assign(&paths.models_root, &genders, &proj.tgt_lang) {
                m.entry(spk).or_insert(v);
            }
        }
        if !m.is_empty() {
            let mut desc: Vec<String> = m.iter().map(|(k, v)| format!("{k}→{v}")).collect();
            desc.sort();
            emit(progress, "tts", &format!("облачные голоса по спикерам: {}", desc.join(", ")));
        }
        m
    } else {
        std::collections::HashMap::new()
    };

    // placed = [(at, wav_path, dur)]. cursor-aware fit (как в питоне). dur мерится ЗДЕСЬ (в цикле
    // укладки) и хранится рядом — дакинг-блоки строятся из него БЕЗ повторного ffprobe (перф [22]).
    // Имя seg-файла и слот next.start — по индексу fi в ПОЛНОМ списке proj.segments.
    let mut placed: Vec<(f64, PathBuf, f64)> = Vec::with_capacity(segs.len());
    let mut cursor = 0.0f64;
    let n_all = proj.segments.len();
    // Анти-артефактные счётчики на весь ролик: consec — проблемные фразы ПОДРЯД (сброс на чистой);
    // retry_budget — суммарный лимит ретраев по длине ролика (лестница из 5 попыток длиннее старой).
    let mut consec = 0usize;
    let mut total_retries = 0usize;
    let retry_budget = ((total / 10.0).ceil() as usize).max(20);
    // Альтернативные рефы спикеров (ступени 4-5 лестницы ретраев) — из того же скоринга/REF-QC,
    // что и main-рефы (alt_refs построены выше в build_speaker_refs).
    // QC-список синтезированных в этом прогоне фраз: (fi, индекс в placed, raw-wav, tgt-текст, спикер,
    // room слота, путь fit-файла, ref_wav, ref_text) — после цикла сверяем транскрипцией и пересинтезируем несовпавшие.
    let mut qc_list: Vec<(usize, usize, PathBuf, String, String, f64, PathBuf, PathBuf, Option<String>)> = Vec::new();
    // Телеметрия укладки (#107): сколько сегментов пришлось растягивать выше капа (rate>1.25) и общий
    // счётчик уложенных — для итоговой доли «слишком быстрого текста».
    let mut fit_total = 0usize;
    let mut fit_over_cap = 0usize;
    let mut drift_escalations = 0usize; // сегменты, где кап atempo эскалирован для догона синка (#116)
    // Multi-take: генерировать 3 дубля и выбирать лучший по близости к target-длительности.
    let multitake_on = crate::models::load_selection(&paths.models_root)
        .get("multitake")
        .and_then(|v| v.as_str())
        .map(|v| v == "1")
        .unwrap_or(false);
    // Pause Squeeze: умное сжатие межсловных пауз перед ускорением для сохранения естественного тембра.
    let pause_squeeze_on = crate::models::load_selection(&paths.models_root)
        .get("pause_squeeze_on")
        .and_then(|v| v.as_str())
        .map(|v| v != "0")
        .unwrap_or(true);
    // Speech Rate: динамическая адаптация темпа генерации нейросети под длину текста/слота.
    let speech_rate_on = crate::models::load_selection(&paths.models_root)
        .get("speech_rate_on")
        .and_then(|v| v.as_str())
        .map(|v| v != "0")
        .unwrap_or(true);
    // UN Voice-Over Lead-in: вступление диктора с задержкой 0.6с после оригинала (правило «Золотой секунды»).
    let vo_lead_in = crate::models::load_selection(&paths.models_root)
        .get("vo_lead_in")
        .and_then(|v| v.as_str())
        .map(|v| v != "0")
        .unwrap_or(true);
    if voiceover && vo_lead_in {
        emit(progress, "mix", "закадр: UN Lead-in активен (оригинал звучит первым +0.6с)");
    }
    // Scene Spatial Reverb: согласование пространственной акустики (Early Reflections).
    let dub_reverb_match = crate::models::load_selection(&paths.models_root)
        .get("dub_reverb_match")
        .and_then(|v| v.as_str())
        .map(|v| v != "0")
        .unwrap_or(true);
    // Ручной контроль сэмплинга (Temperature): дефолт false (выкл -> заводские 0.30)
    let voice_manual_ctrl = crate::models::load_selection(&paths.models_root)
        .get("voice_manual_ctrl")
        .and_then(|v| v.as_str())
        .map(|v| v == "1")
        .unwrap_or(false);

    // Настраиваемая температура / стабильность голоса: дефолт 0.20 (0.08..0.45)
    let user_voice_temp: f64 = if voice_manual_ctrl {
        crate::models::load_selection(&paths.models_root)
            .get("voice_temp")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse::<f64>().ok())
            .unwrap_or(0.20)
            .clamp(0.08, 0.45)
    } else {
        0.30
    };
    // ПАРАЛЛЕЛЬНЫЙ ПРЕ-СИНТЕЗ облачного TTS: OpenRouter держит десятки конкурентных запросов, поэтому все
    // сегменты к синтезу гоним в N потоков (настройка or_concurrency) ДО последовательной укладки — она
    // потом просто подхватит уже готовые seg-файлы (network-latency больше не по одному). Провал сегмента ->
    // файл отсутствует -> цикл ниже синтезирует его сам (ретрай/фолбэк).
    if cloud_tts_on {
        let conc = crate::models::openrouter_concurrency(&paths.models_root);
        let mut jobs: Vec<(PathBuf, String, String)> = Vec::new();
        for &(fi, s) in segs.iter() {
            if seg_keep(s) {
                continue;
            }
            let tgt = s.tgt_text.trim();
            if tgt.is_empty() {
                continue;
            }
            let sid: String = s.id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
            let sid = if sid.is_empty() { format!("i{fi}") } else { sid };
            let raw = wd.join(format!("seg_{sid}.wav"));
            if !((regen_dub && s.dirty) || !raw.is_file()) {
                continue; // уже в кэше
            }
            let voice = cloud_voice_map.get(s.speaker.as_deref().unwrap_or("0")).cloned().unwrap_or_default();
            jobs.push((raw, tgt.to_string(), voice));
        }
        if jobs.len() > 1 && conc > 1 {
            emit(progress, "tts", &format!("облачный TTS: {} сегментов в {} параллельных потоков", jobs.len(), conc));
            let ok = crate::cloud_tts::synth_batch(&paths.models_root, jobs, conc);
            emit(progress, "tts", &format!("облачный TTS: пре-синтез готов ({ok} сегментов)"));
        }
    }
    let dirty_total = dirty_count;
    let mut synth_counter = 0usize;
    for &(fi, s) in segs.iter() {
        // Кэш-файл сегмента — ПО ЕГО ID, не по индексу fi. Кэш переиспользуется между рендерами (не-dirty
        // сегменты не ре-синтезируются). При индекс-имени удаление/перестановка сегмента сдвигает индексы —
        // и чистый сегмент подхватил бы seg_{fi}.wav ПРЕДЫДУЩЕГО жильца индекса => чужая речь/длительность =
        // ДРИФТ дубляжа (регресс кэша порта; питон синтезил заново каждый рендер). ID стабилен -> кэш привязан
        // к контенту. Слот next.start (nxt) остаётся по индексу — это про таймлайн-позицию, не про кэш.
        let sid: String = s.id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
        let sid = if sid.is_empty() { format!("i{fi}") } else { sid };
        let raw = wd.join(format!("seg_{sid}.wav"));
        // 'оставить оригинал': вырезаем ИСХОДНУЮ речь сюда, без TTS и без atempo-подгонки (порт _build_dub keep-ветки).
        // Режем СРАЗУ в 24к моно (питон media.trim(..., sr=24000)) — timeline кладёт по sr ПЕРВОГО файла (TTS=24к),
        // без ресемпла; 44.1к-вырез играл бы не на той скорости. Без промежуточного 16к (не терять ВЧ).
        if seg_keep(s) {
            media::trim(&vocals, &raw, s.start, s.end, 24_000)?;
            let at = s.start.max(cursor);
            let d = media::duration(&raw)?;
            cursor = at + d;
            placed.push((at, raw, d));
            continue;
        }
        let tgt = s.tgt_text.trim();

        // Референс голоса для сегмента: custom_ref (голос из пака или донор) -> emo_ref -> identity-реф
        let custom_ref: Option<(PathBuf, Option<String>)> = if let Some(v) = s.voice.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            if let Some(donor_spec) = v.strip_prefix("donor:").or_else(|| v.strip_prefix("clone:")) {
                // Донорский клон из конкретного сегмента (по ID или по индексу #N)
                let donor_seg = proj.segments.iter().find(|ds| ds.id == donor_spec).or_else(|| {
                    donor_spec.parse::<usize>().ok().and_then(|idx| if idx > 0 { proj.segments.get(idx - 1) } else { proj.segments.get(0) })
                });
                if let Some(ds) = donor_seg {
                    let out = wd.join(format!("ref_donor_{sid}.wav"));
                    let cap = paths.ref_secs.min(REF_IDEAL_HI).max(1.0);
                    let end = ds.end.min(ds.start + cap);
                    if media::trim(&vocals16, &out, ds.start, end.max(ds.start + 0.05), 16_000).is_ok() {
                        let t = ds.src_text.trim();
                        Some((out, if t.is_empty() { None } else { Some(t.to_string()) }))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                // Кастомный голос из библиотеки (voices/<name>.wav/mp3)
                let voice_file = ["wav", "mp3"]
                    .iter()
                    .map(|e| paths.voices_dir.join(format!("{v}.{e}")))
                    .find(|p| p.is_file());
                if let Some(vf) = voice_file {
                    let out = wd.join(format!("ref_voice_{sid}.wav"));
                    // Копируем во временный ASCII-файл, чтобы MinGW FFmpeg под Windows гарантированно открыл путь без сбоев кодировки
                    let ext = vf.extension().and_then(|x| x.to_str()).unwrap_or("wav");
                    let temp_in = wd.join(format!("temp_v_{sid}.{ext}"));
                    let _ = std::fs::copy(&vf, &temp_in);
                    let in_p = if temp_in.is_file() { &temp_in } else { &vf };
                    let trim_ok = media::trim(in_p, &out, 0.0, paths.ref_secs, 16_000).is_ok();
                    let _ = std::fs::remove_file(&temp_in);
                    if trim_ok && out.is_file() {
                        // Подтягиваем текст расшифровки сэмпла .txt, если он есть в каталоге (Higgs клонирует чище)
                        let txt_file = paths.voices_dir.join(format!("{v}.txt"));
                        let txt_content = std::fs::read_to_string(&txt_file).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                        Some((out, txt_content))
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
        } else {
            None
        };

        // Реф: если задан custom_ref -> используем его; иначе пробуем эмоц-реф, затем identity-реф
        let emo_ref = if custom_ref.is_some() { None } else { emo_ref_of(s, &sid) };
        let (ref_wav, ref_text): (PathBuf, Option<String>) = if let Some(cr) = custom_ref {
            cr
        } else {
            match &emo_ref {
                Some(er) => {
                    let cap = paths.ref_secs.min(REF_IDEAL_HI).max(REF_MIN_AFTER_TRIM);
                    let capped = (s.end - s.start) > cap + 0.05;
                    let t = s.src_text.trim();
                    (er.clone(), if capped || t.is_empty() { None } else { Some(t.to_string()) })
                }
                None => (ref_of(s), reftext_of(s)),
            }
        };

        let spk_seed_base: u64 = {
            let n: u64 = s.speaker.as_deref().unwrap_or("0").parse::<u64>().unwrap_or_else(|_| {
                s.speaker.as_deref().unwrap_or("0").bytes().fold(42u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64))
            });
            (n + 1) * 77777 + 42
        };

        // Синтез ТОЛЬКО если сегмент dirty (правился текст/спикер/голос) ИЛИ нет кэша. Реф-клипы
        // пересобираются каждый рендер, поэтому mtime-сравнение с рефом («stale_ref») ошибочно
        // помечало ВЕСЬ кэш устаревшим на каждом рендере → экспорт ре-роллил уже одобренную озвучку
        // («скидывалось»). Смена голоса и так метит все сегменты dirty (op_recast/op_segment), так что
        // dirty-флага достаточно: не-dirty сегменты переиспользуют свой seg_XXX.wav между рендерами.
        let need_synth = (regen_dub && s.dirty) || !raw.is_file();
        // Полный провал лестницы на КОРОТКОМ сегменте -> оригинальная реплика вместо артефакта
        // (объявлен на уровне итерации: ниже гейтит и ASR-QC этого сегмента).
        let mut kept_original = false;
        if need_synth {
            synth_counter += 1;
            emit(progress, "tts", &format!("озвучка фраз: {synth_counter} из {dirty_total} (фраза #{})", fi + 1));
            if cloud_tts_on {
            // Облачный TTS: wav-байты OpenRouter пишем ПРЯМО в seg-файл (без декода/перекодировки).
            // Голос — из автокастинга по полу спикера (пусто -> дефолт настроек). Провал -> оригинал
            // сегмента (ноль немых мест), как локальный фолбэк.
            let cv = cloud_voice_map
                .get(s.speaker.as_deref().unwrap_or("0"))
                .map(String::as_str)
                .unwrap_or("");
            match crate::cloud_tts::synth_audio(&paths.models_root, tgt, cv) {
                Ok(wav) => {
                    std::fs::write(&raw, &wav).map_err(|e| format!("запись облачного seg_{sid}: {e}"))?;
                }
                Err(e) => {
                    emit(progress, "tts", &format!("⚠ фраза #{} ({sid}): облачный TTS не удался ({e}) — оригинал", fi + 1));
                    media::trim(&vocals, &raw, s.start, s.end, 24_000)?;
                    kept_original = true;
                }
            }
            } else {
            // Анти-артефактный ретрай: иногда Higgs выдаёт «гудение» (непрерывный гул без речи). Детект
            // in-memory (synth_defect) по сэмплам; перегенерируем — синтез стохастичен, повтор обычно
            // даёт валидный дубль. Вариативность ретрая (BORROWINGS #17 + ревью-находка D): у Higgs
            // подтверждён рычаг сэмплинга `temperature` (PROSODY_FINDINGS §6.1); поле `seed` НЕ подтверждено
            // (DLL прекомпилена, C++ нет). Если варьировать ТОЛЬКО seed и DLL его игнорит — все 3 попытки
            // битово идентичны → тот же гул → жёсткий abort. Поэтому на ПОВТОРНОЙ попытке бампаем
            // temperature (0.9→1.2) — выше рандом сэмплинга → выход из вырожденного гула; seed добавляем
            // бонусом. Первая попытка — пустой opts = дефолт движка (питон-паритет).
            let spk_key = s.speaker.clone().unwrap_or_else(|| "0".into());
            let alt = alt_refs.get(&spk_key);
            let tgt_chars = tgt.chars().filter(|c| c.is_alphanumeric()).count();
            // КАП ТОКЕНОВ ПО ДЛИТЕЛЬНОСТИ (ENGINES_FINDINGS §1.1, issue #151): в движке НЕТ авто-капа —
            // без него короткая фраза может уйти в 40с гула до max_new_tokens=2048. Кодек 25-75 ток/с
            // (версии разнятся) — берём консервативные 75: cap = ceil(dur×75×1.5)+32, floor 64.
            // Таргет-длительность = длительность оригинальной реплики (у дубля тот же слот).
            let expected_dur = (s.end - s.start).max(0.6);
            let tok_cap: u32 = (((expected_dur * 75.0 * 1.5).ceil() as u32) + 32).clamp(64, 2048);
            
            // Динамическая адаптация темпа (Speech Rate): если текст плотный (>14 знаков/сек), понижаем
            // температуру и зажимаем повторы, выговаривая текст собранно; если редкий — повышаем.
            let rate_ratio = if speech_rate_on && expected_dur > 0.1 && tgt_chars > 0 {
                let ideal_dur = (tgt_chars as f64) / 14.0;
                (ideal_dur / expected_dur).clamp(0.70, 1.40)
            } else {
                1.0
            };
            let base_temp = if rate_ratio > 1.12 {
                (user_voice_temp * 0.85).clamp(0.08, 0.40)
            } else if rate_ratio < 0.88 {
                (user_voice_temp * 1.15).clamp(0.08, 0.40)
            } else {
                user_voice_temp
            };
            let base_ras_rep = if rate_ratio > 1.12 { ",\"ras_win_max_num_repeat\":1" } else { "" };

            let mut attempt = 0usize;
            let mut retried = false;
            // Лучшая из ДЕФЕКТНЫХ попыток (по размаху) — на случай полного провала лестницы.
            let mut best_bad: Option<(Vec<f32>, i32, f64)> = None;
            let (samples, sr) = loop {
                // Лестница (ENGINES_FINDINGS §1.3/1.8: на КВАНТЕ temperature ВНИЗ 0.3→0.1, НЕ вверх;
                // офиц. voice-clone примеры = 0.3): 0 — temp base + кап + RAS7; 1 — жёстче RAS (repeat=1),
                // temp пониже, seed; 2 — temp минимальный, top_p 0.9, seed; 3-4 — АЛЬТ-РЕФ спикера.
                let use_alt = attempt >= 3 && alt.is_some();
                let (rw, rt): (&PathBuf, Option<&str>) = if use_alt {
                    let (p, t) = alt.unwrap();
                    (p, t.as_deref())
                } else {
                    (&ref_wav, ref_text.as_deref())
                };
                let seed = spk_seed_base + attempt as u64;
                let opts = match attempt {
                    0 | 3 => format!(
                        "{{\"temperature\":{base_temp:.2},\"top_p\":0.95,\"top_k\":50,\"max_new_tokens\":{tok_cap},\"ras_win_len\":7{base_ras_rep},\"return_audio_in_tokens\":true,\"seed\":{seed}}}"
                    ),
                    1 => format!(
                        "{{\"temperature\":{:.2},\"top_p\":0.95,\"top_k\":50,\"max_new_tokens\":{tok_cap},\"ras_win_len\":7,\"ras_win_max_num_repeat\":1,\"return_audio_in_tokens\":true,\"seed\":{seed}}}",
                        (base_temp * 0.85).clamp(0.08, 0.35)
                    ),
                    2 => format!(
                        "{{\"temperature\":{:.2},\"top_p\":0.90,\"top_k\":50,\"max_new_tokens\":{tok_cap},\"ras_win_len\":7,\"return_audio_in_tokens\":true,\"seed\":{seed}}}",
                        (base_temp * 0.65).clamp(0.08, 0.25)
                    ),
                    _ => format!(
                        "{{\"temperature\":{:.2},\"top_p\":0.90,\"top_k\":50,\"max_new_tokens\":{tok_cap},\"ras_win_len\":7,\"ras_win_max_num_repeat\":1,\"return_audio_in_tokens\":true,\"seed\":{seed}}}",
                        (base_temp * 0.75).clamp(0.08, 0.30)
                    ),
                };
                attempt += 1;
                // Таймаут ~8× длины слота (флор 45с): нормальный синтез быстрее реалтайма, зависание —
                // минуты, так что порог чисто разделяет. Ошибка/таймаут -> как дефект (ретрай стохастику
                // обычно лечит); исчерпали попытки -> ОРИГИНАЛ (сегмент дороже потерять, чем зависший рендер).
                let vc_to = Duration::from_secs((((s.end - s.start) * 8.0).ceil() as u64).max(45));
                let eng = engine.as_ref().expect("локальный Higgs (не облако)");
                let (samples, sr) = match voice_clone_guarded(eng, tgt, &rw.to_string_lossy(), rt, &opts, vc_to) {
                    Ok(v) => v,
                    Err(e) if e.starts_with(ENGINE_STUCK) => {
                        *paths.tts_cache.lock().unwrap() = None; // инвалидировать кэш при зависании
                        return Err(e);
                    }
                    Err(e) => {
                        retried = true;
                        total_retries += 1;
                        if attempt >= MAX_TTS_ATTEMPTS {
                            if let Some((sm, r, rng)) = best_bad.take() {
                                emit(progress, "tts", &format!(
                                    "⚠ фраза #{}: {MAX_TTS_ATTEMPTS} сбоев синтеза ({e}) — взята сгенерированная озвучка (размах {rng:.0} дБ)",
                                    fi + 1
                                ));
                                break (sm, r);
                            } else {
                                media::trim(&vocals, &raw, s.start, s.end, 24_000)?;
                                kept_original = true;
                                emit(progress, "tts", &format!(
                                    "⚠ фраза #{}: {MAX_TTS_ATTEMPTS} сбоев/таймаутов синтеза ({e}) — оставлена оригинальная реплика",
                                    fi + 1
                                ));
                                break (Vec::new(), 24_000);
                            }
                        }
                        emit(progress, "tts", &format!("озвучка фраз: {synth_counter} из {dirty_total} (фраза #{}) — {e} (попытка {}/{})", fi + 1, attempt + 1, MAX_TTS_ATTEMPTS));
                        std::thread::sleep(Duration::from_millis(1000));
                        continue;
                    }
                };
                match synth_defect(&samples, sr, tgt_chars) {
                    None => break (samples, sr), // дефектов не видно — берём
                    Some(kind) => {
                        let rng = hum_range_db(&samples, sr);
                        if best_bad.as_ref().map_or(true, |(_, _, b)| rng > *b) {
                            best_bad = Some((samples, sr, rng));
                        }
                        if attempt >= MAX_TTS_ATTEMPTS {
                            // Всегда используем сгенерированный TTS-звук (даже для коротких фраз / выкриков / хоров),
                            // избегая сброса на оригинальный вокал.
                            if let Some((sm, r, rng)) = best_bad.take() {
                                emit(progress, "tts", &format!(
                                    "⚠ фраза #{}: все {MAX_TTS_ATTEMPTS} попыток с дефектом ({kind}) — взята сгенерированная озвучка (размах {rng:.0} дБ)",
                                    fi + 1
                                ));
                                break (sm, r);
                            } else {
                                media::trim(&vocals, &raw, s.start, s.end, 24_000)?;
                                kept_original = true;
                                emit(progress, "tts", &format!(
                                    "⚠ фраза #{}: {MAX_TTS_ATTEMPTS} попыток без звука — подставлен оригинал",
                                    fi + 1
                                ));
                                break (Vec::new(), sr);
                            }
                        }
                        retried = true;
                        total_retries += 1;
                        let via = if attempt >= 3 { "альт-реф" } else { "temp-бамп" };
                        emit(progress, "tts", &format!("озвучка фраз: {synth_counter} из {dirty_total} (фраза #{}) — дефект ({kind}), регенерация ({via} {}/{})", fi + 1, attempt + 1, MAX_TTS_ATTEMPTS));
                    }
                }
            };
            if !kept_original {
                let wav = AudiocppEngine::encode_wav(&samples, sr, 1);
                std::fs::write(&raw, &wav).map_err(|e| format!("запись seg_{sid}: {e}"))?;
            }
            // Много ретраев подряд/суммарно = систем. проблема (стенд/VRAM или реф-клипы) → стоп с ошибкой.
            if retried {
                consec += 1;
                if consec > CONSECUTIVE_ABORT || total_retries > retry_budget {
                    return Err(format!(
                        "TTS: слишком много артефактов-гудения (подряд {consec}, всего ретраев {total_retries}) — регенерация не помогает. Вероятно проблема со стендом (модель/VRAM) или с реф-клипами голосов. Остановлено на фразе #{}.",
                        fi + 1
                    ));
                }
            } else {
                consec = 0; // чистая фраза сбрасывает серию
            }
            } // конец локальной (Higgs) ветки — при облаке wav уже записан выше
        }
        // слот: от текущего onset до старта СЛЕДУЮЩЕГО сегмента ПО ИНДЕКСУ (fi+1) полного списка /
        // конца видео (питон nxt = segs[i+1].start if i+1<len else total).
        // UN Voice-Over Lead-in: вступление диктора после оригинала (0.6с для длинных фраз, 0.3с для средних)
        let lead_in = if voiceover && vo_lead_in {
            let slot_len = (s.end - s.start).max(0.1);
            if slot_len >= 1.5 {
                0.60
            } else if slot_len >= 0.8 {
                0.30
            } else {
                0.0
            }
        } else {
            0.0
        };
        let onset = s.start + lead_in;
        let at = onset.max(cursor);
        let nxt = if fi + 1 < n_all { proj.segments[fi + 1].start } else { total };
        let room = (nxt - at).max(0.3);
        let fitp = wd.join(format!("seg_{sid}_fit.wav"));

        // ── MULTI-TAKE: адаптивный отбор дублей ──
        if multitake_on && need_synth && !kept_original && !cloud_tts_on && engine.is_some() {
            let raw_dur = media::duration(&raw).unwrap_or(0.0);
            let target = if speech_rate_on { ((s.end - s.start) - lead_in).max(0.3) } else { room };
            let tgt = s.tgt_text.trim();
            let mut best_path = raw.clone();
            let mut best_score = (raw_dur - target).abs();

            // Проверяем относительную погрешность первого дубля: delta = |dur - target| / target
            let delta = if target > 0.0 { best_score / target } else { 0.0 };
            let take1_ok = if delta <= 0.10 && raw_dur > 0.0 {
                // Первый дубль уложился в 10% от слота — проверяем на отсутствие дефектов синтеза
                if let Ok((samples, sr)) = crate::wavio::read_mono_f32(&raw) {
                    synth_defect(&samples, sr as i32, tgt.chars().filter(|c| c.is_alphanumeric()).count()).is_none()
                } else {
                    false
                }
            } else {
                false
            };

            if take1_ok {
                emit(
                    progress,
                    "tts",
                    &format!("фраза #{}: первый дубль идеален (отклонение {:.0}%) — пропуск доп. дублей", fi + 1, delta * 100.0),
                );
            } else {
                let _spk_key = s.speaker.clone().unwrap_or_else(|| "0".into());
                // Multi-take обязан наследовать индивидуальный голос/донор сегмента (ref_wav / ref_text)
                let ref_wav_mt = ref_wav.clone();
                let ref_text_mt = ref_text.clone();
                let tok_cap: u32 = ((((s.end - s.start).max(0.6) * 75.0 * 1.5).ceil() as u32) + 32).clamp(64, 2048);
                for take_i in 1..=2u64 {
                    let take_path = wd.join(format!("seg_{sid}_take{take_i}.wav"));
                    let seed = spk_seed_base + take_i * 100 + 77;
                    let temp = if take_i == 1 { (user_voice_temp * 0.90).clamp(0.08, 0.40) } else { (user_voice_temp * 1.15).clamp(0.10, 0.45) };
                    let opts = format!(
                        "{{\"temperature\":{temp:.2},\"top_p\":0.95,\"top_k\":50,\"max_new_tokens\":{tok_cap},\"ras_win_len\":7,\"return_audio_in_tokens\":true,\"seed\":{seed}}}"
                    );
                    let rt_mt = ref_text_mt.as_deref();
                    let vc_to = Duration::from_secs((((s.end - s.start) * 8.0).ceil() as u64).max(45));
                    let eng = engine.as_ref().unwrap();
                    match voice_clone_guarded(eng, tgt, &ref_wav_mt.to_string_lossy(), rt_mt, &opts, vc_to) {
                        Ok((samples, sr)) => {
                            if synth_defect(&samples, sr, tgt.chars().filter(|c| c.is_alphanumeric()).count()).is_none() {
                                let wav = AudiocppEngine::encode_wav(&samples, sr, 1);
                                let _ = std::fs::write(&take_path, &wav);
                                if let Ok(td) = media::duration(&take_path) {
                                    let score = (td - target).abs();
                                    if score < best_score {
                                        best_score = score;
                                        best_path = take_path.clone();
                                    }
                                }
                            }
                        }
                        Err(e) if e.starts_with(ENGINE_STUCK) => {
                            *paths.tts_cache.lock().unwrap() = None; // инвалидировать кэш при зависании
                            return Err(e);
                        }
                        Err(_) => {
                            std::thread::sleep(Duration::from_millis(1000));
                        }
                    }
                }
                // Если лучший дубль — не первый, подменяем raw-файл
                if best_path != raw {
                    let _ = std::fs::copy(&best_path, &raw);
                    emit(progress, "tts", &format!("фраза #{}: multi-take — выбран дубль ближе к слоту ({best_score:.2}с отклонение)", fi + 1));
                }
            }
        }

        // Целевая длительность слота: при ВКЛЮЧЕННОМ «Динамическом темпе речи» берем ТОЧНЫЕ границы
        // данного субтитра с вычетом lead_in, чтобы фраза укладывалась ровно в свой прямоугольник.
        // При ВЫКЛЮЧЕННОМ — используем дефолтное поведение (room от старта до старта следующего + защитные кап-лимиты).
        let target_slot = if speech_rate_on {
            ((s.end - s.start) - lead_in).max(0.3)
        } else {
            room
        };

        // Кап этого сегмента: если контроль длительности выключен (qc_duration=0), даем свободу (10.0)
        let run_qc_duration = crate::models::load_selection(&paths.models_root)
            .get("qc_duration")
            .and_then(|v| v.as_str())
            .map(|v| v != "0")
            .unwrap_or(true);
        let seg_cap = if !run_qc_duration {
            10.0
        } else if target_slot < 1.5 {
            paths.max_stretch.max(1.30)
        } else {
            paths.max_stretch
        };
        // Дрейф-кап (#116, находка [4]): рассинхрон дороже темпа. Кап 1.25 при cursor-ripple копит сдвиг
        // на плотном диалоге — фразы всё позже. Если дубль уже отстал (cursor > s.start), эскалируем кап
        // до нужного, чтобы догнать слот (потолок 2.0), ценой временной спешки.
        let drift = (cursor - s.start).max(0.0);
        let raw_dur = media::duration(&raw).unwrap_or(0.0);
        let needed = if target_slot > 0.05 { raw_dur / target_slot } else { 1.0 };
        // При включенном тумблере — прямое ускорение атемпо под точный размер субтитра (до 4.0x).
        // При выключенном — дефолтные защитные ограничения (не быстрее 1.25x-1.30x, либо до 2.0x при дрейфе).
        let eff_cap = if speech_rate_on {
            seg_cap.max(needed).min(4.0)
        } else if drift > 0.6 {
            seg_cap.max(needed).min(2.0)
        } else {
            seg_cap
        };
        if drift > 0.6 && eff_cap > seg_cap {
            drift_escalations += 1;
        }
        // Телеметрия укладки (#107): needed>eff_cap -> дубль не влезает даже с (эскалированным) капом,
        // текст пойдёт быстрее нормы. raw_dur==0 (сбой duration) -> сегмент не считаем в статистику.
        if raw_dur > 0.0 {
            fit_total += 1;
            if needed > eff_cap {
                fit_over_cap += 1;
                emit(progress, "mix", &format!(
                    "сегмент {fi}: нужно растянуть x{needed:.2} (слот {target_slot:.2}с), кап x{eff_cap:.2} — текст быстрее нормы"
                ));
            }
        }
        let (fit, d) = fit_to_slot(&raw, target_slot, &fitp, eff_cap, pause_squeeze_on)?;
        cursor = at + d;
        placed.push((at, fit, d));
        // В QC — только реально синтезированное в этом прогоне (кэш уже проверялся в своём прогоне).
        // kept_original (оригинальная реплика вместо неспасаемого выкрика) НЕ сверяем: там исходный
        // язык, ASR-QC счёл бы его браком и пересинтезировал обратно в артефакт.
        // Higgs-QC (ASR-сверка + пересинтез) — только для локального движка; облачный TTS артефактов-гула
        // не даёт, а его валидация покрытия идёт отдельным гейтом.
        if !cloud_tts_on && need_synth && !kept_original && !s.tgt_text.trim().is_empty() {
            qc_list.push((
                fi,
                placed.len() - 1,
                raw.clone(),
                s.tgt_text.trim().to_string(),
                s.speaker.clone().unwrap_or_else(|| "0".into()),
                target_slot,
                fitp,
                ref_wav.clone(),
                ref_text.clone(),
            ));
        }
    }
    // Итоговая доля «слишком быстрого текста» (#107) + дрейф-эскалации (#116).
    if fit_total > 0 {
        let frac = 100.0 * fit_over_cap as f64 / fit_total as f64;
        let drift = if drift_escalations > 0 { format!(", догон синка на {drift_escalations}") } else { String::new() };
        emit(progress, "mix", &format!(
            "укладка: {fit_over_cap}/{fit_total} сегментов выше капа ({frac:.0}%){drift}"
        ));
    }

    // ── QC: ASR-верификация синтеза (выполняется только если qc_asr="1" в настройках) ──
    let run_qc_asr = crate::models::load_selection(&paths.models_root)
        .get("qc_asr")
        .and_then(|v| v.as_str())
        .map(|v| v == "1")
        .unwrap_or(false);
    if run_qc_asr && !qc_list.is_empty() {
        emit(progress, "tts", &format!("QC: сверка {} фраз транскрипцией", qc_list.len()));
        // `None` означает сбой пакетного ASR/отсутствующий JSON, а `Some("")` — честно
        // распознанную тишину. Первое нельзя считать браком TTS: иначе падение Whisper
        // пересинтезирует весь ролик до трёх раз. Второе по-прежнему идёт в qc_similarity.
        let (bad_idx, skipped) = {
            let mut bad_idx: Vec<usize> = Vec::new();
            let mut skipped = 0usize;
            let mut qc_asr = crate::models::build_engine(&paths.asr);
            let files: Vec<PathBuf> = qc_list.iter().map(|q| q.2.clone()).collect();
            let heard = qc_asr.transcribe_many(&files, &proj.tgt_lang);
            for (i, q) in qc_list.iter().enumerate() {
                // Междометия НЕ пропускаем: вой «О,»->«ОООО…» жил именно на них (QC-скан R5b);
                // ложные капризы ASR на коротких гасит префикс-режим qc_similarity (0.5 на пустом ASR).
                let Some(h) = heard.get(i).and_then(|x| x.as_deref()) else {
                    skipped += 1;
                    continue;
                };
                if qc_similarity(&q.3, h) < 0.35 {
                    bad_idx.push(i);
                }
            }
            (bad_idx, skipped)
        }; // qc_asr дропается здесь, освобождая VRAM перед циклом синтеза Higgs
        if skipped > 0 {
            emit(progress, "tts", &format!(
                "⚠ QC: ASR не вернул результат для {skipped} фраз — пересинтез по ним пропущен"
            ));
        }
        if !bad_idx.is_empty() {
            emit(progress, "tts", &format!("QC: {} фраз не совпали с переводом — пересинтез", bad_idx.len()));
            for &i in &bad_idx {
                let (fi, pidx, raw, tgtq, spk, room, fitp, seg_rw, seg_rt) = &qc_list[i];
                let s = &proj.segments[*fi];
                let has_custom_voice = s.voice.as_deref().map(str::trim).filter(|v| !v.is_empty()).is_some();
                let main_rw = if has_custom_voice { seg_rw.clone() } else { ref_of(s) };
                let main_rt = if has_custom_voice { seg_rt.clone() } else { reftext_of(s) };
                let alt = if has_custom_voice { None } else { alt_refs.get(spk) };
                let tgt_chars = tgtq.chars().filter(|c| c.is_alphanumeric()).count();
                // до 3 свежих попыток (низкая temperature по ENGINES_FINDINGS §1.3 + кап токенов §1.1):
                // альт-реф 0.3 → альт-реф 0.15+RAS1 → основной 0.10 с новым seed
                let e_dur = (s.end - s.start).max(0.6);
                let cap: u32 = (((e_dur * 75.0 * 1.5).ceil() as u32) + 32).clamp(64, 2048);
                let base = format!("\"top_k\":50,\"max_new_tokens\":{cap},\"ras_win_len\":7,\"return_audio_in_tokens\":true");
                let mut fixed = false;
                for (k, (rw, rt, opts)) in {
                    let mut plan: Vec<(&PathBuf, Option<&str>, String)> = Vec::new();
                    if let Some((ap, at_)) = alt {
                        plan.push((ap, at_.as_deref(), format!("{{\"temperature\":0.30,\"top_p\":0.95,{base}}}")));
                        plan.push((ap, at_.as_deref(), format!("{{\"temperature\":0.15,\"top_p\":0.90,\"ras_win_max_num_repeat\":1,{base},\"seed\":{}}}", (*fi as u64) * 1000 + 77)));
                    }
                    plan.push((&main_rw, main_rt.as_deref(), format!("{{\"temperature\":0.10,\"top_p\":0.90,{base},\"seed\":{}}}", (*fi as u64) * 1000 + 88)));
                    plan
                }
                .into_iter()
                .enumerate()
                {
                    let vc_to = Duration::from_secs((((s.end - s.start) * 8.0).ceil() as u64).max(45));
                    let (smp, r) = match voice_clone_guarded(engine.as_ref().expect("локальный Higgs (QC не для облака)"), tgtq, &rw.to_string_lossy(), rt, &opts, vc_to) {
                        Ok(v) => v,
                        Err(e) if e.starts_with(ENGINE_STUCK) => {
                            *paths.tts_cache.lock().unwrap() = None; // инвалидировать кэш при зависании
                            return Err(e);
                        }
                        Err(_) => {
                            std::thread::sleep(Duration::from_millis(1000));
                            continue;
                        }
                    };
                    if synth_defect(&smp, r, tgt_chars).is_some() {
                        continue;
                    }
                    let wav = AudiocppEngine::encode_wav(&smp, r, 1);
                    if std::fs::write(raw, &wav).is_ok() {
                        // пере-fit в тот же слот и подмена в placed (позиция at не меняется, длит. обновляем).
                        // Кап = потолок дрейфа (2.0): основной проход мог дрейф-капнуть этот сегмент выше
                        // seg_cap; пересинтез с seg_cap дал бы более ДЛИННЫЙ дубль и порвал синк (#116 [6]).
                        if let Ok((nf, nd)) = fit_to_slot(raw, *room, fitp, 2.0, pause_squeeze_on) {
                            placed[*pidx].1 = nf;
                            placed[*pidx].2 = nd;
                            fixed = true;
                            emit(progress, "tts", &format!("QC: фраза #{} пересинтезирована (попытка {})", fi + 1, k + 1));
                            break;
                        }
                    }
                }
                if !fixed {
                    emit(progress, "tts", &format!("⚠ QC: фраза #{} («{}») не удалось подтвердить — проверь фразу вручную", fi + 1, tgtq.chars().take(40).collect::<String>()));
                }
            }
            // финальная сверка пересинтезированных — честный отчёт в журнал
            let mut still = 0usize;
            let skipped_final = {
                let mut skipped_final = 0usize;
                let mut qc_asr = crate::models::build_engine(&paths.asr);
                let files2: Vec<PathBuf> = bad_idx.iter().map(|&i| qc_list[i].2.clone()).collect();
                let heard2 = qc_asr.transcribe_many(&files2, &proj.tgt_lang);
                for (j, &i) in bad_idx.iter().enumerate() {
                    let Some(h) = heard2.get(j).and_then(|x| x.as_deref()) else {
                        skipped_final += 1;
                        continue;
                    };
                    if qc_similarity(&qc_list[i].3, h) < 0.35 {
                        // Отключён сброс на оригинальное аудио. Сгенерированный TTS-звук ВСЕГДА остаётся
                        // на таймлайне, даже если QC (сверка через ASR) не подтвердил совпадение текста.
                        still += 1;
                        emit(progress, "tts", &format!("⚠ QC: фраза #{} не совпадает с текстом перевода — оставлена сгенерированная озвучка", qc_list[i].0 + 1));
                    }
                }
                skipped_final
            }; // qc_asr дропается здесь, освобождая VRAM
            if skipped_final > 0 {
                emit(progress, "tts", &format!(
                    "⚠ QC: повторный ASR не вернул результат для {skipped_final} пересинтезированных фраз"
                ));
            }
            let confirmed_fixed = bad_idx.len().saturating_sub(still + skipped_final);
            emit(progress, "tts", &format!(
                "QC итог: исправлено {confirmed_fixed}/{}, осталось помеченных {still}, не проверено {skipped_final}",
                bad_idx.len()
            ));
        } else {
            if skipped == 0 {
                emit(progress, "tts", "QC: все фразы подтверждены транскрипцией ✓");
            } else {
                emit(progress, "tts", &format!(
                    "QC: подтверждено {} фраз, {} пропущено из-за сбоя ASR",
                    qc_list.len() - skipped,
                    skipped
                ));
            }
        }
    }

    // 5) timeline -> dub_vocals.wav. Возвращает фактические спаны укладки.
    emit(progress, "mix", "укладка дубляжа на таймлайн");
    let dub_raw = wd.join("dub_vocals_raw.wav");
    let laid_spans = timeline(&placed, total, &dub_raw)?;
    let dub = wd.join("dub_vocals.wav");
    if dub_reverb_match {
        emit(progress, "mix", "пространственная акустика (Early Reflections)");
        if media::apply_spatial_reverb(&dub_raw, &dub).is_err() {
            let _ = std::fs::copy(&dub_raw, &dub);
        }
    } else {
        let _ = std::fs::copy(&dub_raw, &dub);
    }
    // Речевые блоки для дакинга (#106) — из ФАКТИЧЕСКИХ спанов timeline (единый источник: с учётом
    // cursor-ripple и QC-пересинтеза), а не из onset'ов placed.
    let speech_blocks = build_speech_blocks(&laid_spans);
    let dub = dub;
    let dub_dur = media::duration(&dub).unwrap_or(total);
    // Гарантия хронометража: если финальный хвост дорожки вышел за total_dur, аккуратно обрезаем хвост,
    // но НИКОГДА не ускоряем весь фильм целиком (time-stretch всей дорожки ломал синхрон и утягивал фразы влево).
    let dub = if dub_dur > total + 0.15 {
        let fit = wd.join("dub_trimmed.wav");
        if media::trim(&dub, &fit, 0.0, total, 24_000).is_ok() {
            fit
        } else {
            dub
        }
    } else {
        dub
    };

    // 6) свести дорожку.
    let mixed = if voiceover {
        // Закадровый (UN-style voice-over): оригинал ЗВУЧИТ ПОЛНЫМ между репликами перевода (слышно
        // исходного спикера/эмоцию) и ДИНАМИЧЕСКИ приглушается на voiceover_gain_db ПОД переводом,
        // восстанавливаясь после — best-practice (IVA/Wikipedia). В паузах и во время lead-in (0.6с) оригинал 100%.
        let duck_db = proj.audio.voiceover_gain_db.clamp(VOICEOVER_DUCK_MIN_DB, 0.0);
        emit(progress, "mix", &format!(
            "voiceover: оригинал {duck_db:+.1} dB ПОД переводом, полный в паузах (динам. огибающая, {} блоков)",
            speech_blocks.len()));
        let new_audio = wd.join("new_audio.m4a");
        // Динамическая огибающая на ОРИГИНАЛ по таймингам перевода. Фолбэк — старое плоское приглушение.
        if media::mix_env_db(&dub, &audio_hq, &speech_blocks, duck_db, &new_audio).is_err() {
            emit(progress, "mix", "voiceover: огибающая недоступна -> плоское приглушение");
            let bed = if duck_db.abs() < 0.05 {
                audio_hq.clone()
            } else {
                let ducked = wd.join("orig_ducked.m4a");
                match media::gain(&audio_hq, &ducked, duck_db) {
                    Ok(()) => ducked,
                    Err(_) => audio_hq.clone(),
                }
            };
            media::mix(&dub, &bed, &new_audio)?;
        }
        new_audio
    } else if let Some(inst) = instrumental {
        // Детерминированный дакинг (#106): фон приглушается на дефолтные −3 дБ (env DUB_DUCK_DB) по
        // кусочно-линейной ОГИБАЮЩЕЙ из точных таймингов речевых блоков — компрессор (sidechaincompress)
        // реагировал на мгновенную амплитуду TTS и давал «качели» на микропаузах внутри фраз. Требование
        // юзера: «дубляж громче фона, но фон НЕ гробить» (−12 дБ срезали весь фон). Каскад фолбэков:
        // огибающая -> sidechain -> прямой mix.
        let new_audio = wd.join("new_audio.m4a");
        // Дакинг фона под дубляжом — ОПЦИЯ (duck_on), ВЫКЛ по умолчанию: не всем нужен, многим фон нужен
        // на полной громкости. Выкл -> прямой mix (фон 1:1). Вкл -> огибающая −3дБ (каскад фолбэков).
        if !crate::models::duck_enabled(&paths.models_root) {
            emit(progress, "mix", "сведение: инструментал + дубль-вокал (дакинг ВЫКЛ — фон полный)");
            media::mix(&dub, &inst, &new_audio)?;
        } else {
            emit(progress, "mix", &format!("сведение: инструментал + дубль-вокал (дакинг ВКЛ, огибающая, {} блоков)", speech_blocks.len()));
            if media::mix_env(&dub, &inst, &speech_blocks, &new_audio).is_err() {
                emit(progress, "mix", "огибающая недоступна -> сайдчейн-дакинг");
                if media::mix_ducked(&dub, &inst, &new_audio).is_err() {
                    emit(progress, "mix", "sidechain недоступен -> прямой mix");
                    media::mix(&dub, &inst, &new_audio)?;
                }
            }
        }
        new_audio
    } else {
        dub
    };
    // 7) финальная нормализация программы EBU R128 + true-peak лимитер (-1 dBTP). РЕШЕНИЕ ЮЗЕРА
    // (best-practice, НЕ питон — приказ 2026-07-12): пофразный normalize_voice выровнял спикеров (и
    // клипнул редкие пики фразы на 0.985), здесь программа приводится к целевой громкости соцсетей
    // (-14 LUFS); финальный true-peak лимитер держит межфразовые суммы и микс с фоном.
    emit(progress, "mix", "нормализация громкости (EBU R128, true-peak)");
    let final_audio = wd.join("final_audio.m4a");
    let normalized = match media::loudnorm(&mixed, &final_audio, -14.0, -1.0, 11.0) {
        Ok(()) => final_audio,
        Err(e) => {
            emit(progress, "mix", &format!("loudnorm пропущен ({e})"));
            mixed
        }
    };
    // 8) монтажный гейн всей дорожки (если задан) — наша opt-in фича «усилить всё» поверх нормализации.
    let gain_db = proj.audio.gain_db;
    if gain_db.abs() > 0.05 {
        emit(progress, "mix", &format!("гейн дорожки {gain_db:+.1} dB"));
        let gained = wd.join("gained_audio.m4a");
        match media::gain(&normalized, &gained, gain_db) {
            Ok(()) => Ok(gained),
            Err(_) => Ok(normalized),
        }
    } else {
        Ok(normalized)
    }
}

/// Референс клона на КАЖДОГО спикера: {speaker -> ref_spk{N}.wav}.
/// Порт voices.resolve clone-ветки, УЛУЧШЕННЫЙ (BORROWINGS #2): вместо «абсолютно длиннейшей реплики»
/// (часто крик/оверлап/шумная первая) выбираем СТАБИЛЬНЫЙ identity-реф — чистая реплика 7-12с, ±1с
/// внутренняя обрезка полей, дроп шумной первой реплики у говорливого спикера. Это фолбэк-реф спикера,
/// поверх которого работает per-segment эмоц-реф (emo_ref_of). Спикер None -> ключ "0" (моно-ролик).
type SpkRefs = (
    std::collections::BTreeMap<String, PathBuf>,
    std::collections::BTreeMap<String, String>,
    // альт-рефы (ступени 4-5 лестницы): {спикер -> (wav, ref_text)} — из того же скоринга/REF-QC.
    std::collections::BTreeMap<String, (PathBuf, Option<String>)>,
);

/// Идеальная длина identity-рефа спикера: 7-12с — модель клонирует стабильнее, чем на очень коротком
/// (мало тембра) или очень длинном (крик/оверлап, раздувает prefill-граф Higgs). Верх капится ref_secs.
const REF_IDEAL_LO: f64 = 7.0;
const REF_IDEAL_HI: f64 = 12.0;
/// ±1с внутренняя обрезка полей реф-клипа: края реплики часто с придыханием/захватом соседней речи.
/// Применяем только если после обрезки остаётся достаточно (≥ ~2.5с) — иначе берём клип как есть.
const REF_EDGE_TRIM: f64 = 1.0;
const REF_MIN_AFTER_TRIM: f64 = 2.5;
/// Порог «говорливого» спикера: при >4 репликах ПЕРВАЯ (часто шумный вход/бэкграунд) исключается из
/// кандидатов. У немногословного спикера первую не трогаем — иначе можно остаться без рефа.
const REF_DROP_FIRST_ABOVE: usize = 4;
/// Гейт подсистемы voice-ref+эмоция (#81/#88). При false рендер идёт СТАРЫМ путём (identity-реф =
/// длиннейшая реплика спикера, без per-segment эмоц-рефа) = питон-паритет. Ревью-рой подтвердил 5
/// паритет-брешей при live-включении без гейта → держим OFF до E2E-валидации на реальном длинном
/// контенте + probe DLL (temperature/seed). Включить = сменить на true после валидации.
const EMO_VOICE_REF: bool = false;

/// Реплика спикера «чистая», если НЕ перекрывается по времени репликой ДРУГОГО спикера (BORROWINGS #2
/// `_adjacent_to_other_speaker`): пересечение = захват чужого голоса в реф -> грязный тембр/эмоция.
/// Реплики того же спикера не считаются загрязнением. `all` — весь транскрипт (индекс+сегмент).
fn seg_is_clean(s: &dub_core::Segment, spk: &str, all: &[(usize, &dub_core::Segment)]) -> bool {
    !all.iter().any(|(_, o)| {
        let ospk = o.speaker.clone().unwrap_or_else(|| "0".into());
        ospk != spk && o.start < s.end && o.end > s.start
    })
}

/// Выбранное окно identity-рефа спикера (координаты vocals16, с ±1с обрезкой и капом ref_secs).
struct RefWindow {
    start: f64,
    end: f64,
    /// src_text выбранной реплики.
    src_text: String,
    /// true = окно = вся реплика (нет ни ±1с обрезки, ни капа) -> src_text совпадает с аудио.
    exact_cover: bool,
}

/// Выбрать окно identity-рефа спикера из его реплик по BORROWINGS #2. None — у спикера нет реплик.
fn pick_ref_window(
    spk: &str,
    segs: &[(usize, &dub_core::Segment)],
    ref_secs: f64,
) -> Option<RefWindow> {
    // реплики спикера в порядке транскрипта (для дропа первой).
    let mine: Vec<&dub_core::Segment> = segs
        .iter()
        .filter(|(_, s)| s.speaker.clone().unwrap_or_else(|| "0".into()) == spk)
        .map(|(_, s)| *s)
        .collect();
    if mine.is_empty() {
        return None;
    }
    // дроп шумной первой реплики у говорливого спикера (но всегда оставляем хоть одного кандидата).
    let pool: Vec<&dub_core::Segment> = if mine.len() > REF_DROP_FIRST_ABOVE {
        mine[1..].to_vec()
    } else {
        mine.clone()
    };
    // «полезность» кандидата (BORROWINGS #2 — НЕ «абсолютно длиннейшая», она часто крик/оверлап):
    //   1) чистота (нет оверлапа чужого спикера) — важнее всего;
    //   2) попадание В идеальную полосу 7-12с — предпочесть спокойный клип нужной длины;
    //   3) внутри полосы — длиннее лучше; ВНЕ полосы (все короче 7с) — тоже длиннее (максимум тембра),
    //      но такой кандидат всегда проигрывает любому in-band.
    // Ключ сортировки строим так, чтобы max_by брал лучший: (clean, in_band, dur_key).
    let score = |s: &dub_core::Segment| -> (bool, bool, f64) {
        let dur = (s.end - s.start).max(0.0);
        let clean = seg_is_clean(s, spk, segs);
        let in_band = dur >= REF_IDEAL_LO && dur <= REF_IDEAL_HI;
        // в полосе — ближе к верху полосы (больше тембра, но без «крик/оверлап» сверхдлинных); вне
        // полосы — просто длиннее. Отрицательное расстояние до REF_IDEAL_HI даёт «ближе к 12с = лучше».
        let dur_key = if in_band { -(REF_IDEAL_HI - dur) } else { dur };
        (clean, in_band, dur_key)
    };
    let best = pool
        .iter()
        .copied()
        .max_by(|a, b| {
            let (ca, ia, da) = score(a);
            let (cb, ib, db) = score(b);
            (ca, ia)
                .cmp(&(cb, ib))
                .then(da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal))
        })?;
    // окно = отрезок реплики. Идеал: до REF_IDEAL_HI (капается ref_secs), с ±1с обрезкой полей когда
    // после неё остаётся ≥REF_MIN_AFTER_TRIM. Короткую реплику берём целиком (парити коротких).
    let cap = ref_secs.min(REF_IDEAL_HI).max(REF_MIN_AFTER_TRIM);
    let dur = (best.end - best.start).max(0.0);
    let (mut a, mut b) = (best.start, best.end);
    let mut trimmed = false;
    if dur - 2.0 * REF_EDGE_TRIM >= REF_MIN_AFTER_TRIM {
        a += REF_EDGE_TRIM;
        b -= REF_EDGE_TRIM;
        trimmed = true;
    }
    // кап длины сверху (не раздувать prefill-граф Higgs), обрезаем хвост.
    let mut capped = false;
    if b - a > cap {
        b = a + cap;
        capped = true;
    }
    Some(RefWindow {
        start: a,
        end: b,
        src_text: best.src_text.trim().to_string(),
        exact_cover: !trimmed && !capped,
    })
}

fn build_speaker_refs(
    segs: &[(usize, &dub_core::Segment)],
    vocals16: &Path,
    wd: &Path,
    ref_secs: f64,
    asr: &mut dyn dub_asr::AsrEngine,
    progress: &Progress,
) -> Result<SpkRefs, String> {
    let mut refs: std::collections::BTreeMap<String, PathBuf> = std::collections::BTreeMap::new();
    let mut texts: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    let mut alts: std::collections::BTreeMap<String, (PathBuf, Option<String>)> =
        std::collections::BTreeMap::new();
    let mut speakers: Vec<String> =
        segs.iter().map(|(_, s)| s.speaker.clone().unwrap_or_else(|| "0".into())).collect();
    speakers.sort();
    speakers.dedup();
    if EMO_VOICE_REF {
        // НОВЫЙ выбор (#81): окно 7-12с, ±1с обрезка, дроп первой реплики. ТОЛЬКО под флагом.
        for spk in speakers {
            let Some(pick) = pick_ref_window(&spk, segs, ref_secs) else { continue };
            let ref_wav = wd.join(format!("ref_spk{spk}.wav"));
            media::trim(vocals16, &ref_wav, pick.start, pick.end.max(pick.start + 0.05), 16_000)?;
            refs.insert(spk.clone(), ref_wav);
            if pick.exact_cover && !pick.src_text.is_empty() {
                texts.insert(spk, pick.src_text);
            }
        }
        return Ok((refs, texts, alts));
    }

    // Скоринг кандидата в identity-рефы: ПЛОТНОСТЬ РЕЧИ (симв/с из готового транскрипта) ×
    // близость к Higgs-оптимуму 5-9с. REF-QC-факт (прогон 2026-07-17): «длиннейшая реплика»
    // выбирала мусор — 9.7с крика с одним «No!», хоровой выкрик интро, клип, где ASR слышит
    // тишину; клоны от таких рефов выли «Ааааа» вместо коротких фраз. Нормальная речь ~12-16
    // симв/с, крик/вой/шум — единицы.
    let cps = |s: &dub_core::Segment| -> f64 {
        s.src_text.trim().chars().count() as f64 / (s.end - s.start).max(0.1)
    };
    let score = |s: &dub_core::Segment| -> f64 {
        let dur = s.end - s.start;
        let cps_score = (1.0 - (cps(s) - 14.0).abs() / 14.0).clamp(0.0, 1.0);
        let dur_score = if (5.0..=9.0).contains(&dur) {
            1.0
        } else if dur < 5.0 {
            0.5 + (dur - 2.5) / 5.0
        } else {
            1.0 - (dur - 9.0) / 6.0
        };
        cps_score * dur_score.clamp(0.3, 1.0)
    };
    // Топ-3 кандидата на спикера + ОДНА пакетная транскрипция всех кандидатов (Whisper = один
    // сабпроцесс на список; Parakeet in-process и так быстр).
    let mut cand_map: std::collections::BTreeMap<String, Vec<&dub_core::Segment>> = Default::default();
    let mut batch: Vec<PathBuf> = Vec::new();
    let mut batch_pos: std::collections::BTreeMap<String, usize> = Default::default();
    for spk in &speakers {
        let mine: Vec<&dub_core::Segment> = segs
            .iter()
            .filter(|(_, s)| s.speaker.clone().unwrap_or_else(|| "0".into()) == *spk)
            .map(|(_, s)| *s)
            .collect();
        let mut good: Vec<&dub_core::Segment> = mine
            .iter()
            .copied()
            .filter(|s| {
                let d = s.end - s.start;
                (2.5..=ref_secs + 0.05).contains(&d) && cps(s) >= 6.0
            })
            .collect();
        good.sort_by(|a, b| score(b).partial_cmp(&score(a)).unwrap_or(std::cmp::Ordering::Equal));
        good.truncate(3);
        if good.is_empty() {
            // Фолбэк (мало данных у спикера): длиннейшая влезающая, затем длиннейшая вообще.
            let by_dur = |a: &&dub_core::Segment, b: &&dub_core::Segment| {
                (a.end - a.start).partial_cmp(&(b.end - b.start)).unwrap_or(std::cmp::Ordering::Equal)
            };
            let fitting = mine
                .iter()
                .copied()
                .filter(|s| (2.5..=ref_secs + 0.05).contains(&(s.end - s.start)))
                .max_by(by_dur);
            if let Some(c) = fitting.or_else(|| mine.iter().copied().max_by(by_dur)) {
                good.push(c);
            }
        }
        batch_pos.insert(spk.clone(), batch.len());
        for (i, c) in good.iter().enumerate() {
            let p = wd.join(format!("ref_cand_spk{spk}_{i}.wav"));
            media::trim(vocals16, &p, c.start, c.end.min(c.start + ref_secs), 16_000)?;
            batch.push(p);
        }
        cand_map.insert(spk.clone(), good);
    }
    // REF-QC: транскрипт каждого кандидата сверяем с его src_text — реф обязан ЗВУЧАТЬ как его
    // текст (кривой реф = кривой ref_text = каскад брака в клоне). Сбой ASR -> None -> кандидат
    // принимается без сверки (не хуже прежнего поведения).
    let heard: Vec<Option<String>> = asr.transcribe_many(&batch, "auto");
    for spk in &speakers {
        let cands = &cand_map[spk];
        if cands.is_empty() {
            continue;
        }
        let base = batch_pos[spk];
        // (кандидат, услышанное, прошёл ли сверку)
        let verdict: Vec<(usize, Option<&str>, bool)> = cands
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let h = heard.get(base + i).and_then(|o| o.as_deref());
                let ok = match h {
                    Some(t) => qc_similarity(&c.src_text, t) >= 0.5,
                    None => true, // ASR молчит про сбой — доверяем скорингу
                };
                (i, h, ok)
            })
            .collect();
        let passed: Vec<&(usize, Option<&str>, bool)> = verdict.iter().filter(|v| v.2).collect();
        let (main_i, main_heard) = match passed.first() {
            Some((i, h, _)) => (*i, *h),
            None => {
                let h0 = verdict[0].1.unwrap_or("");
                emit(
                    progress,
                    "tts",
                    &format!("⚠ спикер {spk}: все реф-кандидаты не прошли сверку (слышно: «{}») — беру лучший по скору", h0.chars().take(60).collect::<String>()),
                );
                (0, verdict[0].1)
            }
        };
        let main_seg = cands[main_i];
        let ref_wav = wd.join(format!("ref_spk{spk}.wav"));
        std::fs::rename(wd.join(format!("ref_cand_spk{spk}_{main_i}.wav")), &ref_wav)
            .map_err(|e| format!("реф спикера {spk}: {e}"))?;
        refs.insert(spk.clone(), ref_wav);
        // ref_text: прошёл сверку -> УСЛЫШАННОЕ (точно соответствует звуку клипа); иначе src_text.
        let t = main_heard
            .filter(|h| !h.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| main_seg.src_text.trim().to_string());
        if !t.is_empty() {
            texts.insert(spk.clone(), t);
        }
        // Альт-реф (ступени 4-5 лестницы ретраев): следующий ПРОШЕДШИЙ сверку кандидат; фолбэк —
        // просто следующий по скору. Спикер с одним кандидатом остаётся без альтернативы.
        let alt = passed
            .iter()
            .find(|(i, _, _)| *i != main_i)
            .map(|(i, h, _)| (*i, *h))
            .or_else(|| verdict.iter().find(|(i, _, _)| *i != main_i).map(|(i, h, _)| (*i, *h)));
        if let Some((ai, ah)) = alt {
            let alt_wav = wd.join(format!("ref_alt_spk{spk}.wav"));
            if std::fs::rename(wd.join(format!("ref_cand_spk{spk}_{ai}.wav")), &alt_wav).is_ok() {
                let at = ah
                    .filter(|h| !h.trim().is_empty())
                    .map(str::to_string)
                    .or_else(|| Some(cands[ai].src_text.trim().to_string()).filter(|s| !s.is_empty()));
                alts.insert(spk.clone(), (alt_wav, at));
            }
        }
        // Прибрать невостребованных кандидатов.
        for (i, _) in cands.iter().enumerate() {
            let _ = std::fs::remove_file(wd.join(format!("ref_cand_spk{spk}_{i}.wav")));
        }
        emit(
            progress,
            "tts",
            &format!(
                "реф спикера {spk}: «{}» ({:.1}с, {} кандидата, сверка {})",
                texts.get(spk).map(|s| s.chars().take(50).collect::<String>()).unwrap_or_default(),
                main_seg.end - main_seg.start,
                cands.len(),
                if passed.is_empty() { "⚠ не пройдена" } else { "ok" }
            ),
        );
    }
    Ok((refs, texts, alts))
}

/// Пауза между речевыми блоками, короче которой блоки СЛИВАЮТСЯ (музыку в коротких паузах не поднимаем).
const DUCK_BLOCK_GAP: f64 = 1.6;

/// Слить уложенные сегменты в речевые блоки для дакинг-огибающей (#106). Границы — по ФАКТУ: onset +
/// длительность fit-файла (то, что реально легло в таймлайн). Сортируем по onset, объединяем в один блок,
/// если пауза между концом предыдущего и стартом следующего < DUCK_BLOCK_GAP. Сбой чтения длительности —
/// Спаны — ФАКТИЧЕСКАЯ укладка из timeline (единый источник правды, без повторного ffprobe [22]/[5]).
fn build_speech_blocks(spans: &[(f64, f64)]) -> Vec<media::SpeechBlock> {
    let mut spans: Vec<(f64, f64)> = spans.to_vec();
    spans.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut blocks: Vec<media::SpeechBlock> = Vec::new();
    for (s, e) in spans {
        match blocks.last_mut() {
            Some(b) if s - b.end < DUCK_BLOCK_GAP => b.end = b.end.max(e),
            _ => blocks.push(media::SpeechBlock { start: s, end: e }),
        }
    }
    blocks
}

/// Ускорить или замедлить дубль под target_dur. factor>1 ускоряет (укорачивает); <1 замедляет
/// (растягивает). Замедление ограничено MIN_SLOW=0.85 (~15% растяжения), чтобы голос не тянулся
/// неестественно. `cap` — потолок ускорения (считается у вызова: seg_cap + дрейф-эскалация).
/// `pause_squeeze` — умное сжатие пауз перед применением atempo.
/// Возвращает путь уложенного файла И его фактическую длительность.
fn fit_to_slot(seg_wav: &Path, target_dur: f64, work_path: &Path, cap: f64, pause_squeeze: bool) -> Result<(PathBuf, f64), String> {
    let actual = media::duration(seg_wav)?;
    if target_dur <= 0.05 || actual <= 0.05 {
        return Ok((seg_wav.to_path_buf(), actual.max(0.0)));
    }
    const MIN_SLOW: f64 = 0.85;

    // Если фраза не помещается (actual > target_dur) и включено сжатие пауз:
    let (cur_wav, actual_dur) = if pause_squeeze && actual > target_dur * 1.02 {
        let sq_path = work_path.with_extension("squeezed.wav");
        if let Ok((p, sq_dur)) = media::squeeze_internal_pauses_wav(seg_wav, &sq_path, 40.0) {
            (p, sq_dur)
        } else {
            (seg_wav.to_path_buf(), actual)
        }
    } else {
        (seg_wav.to_path_buf(), actual)
    };

    let mut factor = actual_dur / target_dur;
    factor = factor.min(cap).max(MIN_SLOW);
    if (0.98..=1.02).contains(&factor) {
        if cur_wav.as_path() != seg_wav {
            // Если паузы сжаты и этого хватило, сохраняем результат в work_path
            let _ = std::fs::copy(&cur_wav, work_path);
            let _ = std::fs::remove_file(&cur_wav);
            return Ok((work_path.to_path_buf(), actual_dur));
        }
        return Ok((seg_wav.to_path_buf(), actual));
    }
    media::time_stretch(&cur_wav, work_path, factor)?;
    if cur_wav.as_path() != seg_wav {
        let _ = std::fs::remove_file(&cur_wav);
    }
    let d = media::duration(work_path).unwrap_or(actual_dur / factor);
    Ok((work_path.to_path_buf(), d))
}

/// Уложить сегменты на полную дорожку по таймкодам, без перекрытия/обрезки. Порт assemble.timeline.
/// Применяет 10 мс crossfade к краям фраз для устранения кликов.
fn timeline(placed: &[(f64, PathBuf, f64)], total_dur: f64, out_wav: &Path) -> Result<Vec<(f64, f64)>, String> {
    if placed.is_empty() {
        // тишина total_dur @ 24000.
        let n = (total_dur * 24000.0) as usize;
        wavio::write_mono_f32(out_wav, &vec![0.0f32; n], 24000)?;
        return Ok(Vec::new());
    }
    let mut placed: Vec<(f64, PathBuf, f64)> = placed.to_vec();
    placed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    // sr берём из первого файла.
    let first = wavio::read_mono_f32(&placed[0].1)?;
    let sr = first.1;
    let mut laid: Vec<(f64, Vec<f32>)> = Vec::with_capacity(placed.len());
    let mut spans: Vec<(f64, f64)> = Vec::with_capacity(placed.len());
    let mut cursor = 0.0f64;
    for (start, wav, _) in &placed {
        let (mut s, ssr) = if *wav == placed[0].1 {
            (first.0.clone(), first.1)
        } else {
            wavio::read_mono_f32(wav)?
        };
        normalize_voice(&mut s, ssr); // все фразы/спикеры к одной громкости

        // 10ms Crossfade (fade-in & fade-out) для бесшовного стыка без кликов
        let fade_len = ((sr as f64 * 0.010) as usize).min(s.len() / 2);
        if fade_len > 0 {
            for k in 0..fade_len {
                let f = k as f32 / fade_len as f32;
                s[k] *= f;
                let end_k = s.len() - 1 - k;
                s[end_k] *= f;
            }
        }

        let at = start.max(cursor);
        let end = at + s.len() as f64 / sr as f64;

        cursor = end;
        spans.push((at, end));
        laid.push((at, s));
    }
    let len = ((total_dur.max(cursor) * sr as f64).ceil()) as usize;
    let mut track = vec![0.0f32; len];
    for (at, s) in &laid {
        let i = (at * sr as f64) as usize;
        let end = (i + s.len()).min(track.len());
        for (k, v) in s.iter().take(end - i).enumerate() {
            track[i + k] += *v;
        }
    }
    // НЕ делить всю дорожку на глобальный пик: один громкий сэмпл (крик) ронял громкость ВСЕГО
    // фильма на ~12дБ (жалоба «голос тихий», замер -30.6 LUFS при фразах -18.7). Фразы уже
    // пик-клипнуты в normalize_voice; здесь лишь страховка от сумм при наложении — локальный клип.
    for x in &mut track {
        *x = x.clamp(-0.985, 0.985);
    }
    wavio::write_mono_f32(out_wav, &track, sr)?;
    Ok(spans)
}

/// Выровнять ОДНУ фразу к общей громкости, чтобы все спикеры звучали одинаково громко (dialog-gated
/// нормализация): интегральная громкость BS.1770 к -14 LUFS; короткие/тихие фразы — RMS к -16 dBFS.
/// Цель поднята с -16 (замер 2026-07-17: фон мультика -17.1 LUFS, голос -16 давал зазор всего 1.1 LU —
/// «дубляж не слышно»; вместе с поджимом фона -3 дБ в миксе зазор ~6 LU = нижняя проф-норма).
/// РЕШЕНИЕ ЮЗЕРА (EBU R128 best-practice, НЕ копия питона — «гугли best practices, не повторяй за мной»,
/// приказ 2026-07-12): гейн НЕ клэмпится вниз (тихая фраза дожимается, сани-кап +40 dB от раздувания
/// почти-тишины), а редкие пики результата клипятся на 0.985 — иначе timeline давил всю дорожку
/// глобальным делением на пик одного крика (-12 дБ всему фильму, замер R5).
fn normalize_voice(x: &mut [f32], sr: u32) {
    if x.is_empty() {
        return;
    }
    let peak = x.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
    if peak < 1e-4 {
        return; // почти тишина -> не трогаем
    }
    let mut gain: Option<f64> = None;
    if x.len() >= (0.4 * sr as f64) as usize {
        if let Some(li) = integrated_lufs(x, sr) {
            if li.is_finite() && li > -60.0 {
                gain = Some(10f64.powf((-14.0 - li) / 20.0));
            }
        }
    }
    let gain = gain.unwrap_or_else(|| {
        let thr = peak * 0.05;
        let (mut sum, mut cnt) = (0.0f64, 0usize);
        for &v in x.iter() {
            if v.abs() > thr {
                sum += (v as f64) * (v as f64);
                cnt += 1;
            }
        }
        let rms = if cnt > 0 {
            (sum / cnt as f64).sqrt()
        } else {
            (x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / x.len() as f64).sqrt()
        }
        .max(1e-9);
        10f64.powf(-16.0 / 20.0) / rms
    });
    let gain = gain.min(10f64.powf(40.0 / 20.0)); // сани-кап +40 dB (не раздувать почти-тишину)
    // Пик-клип 0.985 ПОФРАЗНО: у нормализованной к -16 LUFS фразы редкие пики могут выйти за 1.0 —
    // клипим доли процента сэмплов ЗДЕСЬ, чтобы timeline не давил ВСЮ дорожку глобальным делением
    // на пик одной фразы (замер R5: сегменты -18.7 LUFS, дорожка после деления -30.6 = «голос тихий»).
    for v in x.iter_mut() {
        *v = ((*v as f64 * gain) as f32).clamp(-0.985, 0.985);
    }
}

/// Интегральная громкость ITU-R BS.1770 (LUFS) моно-сигнала: K-weighting (high-shelf + high-pass
/// биквады как в pyloudnorm) -> блоки 400мс с overlap 75% -> абсолютный гейт -70 + относительный -10.
/// None если блоков не осталось (слишком коротко/тихо).
fn integrated_lufs(x: &[f32], sr: u32) -> Option<f64> {
    let hs = biquad_high_shelf(1681.9744509555319, 0.7071752369554196, 4.0, sr as f64);
    let hp = biquad_high_pass(38.13547087613982, 0.5003270373253953, sr as f64);
    let y = apply_biquad(&apply_biquad(x, &hs), &hp);
    let block = (0.4 * sr as f64) as usize;
    let step = (0.1 * sr as f64) as usize;
    if block == 0 || step == 0 || y.len() < block {
        return None;
    }
    let mut zs: Vec<f64> = Vec::new();
    let mut i = 0;
    while i + block <= y.len() {
        let ms: f64 = y[i..i + block].iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>()
            / block as f64;
        zs.push(ms);
        i += step;
    }
    if zs.is_empty() {
        return None;
    }
    let loud = |z: f64| -0.691 + 10.0 * (z.max(1e-12)).log10();
    // абсолютный гейт -70 LUFS
    let abs_gated: Vec<f64> = zs.iter().copied().filter(|&z| loud(z) >= -70.0).collect();
    if abs_gated.is_empty() {
        return None;
    }
    let mean_abs = abs_gated.iter().sum::<f64>() / abs_gated.len() as f64;
    let rel_thr = loud(mean_abs) - 10.0;
    let rel_gated: Vec<f64> = abs_gated.into_iter().filter(|&z| loud(z) >= rel_thr).collect();
    if rel_gated.is_empty() {
        return None;
    }
    let mean_rel = rel_gated.iter().sum::<f64>() / rel_gated.len() as f64;
    Some(loud(mean_rel))
}

/// Биквад-фильтр прямой формы I (b/a нормированы на a0). Возвращает отфильтрованный сигнал.
fn apply_biquad(x: &[f32], c: &[f64; 5]) -> Vec<f32> {
    let [b0, b1, b2, a1, a2] = *c;
    let (mut x1, mut x2, mut y1, mut y2) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut out = Vec::with_capacity(x.len());
    for &xn in x {
        let xn = xn as f64;
        let yn = b0 * xn + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
        x2 = x1;
        x1 = xn;
        y2 = y1;
        y1 = yn;
        out.push(yn as f32);
    }
    out
}

/// High-shelf биквад (pyloudnorm K-weighting stage 1). Коэффы [b0,b1,b2,a1,a2] нормированы на a0.
fn biquad_high_shelf(fc: f64, q: f64, gain_db: f64, sr: f64) -> [f64; 5] {
    let a = 10f64.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f64::consts::PI * fc / sr;
    let (cw, sw) = (w0.cos(), w0.sin());
    let alpha = sw / (2.0 * q);
    let am = a - 1.0;
    let ap = a + 1.0;
    let sa = 2.0 * a.sqrt() * alpha;
    let b0 = a * (ap + am * cw + sa);
    let b1 = -2.0 * a * (am + ap * cw);
    let b2 = a * (ap + am * cw - sa);
    let a0 = ap - am * cw + sa;
    let a1 = 2.0 * (am - ap * cw);
    let a2 = ap - am * cw - sa;
    [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0]
}

/// High-pass биквад (pyloudnorm K-weighting stage 2, RLB). Коэффы нормированы на a0.
fn biquad_high_pass(fc: f64, q: f64, sr: f64) -> [f64; 5] {
    let w0 = 2.0 * std::f64::consts::PI * fc / sr;
    let (cw, sw) = (w0.cos(), w0.sin());
    let alpha = sw / (2.0 * q);
    let b0 = (1.0 + cw) / 2.0;
    let b1 = -(1.0 + cw);
    let b2 = (1.0 + cw) / 2.0;
    let a0 = 1.0 + alpha;
    let a1 = -2.0 * cw;
    let a2 = 1.0 - alpha;
    [b0 / a0, b1 / a0, b2 / a0, a1 / a0, a2 / a0]
}

/// E2E-верификация капшенов без TTS/аудио: собрать ASS из Project и вжечь блюр+сабы в captioned.mp4.
/// Используется примером verify_captions (реальные кадры порт-vs-эталон). Порт captions.build+burn
/// call-site pipeline.run, но БЕЗ аудио-ветки.
pub(crate) fn build_and_burn_captions(
    proj: &Project,
    input: &Path,
    out_ass: &Path,
    captioned: &Path,
    fonts_dir: &Path,
    vw: i64,
    vh: i64,
    total: f64,
    src_codec: &str,
) -> Result<(), String> {
    dub_captions::set_fonts_dir(fonts_dir);
    let sub_covers = build_ass(proj, out_ass, vw, vh, total)?;
    let mut blur_boxes = collect_blur_boxes(proj);
    blur_boxes.extend(sub_covers.iter().map(cover_to_blur));
    dub_captions::burn(
        input,
        out_ass,
        captioned,
        &blur_boxes,
        Some((vw, vh)),
        proj.render.blur,
        true,
        true,
        proj.render.burn_cq,
        Some(src_codec),
        proj.render.blur_sigma,
    )
}

/// Блюр-подложка под нашим субтитром -> BlurBox (fill=None -> gblur). Старые band-боксы не трогаем.
pub(crate) fn cover_to_blur(c: &dub_captions::SubCover) -> BlurBox {
    BlurBox { x: c.x, y: c.y, w: c.w, h: c.h, t0: c.t0, t1: c.t1, fill: None }
}

/// Собрать ASS через dub-captions из Project. Порт captions.build call-site pipeline.run. Возвращает
/// габариты подложек под дублированными субтитрами (для блюр-подложки; см. SubCover).
pub(crate) fn build_ass(proj: &Project, out_ass: &Path, vw: i64, vh: i64, total: f64) -> Result<Vec<dub_captions::SubCover>, String> {
    let titles: Vec<CapTitle> = proj.captions.titles.iter().map(map_title).collect();
    let sub_style = proj.captions.sub_style.as_ref().map(map_sub_style);
    // sub_y дефолт vh*0.84 если не задан (как pipeline.py: не затирать edited/pinned sub_y).
    let sub_y = proj.captions.sub_y.unwrap_or((vh as f64 * 0.84) as i64);
    // PER-SEGMENT Y-RIDE (порт pipeline.py 616-631). Каждую дублированную строку кладём на y, где в этот
    // момент была ОРИГИНАЛЬНАЯ полоса сабов, чтобы наш текст/плашка НАКРЫЛИ заблюренный оригинал (а не
    // висели на одной фикс-линии, пока блюр другой строки просвечивает). Источник полосы —
    // persisted blur_boxes. Питон медианит per-segment seg_y (pipeline.py 757-765) ТОЛЬКО по caption_boxes
    // (субтитр-полоса из analyze_layout), а НЕ по всему blur-набору — титры/таглайны/group туда не входят.
    // Порт складывает всё в blur_boxes, поэтому band-подмножество помечено compose.rs маркером extra["band"]
    // (= питоновский caption_boxes-производный band_blur). seg_y едет ТОЛЬКО по нему. Без такого — верхний
    // title/tagline-блюр в нижней половине кадра затягивал медиану вверх и строка садилась мимо полосы
    // Fallback (старые проекты без
    // маркера / ручной blur из редактора): весь blur-набор, как было — иначе потеряли бы band целиком.
    let all_band: Vec<&dub_core::BlurBox> =
        proj.captions.blur_boxes.iter().filter(|b| !b.hidden).collect();
    let tagged: Vec<&dub_core::BlurBox> = all_band
        .iter()
        .copied()
        .filter(|b| b.extra.get("band").and_then(|v| v.as_bool()).unwrap_or(false))
        .collect();
    let band: Vec<&dub_core::BlurBox> = if tagged.is_empty() { all_band } else { tagged };
    let no_band = band.len() < 3; // нет повторяющейся ОРИГИНАЛЬНОЙ полосы -> не на что ехать
    let cap_lo = 0.40 * vw as f64;
    let cap_hi = 0.60 * vw as f64;
    let seg_y = |st: f64, en: f64| -> i64 {
        if proj.captions.sub_y_locked || no_band || proj.captions.sub_y.is_some() {
            return sub_y; // editor-pinned или задан sub_y -> строго выбранная высота
        }
        // медиана y-центров band-боксов, перекрывающих сегмент по времени, центрированных по X,
        // в нижней половине кадра (ехать на нижнюю оригинальную полосу, не на верхний оверлей).
        let mut ys: Vec<f64> = band
            .iter()
            .filter(|b| {
                let cyb = b.y as f64 + b.h as f64 / 2.0;
                (b.t0 as f64) < en + 0.3
                    && (b.t1 as f64) > st - 0.3
                    && (b.x as f64) < cap_hi
                    && (b.x as f64 + b.w as f64) > cap_lo
                    && cyb >= 0.45 * vh as f64
            })
            .map(|b| b.y as f64 + b.h as f64 / 2.0)
            .collect();
        if ys.is_empty() {
            return sub_y;
        }
        ys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        ys[ys.len() / 2] as i64
    };
    // Per-segment CaptionOverride: карта seg_id -> текст-оверрайд (редактор дал свой текст строки).
    // Порт продуктового требования «научить build_ass читать overrides»: питоновский write_artifacts
    // overrides в план НЕ прокидывает (хранит round-trip), поэтому текст-оверрайд — Rust-улучшение поверх
    // источника истины: если для сегмента задан override.text, рисуем ЕГО вместо tgt_text. Стилевые
    // per-seg поля (override.style/x/y/w/fs) сохраняются в Project, но в ASS-строку пока не вплетаются —
    // dub-captions строит субтитр из общего sub_style; это совпадает с питоном (тоже не рендерит их).
    let overrides: std::collections::HashMap<&str, &str> = proj
        .captions
        .overrides
        .iter()
        .filter_map(|o| o.text.as_deref().map(|t| (o.seg_id.as_str(), t)))
        .collect();
    // Режим «без субтитров» (subs.mode=none) -> НЕ рисуем строки субтитров вообще. Титры/локализация
    // экранного текста живут отдельно (proj.captions.titles) и не затрагиваются. Раньше build_ass
    // рендерил сегменты безусловно -> в режиме «без субтитров» они всё равно прожигались (баг-репорт).
    let subs: Vec<Sub> = if proj.subs.mode == "none" {
        Vec::new()
    } else { proj
        .segments
        .iter()
        .filter(|s| {   // hidden -> нет субтитра; keep_original -> играет оригинал, субтитра нет (порт write_artifacts)
            !s.extra.get("hidden").and_then(|v| v.as_bool()).unwrap_or(false)
                && !s.extra.get("keep_original").and_then(|v| v.as_bool()).unwrap_or(false)
        })
        .map(|s| {
            let tgt = match overrides.get(s.id.as_str()) {
                Some(t) => strip_higgs_tags(t),
                None => {
                    let t = strip_higgs_tags(&s.tgt_text);
                    if !t.is_empty() {
                        t
                    } else {
                        strip_higgs_tags(&s.src_text)
                    }
                }
            };
            (s, tgt)
        })
        .filter(|(_, tgt)| !tgt.trim().is_empty())
        .map(|(s, tgt)| {
            let end = if s.end > 0.0 { s.end } else { total };
            Sub {
                start: s.start,
                end,
                tgt,
                y: Some(seg_y(s.start, end)),
            }
        })
        .collect() };

    let preset = proj.captions.preset.name.clone();
    let caption_style = preset.as_deref().filter(|n| *n != "match");
    let args = dub_captions::BuildArgs {
        preset: caption_style,
        titles: &titles,
        subs: &subs,
        max_lines: 2,
        sub_y: Some(sub_y),
        sub_style: sub_style.as_ref(),
        caption_style,
        caption_plate: proj.captions.preset.plate.as_deref(),
        caption_reveal: proj.captions.preset.reveal.as_deref(),
        caption_font: proj.captions.preset.font.as_deref(),
        sub_px: proj
            .captions
            .raw_plan
            .get("sub_px")
            .and_then(|v| v.as_i64()),
    };
    dub_captions::build(vw, vh, out_ass, args)
}

/// Blur-боксы из Project (project.captions.blur_boxes, hidden исключаются). Порт caption_plan blur_boxes.
/// В режиме «без субтитров» (subs.mode=none) НЕ блюрим band-полосу (место оригинальных субтитров) — раз
/// мы не накладываем свои субтитры, незачем и закрашивать оригинал (баг-репорт: лишний блюр + прогон).
/// OCR-блюр экранного текста/титров (не-band) остаётся — локализация картинки от субтитров не зависит.
fn collect_blur_boxes(proj: &Project) -> Vec<BlurBox> {
    let drop_band = proj.subs.mode == "none";
    proj.captions
        .blur_boxes
        .iter()
        .filter(|b| !b.hidden)
        .filter(|b| !(drop_band && b.extra.get("band").and_then(|v| v.as_bool()).unwrap_or(false)))
        .map(|b| BlurBox { x: b.x, y: b.y, w: b.w, h: b.h, t0: b.t0, t1: b.t1, fill: b.fill.clone() })
        .collect()
}

fn map_title(t: &CoreTitle) -> CapTitle {
    CapTitle {
        text: if !t.tgt.is_empty() { t.tgt.clone() } else { t.text.clone() },
        bbox: t.bbox.clone(),
        color: t.color.clone(),
        bg: t.bg.clone(),
        font: t.font.clone(),
        italic: t.italic,
        align: t.align.clone(),
        start: t.start,
        end: t.end,
        lh: t.lh,
        solid: t.solid,
        bold: t.bold,
        size_px: t.size_px,
        outline: t.outline.clone(),
        outline_w: t.outline_w,
        shadow_dir: t.shadow_dir,
        uppercase: t.uppercase,
    }
}

fn map_sub_style(s: &CoreSubStyle) -> CapSubStyle {
    // background берём из extra (vision кладёт "background"); scene_* тоже могут быть в extra/типизированы.
    let bg = s
        .extra
        .get("background")
        .and_then(|v| v.as_str())
        .map(|x| x.to_string());
    let size_frac = s.extra.get("size_frac").and_then(|v| v.as_f64());
    CapSubStyle {
        color: s.color.clone(),
        background: bg,
        outline: Some(s.outline.clone()),
        outline_w: s.outline_w,
        shadow_dir: s.shadow_dir,
        bold: s.bold,
        italic: s.italic,
        uppercase: s.uppercase,
        align: s.align.clone(),
        font: s.font.clone(),
        n_lines: s.n_lines,
        size_frac,
        size_px: s.size_px,
        scene_color: s.scene_color.clone(),
        scene_flat: s.scene_flat,
        solid: s.extra.get("solid").and_then(|v| v.as_bool()).unwrap_or(false),
        // plate/plate_color из extra (PATCH caption их кладёт).
        plate: s.extra.get("plate").and_then(|v| v.as_bool()),
        plate_color: s
            .extra
            .get("plate_color")
            .and_then(|v| v.as_str())
            .map(|x| x.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dub_core::{BlurBox as CoreBlurBox, Segment};

    fn bb(x: i64, y: i64, w: i64, h: i64, t0: f64, t1: f64, band: bool) -> CoreBlurBox {
        let mut b = CoreBlurBox {
            x, y, w, h, t0, t1, hidden: false, fill: None, extra: Default::default(),
        };
        if band {
            b.extra.insert("band".into(), Value::Bool(true));
        }
        b
    }

    fn seg(id: &str, start: f64, end: f64, tgt: &str) -> Segment {
        // ..Default::default() для полей вне интереса теста (id/тайминги/tgt) — устойчиво к добавлению
        // новых полей Segment смежными подсистемами (напр. ckpt чекпоинтинга).
        Segment {
            id: id.into(),
            start,
            end,
            tgt_text: tgt.into(),
            ..Default::default()
        }
    }

    // Y самого раннего S-субтитра (наш дублированный) в ASS: \pos(cx,cy) -> cy.
    fn first_sub_y(ass: &str) -> i64 {
        for l in ass.lines() {
            if l.starts_with("Dialogue:") && l.contains(",S,") {
                if let Some(i) = l.find("\\pos(") {
                    let rest = &l[i + 5..];
                    let close = rest.find(')').unwrap();
                    let inner = &rest[..close];
                    let cy: i64 = inner.split(',').nth(1).unwrap().trim().parse().unwrap();
                    return cy;
                }
            }
        }
        panic!("нет S-субтитра с \\pos в ASS:\n{ass}");
    }

    fn build_to_string(proj: &Project, vw: i64, vh: i64) -> String {
        let dir = std::env::temp_dir()
            .join(format!("render_segy_{}_{}.ass", std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        build_ass(proj, &dir, vw, vh, 44.77).unwrap();
        let s = std::fs::read_to_string(&dir).unwrap();
        let _ = std::fs::remove_file(&dir);
        s
    }

    // ducks_ru (ru→en), кадр s0 «SUBSCRIPTION.»: верхний title/tagline-блюр (cy≈438,464)
    // сидит в НИЖНЕЙ половине 824-кадра (>=0.45vh=371) вместе с настоящей полосой (cy≈630-642). Питон медианит
    // seg_y ТОЛЬКО по caption_boxes (полоса). Band-боксы помечены extra["band"], seg_y едет только
    // по ним -> строка на полосе, плашка (BorderStyle=3) обнимает текст там же, блюр оригинала накрыт.
    // Две фразы разной входной громкости после normalize_voice звучат одинаково (RMS сходится) —
    // все спикеры на выходе на одном уровне.
    #[test]
    fn normalize_equalizes_speaker_loudness() {
        let sr = 24000u32;
        let sine = |amp: f32| -> Vec<f32> {
            (0..sr) // 1 c
                .map(|i| amp * (2.0 * std::f64::consts::PI * 180.0 * i as f64 / sr as f64).sin() as f32)
                .collect()
        };
        let rms = |x: &[f32]| (x.iter().map(|&v| (v * v) as f64).sum::<f64>() / x.len() as f64).sqrt();
        let mut loud = sine(0.8);
        let mut quiet = sine(0.08);
        normalize_voice(&mut loud, sr);
        normalize_voice(&mut quiet, sr);
        let (rl, rq) = (rms(&loud), rms(&quiet));
        // после выравнивания уровни должны сойтись (dialog-gated нормализация); пики держит финальный
        // media::loudnorm true-peak лимитер на смиксованной дорожке (пофразного клэмпа нет — решение юзера).
        assert!((rl - rq).abs() / rl.max(1e-9) < 0.15, "уровни должны сойтись: loud={rl:.4} quiet={rq:.4}");
    }

    #[test]
    fn seg_y_rides_band_not_tagline_ducks() {
        let (vw, vh) = (464i64, 824i64);
        let mut proj = Project::default();
        proj.mode = "dub".into();
        proj.subs.mode = "translate".into(); // включить субтитры (subs=none даёт пустой ASS с фикса #47)
        proj.captions.sub_y = Some(634);
        proj.captions.sub_y_locked = false;
        // sub_style как у ducks: белый Oswald caps на тёмной полосе (vision background -> BorderStyle=3).
        let mut ss = CoreSubStyle::default();
        ss.color = "#FFFFFF".into();
        ss.bold = true;
        ss.uppercase = true;
        ss.font = Some("Oswald".into());
        ss.extra.insert("background".into(), Value::String("#000000".into()));
        proj.captions.sub_style = Some(ss);
        // Покадровый таглайн cy≈437/464 (10 боксов, НЕ band) равен по числу настоящей полосе
        // cy≈630-686 (9 боксов, band): без разделения списков таглайн затянул бы медиану.
        let mut boxes = Vec::new();
        for t in [1.75, 2.0, 2.25, 2.5, 2.75].iter() {
            boxes.push(bb(161, 427, 140, 21, *t, *t + 0.25, false)); // таглайн стр.1 cy=437
            boxes.push(bb(42, 453, 378, 22, *t, *t + 0.25, false));  // таглайн стр.2 cy=464 (широкая)
        }
        for (cy, t) in [(630.0, 1.75), (630.0, 2.0), (641.0, 2.25), (641.0, 2.5),
                        (641.0, 2.75), (641.0, 3.0), (652.0, 1.75), (652.0, 2.0), (686.0, 2.5)]
        {
            let y = cy as i64 - 9;
            boxes.push(bb(120, y, 220, 19, t, t + 0.25, true)); // полоса
        }
        proj.captions.blur_boxes = boxes;
        proj.segments = vec![
            seg("s0", 2.22, 2.80, "SUBSCRIPTION."), // окно 4-й word-группы s0
        ];
        let ass = build_to_string(&proj, vw, vh);
        let y = first_sub_y(&ass);
        assert!(
            (630..=650).contains(&y),
            "строка должна ехать на полосу оригинала (~640), а не на таглайн (~464): y={y}\n{ass}"
        );
        // плашка обнимает текст: S-style несёт BorderStyle=3 (плашка = обводка вокруг текста той же строки).
        let s_style = ass.lines().find(|l| l.starts_with("Style: S,")).unwrap();
        assert!(s_style.contains(",3,11,"), "плашка = BorderStyle=3 в стиле строки: {s_style}");
    }

    // Fallback: проект без маркеров band (ручной blur из редактора) — seg_y работает по всему
    // blur-набору. Здесь только полоса без тегов -> едет на неё.
    #[test]
    fn seg_y_fallback_untagged_boxes() {
        let (vw, vh) = (464i64, 824i64);
        let mut proj = Project::default();
        proj.mode = "dub".into();
        proj.subs.mode = "translate".into(); // включить субтитры (subs=none даёт пустой ASS с фикса #47)
        proj.captions.sub_y = Some(634);
        let mut boxes = Vec::new();
        for t in [0.75, 1.0, 1.25, 1.5].iter() {
            boxes.push(bb(120, 631, 220, 19, *t, *t + 0.25, false)); // НЕ помечены band
        }
        proj.captions.blur_boxes = boxes;
        proj.segments = vec![seg("s0", 1.0, 1.5, "TEXT")];
        let ass = build_to_string(&proj, vw, vh);
        let y = first_sub_y(&ass);
        assert!((630..=650).contains(&y), "fallback: без маркеров едем по всему набору: y={y}");
    }

    #[test]
    fn test_strip_higgs_tags_basic() {
        assert_eq!(strip_higgs_tags("<|emotion:anger|>Привет мир!"), "Привет мир!");
        assert_eq!(strip_higgs_tags("<|style:whispering|>Тихий шепот"), "Тихий шепот");
        assert_eq!(strip_higgs_tags("Начало <|prosody:pause|> конец."), "Начало конец.");
        assert_eq!(strip_higgs_tags("<|sfx:laughter|>Haha, смешная шутка!"), "Haha, смешная шутка!");
        assert_eq!(strip_higgs_tags("<|emotion:fear|><|style:shouting|> Помогите! <|prosody:speed_fast|>"), "Помогите!");
        assert_eq!(strip_higgs_tags("Обычный текст без тегов."), "Обычный текст без тегов.");
        assert_eq!(strip_higgs_tags("<|emotion:sadness|>"), "");
    }

    #[test]
    fn test_build_ass_strips_higgs_tags() {
        let (vw, vh) = (1080i64, 1920i64);
        let mut proj = Project::default();
        proj.mode = "dub".into();
        proj.subs.mode = "translate".into();
        proj.segments = vec![
            seg("s0", 1.0, 2.0, "<|emotion:anger|>Сердитая фраза!"),
            seg("s1", 2.5, 3.5, "<|sfx:laughter|>Haha, смешно!"),
        ];
        let ass = build_to_string(&proj, vw, vh);
        assert!(ass.contains("Сердитая фраза!"));
        assert!(ass.contains("Haha, смешно!"));
        assert!(!ass.contains("<|emotion:anger|>"));
        assert!(!ass.contains("<|sfx:laughter|>"));
    }
}
