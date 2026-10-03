import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion } from "motion/react";
import {
  AudioLines,
  Captions,
  Clock,
  Mic2,
  Scissors,
  Sparkles,
  Star,
  Timer,
  Volume2,
  Wand2,
} from "lucide-react";
import { api } from "../../lib/api";
import { useStore } from "../../store";
import SettingSwitch from "./SettingSwitch";

export default function QualitySection() {
  const { t } = useTranslation();
  const [qcAsr, setQcAsr] = useState(false);
  const [qcDur, setQcDur] = useState(true);
  const [multitake, setMultitake] = useState(false);
  const [speechRateOn, setSpeechRateOn] = useState(true);
  const [emoRefOn, setEmoRefOn] = useState(true);
  const [emoRefClean, setEmoRefClean] = useState(false);
  const [smartRefTrimOn, setSmartRefTrimOn] = useState(true);
  const [pauseSqueezeOn, setPauseSqueezeOn] = useState(true);
  const [voLeadIn, setVoLeadIn] = useState(true);
  const [dubReverbMatch, setDubReverbMatch] = useState(true);
  const [bench, setBench] = useState(false);
  const [busyKey, setBusyKey] = useState<string | null>(null);

  const autoCastOn = useStore((s) => s.autoCastOn);
  const setAutoCastOn = useStore((s) => s.setAutoCastOn);

  useEffect(() => {
    api.capabilities()
      .then((c) => {
        const s = c.selection ?? {};
        setBench(s.bench === "1");
        setQcAsr(s.qc_asr === "1");
        setQcDur(s.qc_duration !== "0");
        setMultitake(s.multitake === "1");
        setSpeechRateOn(s.speech_rate_on !== "0");
        setEmoRefOn(s.emo_ref_on !== "0");
        setEmoRefClean(s.emo_ref_clean === "1");
        setSmartRefTrimOn(s.smart_ref_trim !== "0");
        setPauseSqueezeOn(s.pause_squeeze_on !== "0");
        setVoLeadIn(s.vo_lead_in !== "0");
        setDubReverbMatch(s.dub_reverb_match !== "0");
        if (s.auto_cast_on !== undefined) {
          setAutoCastOn(s.auto_cast_on !== "0");
        }
      })
      .catch(() => {});
  }, [setAutoCastOn]);

  const toggle = async (key: string, current: boolean, setter: (v: boolean) => void) => {
    const next = !current;
    setter(next);
    setBusyKey(key);
    try {
      await api.setSelection(key, next ? "1" : "0");
    } catch {
      setter(current); // откат при ошибке
    } finally {
      setBusyKey(null);
    }
  };

  const groupTitle = "text-[11px] uppercase tracking-[0.14em] text-[var(--color-muted)] font-semibold mb-2";
  const groupCard = "rounded-xl border border-white/[0.08] divide-y divide-white/[0.06] bg-white/[0.035] overflow-hidden";

  return (
    <div className="max-w-3xl space-y-5">
      {/* 1. Контроль и сверка (QC) */}
      <section>
        <div className={groupTitle}>{t("qc.groupQc", "Контроль и сверка (QC)")}</div>
        <div className={groupCard}>
          <SettingSwitch
            icon={Captions}
            label={t("qc.asrTitle", "Проверка текста через ASR (QC)")}
            hint={t("qc.asrDesc", "авто-сверка озвучки через ASR (отключение ускоряет синтез)")}
            tip="Авто-проверка услышанного текста через Whisper ASR для отсечения тишины и дефектов"
            on={qcAsr}
            busy={busyKey === "qc_asr"}
            onToggle={() => toggle("qc_asr", qcAsr, setQcAsr)}
          />
          <SettingSwitch
            icon={Clock}
            label={t("qc.durationTitle", "Контроль длительности фраз (Stretch QC)")}
            hint={t("qc.durationDesc", "подгонка хронометража и максимального растяжения")}
            tip="Подгонка скорости и контроль хронометража аудио под рамки субтитра"
            on={qcDur}
            busy={busyKey === "qc_duration"}
            onToggle={() => toggle("qc_duration", qcDur, setQcDur)}
          />
          <SettingSwitch
            icon={Star}
            label={t("qc.multitakeTitle", "Multi-take отбор (3 дубля)")}
            hint={t("qc.multitakeDesc", "3 варианта озвучки — выбирается лучший по таймингу (медленнее, но качественнее)")}
            tip="Генерировать 3 варианта озвучки каждой фразы и автоматически выбирать лучший по таймингу"
            on={multitake}
            busy={busyKey === "multitake"}
            onToggle={() => toggle("multitake", multitake, setMultitake)}
          />
        </div>
      </section>

      {/* 2. Интонация и темп речи */}
      <section>
        <div className={groupTitle}>{t("qc.groupVoice", "Интонация и темп речи")}</div>
        <div className={groupCard}>
          <SettingSwitch
            icon={Sparkles}
            label={t("qc.speechRateTitle", "Динамический темп речи (Speech Rate TTS)")}
            hint={t("qc.speechRateDesc", "адаптация скорости выговора нейросети под длину текста в окне")}
            tip="Динамическая адаптация темпа генерации нейросети под длину текста и доступный временной слот"
            on={speechRateOn}
            busy={busyKey === "speech_rate_on"}
            onToggle={() => toggle("speech_rate_on", speechRateOn, setSpeechRateOn)}
          />
          <SettingSwitch
            icon={Mic2}
            label={t("qc.emoRefTitle", "Эмоциональный референс сцены (Emo-Ref)")}
            hint={t("qc.emoRefDesc", "копирование интонации, эмоции и подачи оригинала сцены")}
            tip="Перенос эмоций, интонации и подачи прямо из оригинального звука сцены"
            on={emoRefOn}
            busy={busyKey === "emo_ref_on"}
            onToggle={() => toggle("emo_ref_on", emoRefOn, setEmoRefOn)}
          />
          <AnimatePresence initial={false}>
            {emoRefOn && (
              <motion.div
                key="emo-ref-clean-wrapper"
                initial={{ height: 0, opacity: 0 }}
                animate={{ height: "auto", opacity: 1 }}
                exit={{ height: 0, opacity: 0 }}
                transition={{ duration: 0.22, ease: [0.22, 1, 0.36, 1] }}
                className="overflow-hidden !border-t-0"
              >
                <div className="border-t border-white/[0.06]">
                  <SettingSwitch
                    label={t("qc.emoRefCleanTitle", "Умный Emo-Ref (Smart Word-Trim)")}
                    hint={t("qc.emoRefCleanDesc", "чистый срез без вздохов, синхронизация текста при капе и охват восклицаний")}
                    tip="Отсекать предвдохи и фоновый шум по словам Whisper, синхронизировать текст референса при обрезке и сохранять экспрессию коротких восклицаний"
                    on={emoRefClean}
                    nested
                    busy={busyKey === "emo_ref_clean"}
                    onToggle={() => toggle("emo_ref_clean", emoRefClean, setEmoRefClean)}
                  />
                </div>
              </motion.div>
            )}
          </AnimatePresence>
          <SettingSwitch
            icon={Scissors}
            label={t("settings.smartRefTrim", "Умная нарезка по паузам")}
            hint={t("settings.smartRefTrimHint", "Поиск естественных пауз речи, без обрыва слов")}
            tip="Поиск естественных пауз речи при подготовке голосового референса, предотвращающий обрыв слов на полуслове"
            on={smartRefTrimOn}
            busy={busyKey === "smart_ref_trim"}
            onToggle={() => toggle("smart_ref_trim", smartRefTrimOn, setSmartRefTrimOn)}
          />
          <SettingSwitch
            icon={AudioLines}
            label={t("settings.pauseSqueeze", "Сжатие пауз речи")}
            hint={t("settings.pauseSqueezeDesc", "умное сжатие межсловных пауз перед ускорением")}
            tip={t("settings.pauseSqueezeHint")}
            on={pauseSqueezeOn}
            busy={busyKey === "pause_squeeze_on"}
            onToggle={() => toggle("pause_squeeze_on", pauseSqueezeOn, setPauseSqueezeOn)}
          />
        </div>
      </section>

      {/* 3. Звукорежиссура и мастеринг */}
      <section>
        <div className={groupTitle}>{t("qc.groupSound", "Звукорежиссура и мастеринг")}</div>
        <div className={groupCard}>
          <SettingSwitch
            icon={Volume2}
            label={t("settings.voLeadIn", "UN-Закадр: Золотая секунда")}
            hint={t("settings.voLeadInDesc", "задержка вступления диктора после оригинальной фразы")}
            tip={t("settings.voLeadInHint")}
            on={voLeadIn}
            busy={busyKey === "vo_lead_in"}
            onToggle={() => toggle("vo_lead_in", voLeadIn, setVoLeadIn)}
          />
          <SettingSwitch
            icon={Sparkles}
            label={t("settings.dubReverb", "Пространственная акустика (Early Reflections)")}
            hint={t("settings.dubReverbDesc", "легкое акустическое согласование пространства")}
            tip={t("settings.dubReverbHint")}
            on={dubReverbMatch}
            busy={busyKey === "dub_reverb_match"}
            onToggle={() => toggle("dub_reverb_match", dubReverbMatch, setDubReverbMatch)}
          />
          <SettingSwitch
            icon={Wand2}
            label={t("qc.autoCastTitle", "Автоподбор голосов (Auto-Cast)")}
            hint={t("qc.autoCastDesc", "автоматический подбор голосов из voices/ по тембру и полу персонажей")}
            tip="Автоматический подбор и распределение голосов из voices/ по тембру и полу персонажей"
            on={autoCastOn}
            busy={busyKey === "auto_cast_on"}
            onToggle={() => {
              const v = !autoCastOn;
              setAutoCastOn(v);
              api.setSelection("auto_cast_on", v ? "1" : "0").catch(() => {});
            }}
          />
        </div>
      </section>

      {/* 4. Диагностика */}
      <section>
        <div className={groupTitle}>{t("qc.groupDiag", "Диагностика")}</div>
        <div className={groupCard}>
          <SettingSwitch
            icon={Timer}
            label={t("settings.bench", "Пер-стадийный бенчмарк")}
            hint={t("settings.benchHint", "замер времени каждой стадии (bench.json + журнал)")}
            tip={t("settings.benchHint")}
            on={bench}
            busy={busyKey === "bench"}
            onToggle={() => toggle("bench", bench, setBench)}
          />
        </div>
      </section>
    </div>
  );
}
