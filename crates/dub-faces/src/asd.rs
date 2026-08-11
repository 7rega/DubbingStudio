//! LR-ASD (Lightweight and Robust Active Speaker Detection, IJCV 2025) — active-speaker скор на трек лица.
//!
//! СТАТУС: официального ONNX НЕТ (репозиторий Junhua-Liao/LR-ASD отдаёт только PyTorch .model). Для
//! Rust/ort его надо экспортировать самому (torch.onnx.export двух-башенной ASD_Model: visual
//! (1,T,112,112) + audio (1,4T,13) -> (T,2), dynamic_axes на T, opset 17+). Скачанный pretrain_AVA.model
//! лежит в models/faces/lr-asd/, но .onnx на момент сборки не создан.
//!
//! ПОЭТОМУ AsdScorer здесь — ГЕЙТ (как NullEmbedder в dub-asr #83): пока lr_asd.onnx нет, is_available()
//! = false и score() возвращает None, а связка лицо↔голос (link.rs) работает ТОЛЬКО на со-встречаемости
//! «лицо в кадре во время реплик спикера». Это допустимый и документированный фолбэк по ТЗ. Как только
//! lr_asd.onnx появится (экспорт пользователем), тут подключается ort-сессия + MFCC-препроцесс без
//! изменения контракта link.rs (score() начнёт отдавать Some, argmax станет точнее в многолицевых кадрах).

use std::path::Path;

/// Оценщик active-speaker для трека лица. Заглушка до экспорта LR-ASD в ONNX.
pub struct AsdScorer {
    available: bool,
}

impl AsdScorer {
    /// Загрузить из lr_asd.onnx, если он есть. None/отсутствует -> недоступен (фолбэк со-встречаемости).
    pub fn load(onnx: Option<&Path>) -> Self {
        // Экспортированный ONNX ещё не поддержан в этой сборке (нет официального файла). Даже если путь
        // указывает на существующий .onnx — держим available=false до реализации MFCC-препроцесса и
        // T:4T-инференса, чтобы не отдавать мусорные скоры. Готовый гейт под будущее подключение.
        let _ = onnx;
        AsdScorer { available: false }
    }

    /// Доступен ли движок (ONNX подключён и препроцесс реализован). Пока всегда false.
    pub fn is_available(&self) -> bool {
        self.available
    }

    /// Speaking-скор [0..1] для трека лица (T кадров) на отрезке аудио. None = движок недоступен ->
    /// вызывающий откатывается на со-встречаемость. Сигнатура-задел под будущий инференс.
    pub fn score(&self, _face_crops: &[image::RgbImage], _audio_16k: &[f32]) -> Option<f32> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scorer_unavailable_by_default() {
        let s = AsdScorer::load(None);
        assert!(!s.is_available());
        assert!(s.score(&[], &[]).is_none());
    }

    #[test]
    fn scorer_unavailable_even_with_path() {
        // Даже с путём — гейт закрыт (нет реализации препроцесса).
        let s = AsdScorer::load(Some(Path::new("some/lr_asd.onnx")));
        assert!(!s.is_available());
    }
}
