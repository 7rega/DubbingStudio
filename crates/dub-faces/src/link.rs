//! Связка лицо (кластер персонажа) ↔ голос (спикер диаризации Sortformer).
//!
//! Метод (best practices ASD-lite): для каждой реплики спикера смотрим, какие лица-кластеры были в кадре
//! в её интервале (со-встречаемость). Матрица M[cluster][speaker] += вес присутствия (доля кадров лица
//! в интервале реплики). Опц. domination-скор LR-ASD добавляется, если движок доступен (asd.rs; сейчас
//! фолбэк — только со-встречаемость). Назначение cluster->speaker детерминированным жадным argmax по
//! убыванию веса (устойчивый tie-break по индексам). Закадровые спикеры (лица нет) остаются без лица.

use std::collections::HashMap;

/// Реплика спикера: интервал + id спикера (строка, как в project.json segments[].speaker).
#[derive(Clone, Debug)]
pub struct SpeakerTurn {
    pub start: f64,
    pub end: f64,
    pub speaker: String,
}

/// Результат связки: cluster_idx (индекс FaceCluster) -> speaker_id (строка). Спикеры без лица не
/// попадают сюда (чисто голосовые/закадровые). Кластеры без спикера — тоже (лицо без реплик рядом).
pub type Linkage = HashMap<usize, String>;

/// Присутствие лица кластера в момент t (в секундах) по временам его кадров-членов. Здесь передаём
/// заранее собранные времена кадров каждого кластера.
///
/// clusters_frame_times[c] = отсортированные времена (сек) кадров, где встречен кластер c.
/// turns — реплики спикеров. cooc_window — допуск по времени привязки кадра к реплике (сек).
pub fn cooccurrence_matrix(
    clusters_frame_times: &[Vec<f64>],
    turns: &[SpeakerTurn],
    speakers: &[String],
) -> Vec<Vec<f64>> {
    let nc = clusters_frame_times.len();
    let ns = speakers.len();
    let mut m = vec![vec![0.0f64; ns]; nc];
    let spk_idx: HashMap<&str, usize> =
        speakers.iter().enumerate().map(|(i, s)| (s.as_str(), i)).collect();
    for turn in turns {
        let Some(&si) = spk_idx.get(turn.speaker.as_str()) else {
            continue;
        };
        let dur = (turn.end - turn.start).max(1e-6);
        for (c, times) in clusters_frame_times.iter().enumerate() {
            // times ОТСОРТИРОВАНЫ (lib.rs) -> число кадров в [start,end] через две бинарные границы (O(log F)),
            // а не линейным сканом (было O(F) на пару кластер×реплика). partition_point возвращает индекс
            // первого элемента, не удовлетворяющего предикату (для сорт. — граница диапазона).
            let lo = times.partition_point(|&t| t < turn.start); // первый t >= start
            let hi = times.partition_point(|&t| t <= turn.end); // первый t > end
            let hits = hi.saturating_sub(lo);
            if hits > 0 {
                // вес = сумма присутствий, взвешенная длительностью реплики (долгая реплика с лицом важнее).
                m[c][si] += hits as f64 * dur;
            }
        }
    }
    m
}

/// Жадное детерминированное назначение cluster->speaker по матрице со-встречаемости. Каждый спикер и
/// каждый кластер используются максимум один раз (взаимно-однозначно, как Hungarian-argmax). Нулевые
/// пары не назначаются. Tie-break: больший вес; при равенстве — меньший (cluster, speaker) индекс.
pub fn assign(matrix: &[Vec<f64>], speakers: &[String]) -> Linkage {
    let nc = matrix.len();
    let ns = speakers.len();
    let mut used_c = vec![false; nc];
    let mut used_s = vec![false; ns];
    let mut out: Linkage = HashMap::new();
    // Собираем все пары (вес, c, s) и сортируем детерминированно.
    let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
    for c in 0..nc {
        for s in 0..ns {
            if matrix[c][s] > 0.0 {
                pairs.push((matrix[c][s], c, s));
            }
        }
    }
    pairs.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
    });
    for (_, c, s) in pairs {
        if !used_c[c] && !used_s[s] {
            used_c[c] = true;
            used_s[s] = true;
            out.insert(c, speakers[s].clone());
        }
    }
    out
}

