# План реализации: Улучшение тайминга, укладки речи и качества дубляжа в Dub Studio

## 1. Цель и решаемые проблемы

Данный план устраняет основные дефекты синхронизации и темпа речи в модуле дубляжа Dub Studio:
1. **Языковой перекос длины:** русский перевод естественным образом на 20–35% длиннее английского оригинала, из-за чего фразы вылетают из тайм-слотов.
2. **Артефакты фильтра `atempo`:** грубое ускорение всей аудиодорожки превращает речь в «скороговорку» и портит естественный тембр голоса.
3. **Избыточный расход ресурсов на Multi-take:** текущий `multitake` генерирует по 3 дубля для *каждой* фразы без исключения, замедляя экспорт в 3 раза.
4. **Ненужный шум:** синтетический процедурный шум вдоха (`breath_on`) засоряет паузы и создает ощущение фонового мусора.

---

## 2. Состав и этапы реализации

```
DubStudio/
├── DUBBING_TIMING_IMPROVEMENT_PLAN.md    [ЭТОТ ПЛАН]
├── crates/
│   ├── dub-translate/
│   │   └── src/
│   │       ├── translate.rs             (добавление расчета бюджета символов для батч-перевода)
│   │       └── ctx.rs                   (пресет стиля перевода «Кинодубляж» с лимитом длины)
│   ├── dub-server/
│   │   └── src/
│   │       ├── render.rs                (удаление breath, адаптивный multi-take, интеграция pause-squeeze)
│   │       ├── media.rs                 (алгоритм умного сжатия межсловных пауз squeeze_internal_pauses)
│   │       └── models.rs                (замена breath_on на pause_squeeze_on)
└── frontend/src/App.tsx                 (замена тумблера «Вставка дыханий» на «Сжатие пауз речи»)
```

---

## 3. Пошаговые технические задачи

### Этап 1. Удаление функционала дыхания (`breath_on`)
1. **В [crates/dub-server/src/render.rs](file:///d:/DubStudio/crates/dub-server/src/render.rs):**
   - Удалить функцию `generate_breath_sample`.
   - Удалить аргумент `breath_on: bool` из сигнатуры и вызова функции `timeline(...)`.
   - Убрать врезку сэмплов дыхания в цикле укладки дорожки.
2. **В [crates/dub-server/src/models.rs](file:///d:/DubStudio/crates/dub-server/src/models.rs):**
   - Удалить ключ `"breath_on"` из функции `is_selection_key`.
3. **Во [frontend/src/App.tsx](file:///d:/DubStudio/frontend/src/App.tsx):**
   - Удалить состояние `const [breathOn, setBreathOn] = useState(...)`.
   - Удалить JSX-блок тумблера «Вставка дыханий между фразами».

---

### Этап 2. Стиль перевода «Кинодубляж» (Length-Constrained Translation)
1. **Формула расчета бюджета:**
   Для каждого сегмента транскрипта рассчитывается жесткий лимит символов:
   $$\text{Бюджет символов} = \text{round}\Big((\text{s.end} - \text{s.start}) \times 13.0\Big)$$
2. **Инструкция для LLM в `dub-translate`:**
   В [crates/dub-translate/src/ctx.rs](file:///d:/DubStudio/crates/dub-translate/src/ctx.rs) и [translate.rs](file:///d:/DubStudio/crates/dub-translate/src/translate.rs) добавить поддержку пресета стиля `"dub_fit"` / `"Кинодубляж (укладка в хронометраж)"`:
   * В промпт каждой нумерованной строке передаётся маркер `(≤N симв)`.
   * Системный промпт инструктирует модель:
     *«Translate each numbered line strictly within the given character limit (≤N chars). Compress phrasing, drop filler and introductory words, keep core meaning and natural spoken flow. Do not output character count markers in the result.»*

---

### Этап 3. Адаптивный Multi-Take
1. **Модернизация логики в [crates/dub-server/src/render.rs](file:///d:/DubStudio/crates/dub-server/src/render.rs):**
   В секции `if multitake_on`:
   - Синтезируем **Take 1** (основной дубль).
   - Замеряем его фактическую длительность `dur = media::duration(&raw)`.
   - Вычисляем относительную погрешность $\delta = \frac{|\text{dur} - \text{target\_slot}|}{\text{target\_slot}}$.
   - **Условие раннего выхода:** если $\delta \le 0.10$ (попадание в пределы 10% от длины слота) и дефектов не обнаружено:
     - Принимаем Take 1 сразу;
     - Пропускаем вызовы Take 2 и Take 3;
     - Логируем: `сегмент {fi}: первый дубль идеален (отклонение {delta*100:.0}%) — пропуск доп. дублей`.
   - **Если $\delta > 0.10$:**
     - Генерируем Take 2 (`temp=0.25`) и Take 3 (`temp=0.35`);
     - Выбираем дубль с минимальным $|\text{dur} - \text{target\_slot}|$.

---

### Этап 4. Умное сжатие межсловных пауз (`pause_squeeze_on`)
1. **Алгоритм анализа пауз в [crates/dub-server/src/media.rs](file:///d:/DubStudio/crates/dub-server/src/media.rs):**
   Реализовать функцию `squeeze_internal_pauses(samples: &[f32], sr: u32, target_max_pause_ms: f64) -> Vec<f32>`:
   - Анализирует RMS окон по 10 мс.
   - Находит промежутки тишины ($< -35$ dB) между словами длительностью $> 70$ мс.
   - Сжимает их до $40$ мс с применением 5 мс сглаживающего кроссфейда.
   - Не затрагивает сами слова и гласные, сохраняя 100% тембр голоса.
2. **Интеграция в `fit_to_slot` ([render.rs](file:///d:/DubStudio/crates/dub-server/src/render.rs)):**
   - Перед вызовом фильтра `atempo` применяется `squeeze_internal_pauses`.
   - Если после сжатия пауз фраза уже уложилась в хронометраж ($0.98 \le \text{factor} \le 1.02$), `atempo` не вызывается вовсе.
3. **Настройки и UI:**
   - В `models.rs` добавить ключ `"pause_squeeze_on"` (по умолчанию `"1"`).
   - В `App.tsx` добавить тумблер **«Сжатие пауз речи»** с описанием *«сокращение тишины между словами перед ускорением — сохраняет естественный тембр»*.

---

## 4. Верификация и тесты

1. **Компиляция**: `cargo check --workspace` и `cargo test --workspace`.
2. **Тестовый прогон**: рендер `docs/example_original.mp4`:
   - Проверка сокращения среднего коэффициента ускорения `atempo` (доля ускоренных фраз должна упасть с ~40% до <10%).
   - Замер общего времени рендера с адаптивным Multi-take (ускорение в 2–2.5 раза по сравнению со старым Multi-take).
   - Проверка чистоты пауз на слух.