/// ДИСКРИМИНАТИВНЫЙ score из матрицы со-встречаемости `m` (клетка c,s), с пер-кластерным весом `prominence`
/// (передний план: крупнее/фронтальнее => важнее). score[c][s] = (M[c][s]² / Σ_s' M[c][s']) · prominence[c].
/// Первый множитель — эксклюзивность (лицо у МНОГИХ спикеров размазывает M -> мал для каждого; лицо своего
/// спикера -> score ≈ M); второй — приоритет говорящего в кадре над фоновым. `prominence` длиной = числу
/// строк m (кластеров); пусто/короче -> вес 1.0. ЕДИНЫЙ источник формулы для теста и прод-пути
/// (faces_to_speakers). TF/exclusivity-взвешивание из ASD/диаризация-fusion (MERL/AVA-AVD), без моделей.
pub fn discriminative_prominence_score(m: &[Vec<f64>], prominence: &[f64]) -> Vec<Vec<f64>> {
    m.iter()
        .enumerate()
        .map(|(c, row)| {
            let total: f64 = row.iter().sum();
            let prom = prominence.get(c).copied().unwrap_or(1.0);
            row.iter().map(|&v| if total > 1e-9 { (v * v / total) * prom } else { 0.0 }).collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(s: f64, e: f64, spk: &str) -> SpeakerTurn {
        SpeakerTurn { start: s, end: e, speaker: spk.to_string() }
    }

    // Тестовые обёртки над прод-примитивами: сырая со-встречаемость и дискриминативная.
    fn cooc_link(times: &[Vec<f64>], turns: &[SpeakerTurn], speakers: &[String]) -> Linkage {
        assign(&cooccurrence_matrix(times, turns, speakers), speakers)
    }
    fn disc_link(times: &[Vec<f64>], turns: &[SpeakerTurn], speakers: &[String]) -> Linkage {
        assign(&discriminative_prominence_score(&cooccurrence_matrix(times, turns, speakers), &[]), speakers)
    }

    #[test]
    fn links_face_to_cooccurring_speaker() {
        // Кластер 0 в кадре во время реплик спикера "0"; кластер 1 — во время "1".
        let times = vec![vec![0.5, 1.0, 1.5], vec![5.5, 6.0]];
        let turns = vec![turn(0.0, 2.0, "0"), turn(5.0, 7.0, "1")];
        let speakers = vec!["0".to_string(), "1".to_string()];
        let link = cooc_link(&times, &turns, &speakers);
        assert_eq!(link.get(&0), Some(&"0".to_string()));
        assert_eq!(link.get(&1), Some(&"1".to_string()));
    }

    #[test]
    fn offscreen_speaker_unlinked() {
        // Спикер "2" говорит, но ни один кластер не в кадре в его интервал -> не назначен.
        let times = vec![vec![0.5]];
        let turns = vec![turn(0.0, 1.0, "0"), turn(10.0, 12.0, "2")];
        let speakers = vec!["0".to_string(), "2".to_string()];
        let link = cooc_link(&times, &turns, &speakers);
        assert_eq!(link.get(&0), Some(&"0".to_string()));
        assert!(!link.values().any(|v| v == "2"), "закадровый спикер без лица");
    }

    #[test]
    fn one_to_one_no_double_assign() {
        // Оба кластера ко-встречаются с обоими спикерами, но сильнее по диагонали.
        let m = vec![vec![10.0, 3.0], vec![2.0, 8.0]];
        let speakers = vec!["a".to_string(), "b".to_string()];
        let link = assign(&m, &speakers);
        assert_eq!(link.get(&0), Some(&"a".to_string()));
        assert_eq!(link.get(&1), Some(&"b".to_string()));
    }

    #[test]
    fn empty_all_empty() {
        let link = cooc_link(&[], &[], &[]);
        assert!(link.is_empty());
    }

    #[test]
    fn discriminative_beats_bystander() {
        // c0 — «слушатель»/статист: РАВНОМЕРНО в кадре у ОБОИХ спикеров, много (по 4 кадра/реплику).
        // c1 — лицо спикера 0 (эксклюзивно, но меньше кадров), c2 — лицо спикера 1.
        let times = vec![
            vec![0.2, 0.5, 0.8, 1.2, 5.2, 5.5, 5.8, 6.2], // c0: 4 в реплике s0, 4 в s1
            vec![0.3, 0.6, 0.9],                          // c1: только в реплике s0
            vec![5.3, 5.6, 5.9],                          // c2: только в реплике s1
        ];
        let turns = vec![turn(0.0, 2.0, "0"), turn(5.0, 7.0, "1")];
        let speakers = vec!["0".to_string(), "1".to_string()];
        // Сырая со-встречаемость назначает слушателя (c0) спикеру — он в кадре больше всех.
        let raw = cooc_link(&times, &turns, &speakers);
        assert_eq!(raw.get(&0), Some(&"0".to_string()), "сырое: слушатель c0 забирает спикера");
        // Дискриминативная — отдаёт спикерам их ЭКСКЛЮЗИВНЫЕ лица, слушатель без спикера.
        let disc = disc_link(&times, &turns, &speakers);
        assert_eq!(disc.get(&1), Some(&"0".to_string()), "диск: c1 -> спикер 0");
        assert_eq!(disc.get(&2), Some(&"1".to_string()), "диск: c2 -> спикер 1");
        assert!(!disc.contains_key(&0), "диск: слушатель c0 не назначен");
    }
}
