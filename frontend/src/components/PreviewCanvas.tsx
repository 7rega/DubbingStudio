// PreviewCanvas — the editing heart with Pure HTML5 Video & Unified WYSIWYG Subtitle Overlay:
// 1. Аппаратный видеоплеер (<video>) работает непрерывно (и на плее, и на паузе, и при скраббинге) с 60 FPS
//    и нулевыми задержками.
// 2. Единый оверлей субтитров (DOM Overlay) со всеми 26 пресетами, пословным караоке и динамическим блюром
//    отображается в любом состоянии плеера (0% мерцания, 100% стабильность шрифтов).
// 3. На паузе поверх субтитров отображаются направляющая линия высоты и рамки масок блюра/титров (Konva).
import { useEffect, useRef, useState } from "react";
import { Stage, Layer, Rect, Line, Transformer } from "react-konva";
import type Konva from "konva";
import { api, type Project, type SubStyle } from "../lib/api";
import { useStore } from "../store";

type Lane = "subs" | "blur" | "titles";
type Props = {
  pid: string;
  project: Project;
  scrub: number;
  rendered: boolean;
  lane: Lane;
  playing?: boolean;
  onChanged: (fresh: Project) => void;
};

// Каталог 26 пресетов субтитров (синхронизирован с backend dub-captions / look.rs)
const PRESET_LOOKS: Record<
  string,
  {
    font: string;
    color: string;
    plate: "box" | "pill" | "rounded" | "card" | "glow" | "blob" | "soft" | "none";
    plate_c: string;
    accent?: string;
    bold?: boolean;
    uppercase?: boolean;
    reveal?: "highlight" | "pop" | "karaoke" | "word" | "whole";
  }
> = {
  clean: { font: "Montserrat", color: "#FFFFFF", plate: "pill", plate_c: "#1A1A1A", reveal: "whole" },
  minimal: { font: "Roboto", color: "#FFFFFF", plate: "rounded", plate_c: "#181818", reveal: "whole" },
  boxed: { font: "Montserrat", color: "#FFFFFF", plate: "box", plate_c: "#101010", reveal: "whole" },
  headline: { font: "Oswald", color: "#FFFFFF", plate: "box", plate_c: "#0C0C0C", uppercase: true, reveal: "whole" },
  serif: { font: "Playfair Display", color: "#FFFFFF", plate: "card", plate_c: "#16110D", reveal: "whole" },
  card: { font: "Montserrat", color: "#FFFFFF", plate: "card", plate_c: "#16110D", reveal: "whole" },
  hormozi: { font: "Russo One", color: "#FFFFFF", plate: "box", plate_c: "#0C0C0C", accent: "#FFD400", bold: true, uppercase: true, reveal: "highlight" },
  hormozi_green: { font: "Russo One", color: "#FFFFFF", plate: "box", plate_c: "#0C0C0C", accent: "#28E0A8", bold: true, uppercase: true, reveal: "highlight" },
  mrbeast: { font: "Russo One", color: "#FFFFFF", plate: "box", plate_c: "#0C0C0C", accent: "#FFE000", bold: true, uppercase: true, reveal: "pop" },
  impact: { font: "Russo One", color: "#FFFFFF", plate: "box", plate_c: "#101010", accent: "#FF3B30", bold: true, uppercase: true, reveal: "highlight" },
  pop: { font: "Oswald", color: "#FFFFFF", plate: "pill", plate_c: "#141414", bold: true, uppercase: true, reveal: "pop" },
  karaoke: { font: "Oswald", color: "#FFFFFF", plate: "pill", plate_c: "#181818", accent: "#28E0A8", bold: true, reveal: "karaoke" },
  karaoke_gold: { font: "Montserrat", color: "#FFFFFF", plate: "box", plate_c: "#101010", accent: "#FFD400", bold: true, reveal: "karaoke" },
  karaoke_neon: { font: "Montserrat", color: "#FFFFFF", plate: "glow", plate_c: "#0A0A14", accent: "#00E5FF", bold: true, reveal: "karaoke" },
  bubble: { font: "Caveat", color: "#201018", plate: "blob", plate_c: "#FF5DA2", bold: true, reveal: "whole" },
  bubble_pop: { font: "Pacifico", color: "#201018", plate: "blob", plate_c: "#FFC857", reveal: "pop" },
  candy: { font: "Pacifico", color: "#2A0E1E", plate: "pill", plate_c: "#FF6FB5", reveal: "word" },
  neon: { font: "Montserrat", color: "#00E5FF", plate: "glow", plate_c: "#0A0A14", accent: "#00E5FF", bold: true, reveal: "whole" },
  neon_pink: { font: "Oswald", color: "#FF54C8", plate: "glow", plate_c: "#100A14", accent: "#FF54C8", bold: true, reveal: "whole" },
  cyber: { font: "Oswald", color: "#7DF9FF", plate: "glow", plate_c: "#07101A", accent: "#00E5FF", bold: true, reveal: "word" },
  fresh: { font: "Montserrat", color: "#FFFFFF", plate: "none", plate_c: "transparent", reveal: "whole" },
  fresh_bold: { font: "Russo One", color: "#FFFFFF", plate: "none", plate_c: "transparent", bold: true, reveal: "pop" },
  fresh_pop: { font: "Montserrat", color: "#FFFFFF", plate: "none", plate_c: "transparent", bold: true, reveal: "pop" },
  fresh_karaoke: { font: "Oswald", color: "#FFFFFF", plate: "none", plate_c: "transparent", accent: "#FFD400", bold: true, reveal: "karaoke" },
  fresh_hormozi: { font: "Russo One", color: "#FFFFFF", plate: "none", plate_c: "transparent", accent: "#FFD400", bold: true, uppercase: true, reveal: "highlight" },
  fresh_soft: { font: "Montserrat", color: "#FFFFFF", plate: "soft", plate_c: "transparent", reveal: "whole" },
};

interface WordTiming {
  word: string;
  start: number;
  end: number;
}

const getWordsWithTimings = (seg: Project["segments"][number], customText?: string): WordTiming[] => {
  const rawText = (customText ?? seg.tgt_text ?? seg.src_text ?? "").trim();
  if (!rawText) return [];
  const tokens = rawText.split(/\s+/).filter(Boolean);
  if (tokens.length === 0) return [];

  const extraWords = (seg as unknown as { extra?: { words?: Array<{ word: string; start: number; end: number }> } }).extra?.words;
  if (!customText && extraWords && extraWords.length === tokens.length) {
    return tokens.map((w, i) => ({
      word: w,
      start: extraWords[i].start,
      end: extraWords[i].end,
    }));
  }

  // Пропорциональный расчет времени слов внутри фразы
  const segDur = Math.max(0.1, seg.end - seg.start);
  const totalChars = tokens.reduce((sum, t) => sum + t.length, 0) || 1;
  let curTime = seg.start;
  return tokens.map((w) => {
    const wDur = (w.length / totalChars) * segDur;
    const st = curTime;
    const en = curTime + wDur;
    curTime = en;
    return { word: w, start: st, end: en };
  });
};

export default function PreviewCanvas({ pid, project, scrub, rendered, lane, playing = false, onChanged }: Props) {
  const rev = useStore((s) => s.rev);
  const bump = useStore((s) => s.bump);
  const sel = useStore((s) => s.selBlur);        // SHARED with the left blur list
  const setSel = useStore((s) => s.setSelBlur);
  const selT = useStore((s) => s.selTitle);      // SHARED with the left titles list
  const setSelT = useStore((s) => s.setSelTitle);
  const wrap = useRef<HTMLDivElement>(null);
  const videoRef = useRef<HTMLVideoElement>(null);
  const trRef = useRef<Konva.Transformer>(null);
  const boxRefs = useRef<Record<number, Konva.Rect>>({});
  const titleRefs = useRef<Record<number, Konva.Rect>>({});
  const [disp, setDisp] = useState({ w: 0, h: 0 });
  const [guide, setGuide] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);

  const vw = project.meta.width || 1, vh = project.meta.height || 1;
  const sx = disp.w / vw, sy = disp.h / vh;
  const previewSrc = rendered ? api.outputUrl(pid) : api.previewUrl(pid, scrub, rev);

  // Синхронизация нативного видеоплеера при плее/паузе/перемотке (Pure HTML5 Video)
  useEffect(() => {
    const v = videoRef.current;
    if (!v) return;
    if (playing) {
      v.muted = true;
      if (Math.abs(v.currentTime - scrub) > 0.1) {
        v.currentTime = scrub;
      }
      const playPromise = v.play();
      if (playPromise !== undefined) {
        playPromise.catch((err) => {
          console.warn("Video playback start:", err);
        });
      }
    } else {
      v.pause();
      if (Math.abs(v.currentTime - scrub) > 0.04) {
        v.currentTime = scrub;
      }
    }
  }, [playing]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    const v = videoRef.current;
    if (!v) return;
    if (!playing && Math.abs(v.currentTime - scrub) > 0.04) {
      v.currentTime = scrub;
    } else if (playing && Math.abs(v.currentTime - scrub) > 0.25) {
      v.currentTime = scrub;
    }
  }, [scrub, playing]);

  // Масштабирование холста под контейнер с сохранением соотношения сторон
  useEffect(() => {
    const el = wrap.current; if (!el) return;
    const measure = () => {
      const cw = el.clientWidth, ch = el.clientHeight;
      if (cw <= 0 || ch <= 0) return;
      const scale = Math.min(cw / vw, ch / vh);
      setDisp({ w: Math.round(vw * scale), h: Math.round(vh * scale) });
    };
    const ro = new ResizeObserver(measure);
    ro.observe(el); measure();
    return () => ro.disconnect();
  }, [vw, vh]);

  // Присоединение трансформера Konva к активному объекту
  useEffect(() => {
    let node: Konva.Rect | null = null;
    if (lane === "blur" && sel != null) {
      const hidden = (project.captions.blur_boxes || [])[sel]?.hidden;
      if (!hidden) node = boxRefs.current[sel] ?? null;
    } else if (lane === "titles" && selT != null) {
      node = titleRefs.current[selT] ?? null;
    }
    if (node && node.getLayer() && trRef.current) {
      trRef.current.nodes([node]); trRef.current.getLayer()?.batchDraw();
    } else trRef.current?.nodes([]);
  }, [sel, selT, lane, disp, project]);

  useEffect(() => { setGuide(null); }, [scrub, lane]);

  async function patch(edit: Record<string, unknown>) {
    setBusy(true);
    try { const fresh = await api.patch(pid, edit); onChanged(fresh); bump(); } finally { setBusy(false); }
  }

  const blurs = project.captions.blur_boxes || [];
  const titles = project.captions.titles || [];
  const subY = project.captions.sub_y ?? Math.round(vh * 0.82);
  const ss: Partial<SubStyle> = project.captions.sub_style || {};

  // Пресет стилей субтитров
  const presetName = String(project.captions.preset?.name || "");
  const preset = PRESET_LOOKS[presetName] || null;

  const centerGuide = (nx: number, wPx: number) =>
    setGuide(Math.abs(nx + wPx / 2 - disp.w / 2) < 8 ? disp.w / 2 : null);

  const readRect = (n: Konva.Node): { x: number; y: number; w: number; h: number } | null => {
    if (!sx || !sy) { n.scaleX(1); n.scaleY(1); return null; }
    const w = Math.round((n.width() * n.scaleX()) / sx);
    const h = Math.round((n.height() * n.scaleY()) / sy);
    n.scaleX(1); n.scaleY(1);
    return { x: Math.round(n.x() / sx), y: Math.round(n.y() / sy), w, h };
  };

  // Определение текста субтитров в зависимости от режима p.subs.mode ("translate" | "transcribe" | "none") и p.subs.burn
  const subsBurnOn = project.subs.burn !== false;
  const subsMode = project.subs.mode || "translate";
  const activeRawSeg = (project.segments || []).find((s) => !s.hidden && scrub >= s.start && scrub <= s.end);

  const activeSegText = (() => {
    if (!subsBurnOn || subsMode === "none" || !activeRawSeg) return null;
    if (subsMode === "transcribe") return (activeRawSeg.src_text || "").trim();
    if (subsMode === "translate") return (activeRawSeg.tgt_text || activeRawSeg.src_text || "").trim();
    return (activeRawSeg.tgt_text || activeRawSeg.src_text || "").trim();
  })();

  const activeTitles = (project.captions.titles || []).filter((ti) => scrub >= ti.start && scrub <= ti.end);
  const activeBlurs = project.render.blur
    ? (project.captions.blur_boxes || []).filter((b) => !b.hidden && scrub >= b.t0 && scrub <= b.t1)
    : [];

  const effectiveFont = preset?.font || ss.font || "Montserrat";
  const effectiveColor = preset?.color || ss.color || "#FFFFFF";
  const effectiveBold = preset?.bold ?? ss.bold;
  const effectiveUppercase = preset?.uppercase ?? ss.uppercase;
  const hasPlate = preset ? preset.plate !== "none" : !!ss.plate;
  const plateColor = preset?.plate_c || ss.plate_color || "rgba(0,0,0,0.75)";
  const plateType = preset?.plate || (ss.plate ? "box" : "none");

  // Точный расчет размера шрифта, 1-в-1 синхронизированный с libass (dub_captions)
  const explicitSize = ss.size_px && ss.size_px > 0 ? ss.size_px : null;
  const h5 = Math.round(vh / 5);
  const h10 = Math.round(vh / 10);
  const baseSubFs = explicitSize
    ? Math.max(20, Math.min(explicitSize, h5))
    : Math.min(h10, Math.max(44, Math.round(vh / 16)));
  const subFontSize = Math.max(10, baseSubFs * sy * 0.72);

  // Динамический расчет высоты авто-блюра под количество строк текста
  const lineCount = activeSegText
    ? Math.max(1, Math.ceil(activeSegText.length / 42) + (activeSegText.includes("\n") ? 1 : 0))
    : 1;
  const autoBlurH = Math.max(subFontSize * 1.85, lineCount * subFontSize * 1.35 + 12 * sy);

  const isOriginalPreset = !preset || presetName === "original" || presetName === "match";
  const shouldRenderAutoSubBlur = project.render.blur && isOriginalPreset && !!activeSegText;

  const textShadowCSS = (() => {
    const parts: string[] = [];
    if (preset?.plate === "glow" && preset.accent) {
      parts.push(`0 0 10px ${preset.accent}`, `0 0 20px ${preset.accent}`, `0 0 35px ${preset.accent}`);
    } else {
      const ow = Math.max(1, (ss.outline_w ?? 2) * sy * 0.72);
      const oc = ss.outline || "#000000";
      if (ow > 0) {
        parts.push(
          `-${ow}px -${ow}px 0 ${oc}`,
          `${ow}px -${ow}px 0 ${oc}`,
          `-${ow}px ${ow}px 0 ${oc}`,
          `${ow}px ${ow}px 0 ${oc}`,
          `0px -${ow}px 0 ${oc}`,
          `0px ${ow}px 0 ${oc}`,
          `-${ow}px 0px 0 ${oc}`,
          `${ow}px 0px 0 ${oc}`
        );
      }
      if (ss.shadow_dir != null) {
        const sd = Math.max(2, 2.5 * sy);
        parts.push(`${sd}px ${sd}px 3px rgba(0,0,0,0.85)`);
      }
    }
    return parts.join(", ");
  })();

  const plateBorderRadius = (() => {
    if (plateType === "pill") return "9999px";
    if (plateType === "rounded" || plateType === "card" || plateType === "blob") return `${6 * sy}px`;
    if (plateType === "box") return `${3 * sy}px`;
    return `${3 * sy}px`;
  })();

  return (
    <div ref={wrap} className="relative w-full h-full min-h-0 overflow-hidden grid place-items-center bg-black/40 rounded-xl">
      <div className="relative overflow-hidden rounded-lg" style={{ width: disp.w, height: disp.h }}>
        {rendered ? (
          <video src={previewSrc} controls className="absolute inset-0 w-full h-full rounded-lg" />
        ) : (
          <>
            {/* Аппаратный видеоплеер (Pure HTML5 Video: активен и на плее, и на паузе, и при скраббинге) */}
            <video
              ref={videoRef}
              src={api.sourceVideoUrl(pid)}
              playsInline
              muted
              preload="auto"
              className="absolute inset-0 w-full h-full rounded-lg object-contain bg-black"
            />

            {/* Единый оверлей субтитров/титров/блюра (активен всегда: 0% мерцания, 100% стабильность шрифта) */}
            {disp.w > 0 && (
              <div className="absolute inset-0 pointer-events-none overflow-hidden z-10">
                {/* Автоматическое размытие оригинальных субтитров при p.render.blur (только для оригинального пресета) */}
                {shouldRenderAutoSubBlur && (
                  <div
                    style={{
                      position: "absolute",
                      left: `${disp.w * 0.05}px`,
                      top: `${Math.max(0, subY * sy - autoBlurH * 0.45)}px`,
                      width: `${disp.w * 0.9}px`,
                      height: `${autoBlurH}px`,
                      backdropFilter: `blur(${Math.max(8, (project.render.blur_sigma || 60) * 0.35)}px) brightness(0.75)`,
                      backgroundColor: "rgba(0, 0, 0, 0.55)",
                      borderRadius: `${8 * sy}px`,
                      boxShadow: "0 4px 20px rgba(0,0,0,0.5)",
                    }}
                  />
                )}

                {/* Пользовательские зоны размытия (блюр-боксы) */}
                {activeBlurs.map((b, i) => (
                  <div
                    key={`live-blur-${i}`}
                    style={{
                      position: "absolute",
                      left: `${b.x * sx}px`,
                      top: `${b.y * sy}px`,
                      width: `${b.w * sx}px`,
                      height: `${b.h * sy}px`,
                      backgroundColor: b.fill || "rgba(0,0,0,0.45)",
                      backdropFilter: b.fill ? undefined : `blur(${Math.max(6, (project.render.blur_sigma || 60) * 0.25)}px)`,
                      borderRadius: `${4 * sx}px`,
                    }}
                  />
                ))}

                {/* Титры */}
                {activeTitles.map((ti, i) => {
                  const [bx, by, bw, bh] = (ti.bbox || [0, 0, vw, 60]) as number[];
                  const tSize = (ti.size_px || Math.round(vh / 18)) * sy;
                  return (
                    <div
                      key={`live-title-${i}`}
                      style={{
                        position: "absolute",
                        left: `${bx * sx}px`,
                        top: `${by * sy}px`,
                        width: `${bw * sx}px`,
                        height: `${bh * sy}px`,
                        display: "flex",
                        alignItems: "center",
                        justifyContent: ti.align === "left" ? "flex-start" : ti.align === "right" ? "flex-end" : "center",
                        textAlign: (ti.align || "center") as React.CSSProperties["textAlign"],
                        fontSize: `${tSize}px`,
                        fontFamily: ti.font || effectiveFont,
                        fontWeight: ti.bold ? "bold" : "normal",
                        fontStyle: ti.italic ? "italic" : "normal",
                        textTransform: ti.uppercase ? "uppercase" : "none",
                        color: ti.color || "#FFFFFF",
                        textShadow: ti.outline ? `0 0 ${ti.outline_w || 2}px ${ti.outline}` : undefined,
                      }}
                    >
                      {ti.tgt || ti.text}
                    </div>
                  );
                })}

                {/* Основные субтитры с пословной караоке-анимацией */}
                {activeSegText && activeRawSeg && (() => {
                  const words = getWordsWithTimings(activeRawSeg, activeSegText);
                  const revealType = preset?.reveal || "whole";

                  return (
                    <div
                      style={{
                        position: "absolute",
                        left: 0,
                        top: `${Math.max(0, Math.min(disp.h - subFontSize * 1.5, subY * sy - subFontSize * 0.65))}px`,
                        width: `${disp.w}px`,
                        textAlign: (ss.align || "center") as React.CSSProperties["textAlign"],
                        fontSize: `${subFontSize}px`,
                        fontFamily: `"${effectiveFont}", "Montserrat", sans-serif`,
                        fontWeight: effectiveBold ? 800 : 600,
                        fontStyle: ss.italic ? "italic" : "normal",
                        textTransform: effectiveUppercase ? "uppercase" : "none",
                        color: effectiveColor,
                        textShadow: textShadowCSS,
                        lineHeight: 1.12,
                        padding: `0 ${16 * sx}px`,
                      }}
                    >
                      <span
                        style={{
                          backgroundColor: hasPlate ? plateColor : undefined,
                          padding: hasPlate ? `${Math.max(2, Math.round(3 * sy))}px ${Math.max(5, Math.round(10 * sx))}px` : undefined,
                          borderRadius: hasPlate ? plateBorderRadius : undefined,
                          boxDecorationBreak: "clone",
                          WebkitBoxDecorationBreak: "clone",
                          display: "inline-flex",
                          flexWrap: "wrap",
                          justifyContent: "center",
                          alignItems: "center",
                          maxWidth: "92%",
                          backdropFilter: hasPlate && plateType === "soft" ? "blur(8px)" : undefined,
                          boxShadow: hasPlate && plateColor !== "transparent" ? "0 2px 10px rgba(0,0,0,0.35)" : undefined,
                        }}
                      >
                        {words.map((w, idx) => {
                          const isCurrent = scrub >= w.start && scrub < w.end;
                          const isPast = scrub >= w.end;
                          const isFuture = scrub < w.start;

                          let wordColor = effectiveColor;
                          let wordTransform = "none";
                          let wordOpacity = 1;
                          let wordTextShadow = textShadowCSS;

                          if (revealType === "highlight") {
                            if (isCurrent) {
                              wordColor = preset?.accent || "#FFD400";
                              wordTransform = "scale(1.08)";
                              wordTextShadow = `0 0 14px ${preset?.accent || "#FFD400"}, ${textShadowCSS}`;
                            }
                          } else if (revealType === "pop") {
                            if (isCurrent) {
                              wordColor = preset?.accent || "#FFE000";
                              wordTransform = "scale(1.15) translateY(-2px)";
                            }
                          } else if (revealType === "karaoke") {
                            if (isCurrent || isPast) {
                              wordColor = preset?.accent || "#28E0A8";
                            } else {
                              wordOpacity = 0.6;
                            }
                          } else if (revealType === "word") {
                            if (isFuture) {
                              wordOpacity = 0;
                            } else if (isCurrent) {
                              wordColor = preset?.accent || "#00E5FF";
                            }
                          }

                          return (
                            <span
                              key={idx}
                              style={{
                                display: "inline-block",
                                color: wordColor,
                                transform: wordTransform,
                                opacity: wordOpacity,
                                textShadow: wordTextShadow,
                                transition: "transform 0.08s ease-out, color 0.08s ease-out, opacity 0.08s ease-out",
                                marginRight: idx < words.length - 1 ? "0.28em" : 0,
                              }}
                            >
                              {w.word}
                            </span>
                          );
                        })}
                      </span>
                    </div>
                  );
                })()}
              </div>
            )}

            {/* Интерактивный редакторский холст Konva (активен на паузе) */}
            {!playing && disp.w > 0 && (
              <Stage
                width={disp.w}
                height={disp.h}
                className="absolute inset-0 z-20"
                onMouseDown={(e) => {
                  if (e.target === e.target.getStage()) {
                    setSel(null);
                    setSelT(null);
                  }
                }}
              >
                <Layer>
                  {/* Полоса субтитров (перетаскивание по высоте) */}
                  {lane === "subs" && (
                    <Rect
                      x={0}
                      y={subY * sy - 14}
                      width={disp.w}
                      height={28}
                      fill="rgba(198,242,78,0.05)"
                      stroke="rgba(198,242,78,0.32)"
                      dash={[6, 4]}
                      draggable
                      dragBoundFunc={(p) => ({ x: 0, y: p.y })}
                      onDragEnd={(e) => {
                        if (!sy) return;
                        patch({ op: "subpos", sub_y: Math.round((e.target.y() + 14) / sy) });
                      }}
                    />
                  )}

                  {/* Центральный гайд выравнивания */}
                  {guide != null && (
                    <Line points={[guide, 0, guide, disp.h]} stroke="#c6f24e" strokeWidth={1} dash={[4, 4]} />
                  )}

                  {/* Редактирование зон блюра */}
                  {lane === "blur" &&
                    blurs.map((b, i) => {
                      if (b.hidden) return null;
                      const inRange = scrub >= b.t0 && scrub <= b.t1;
                      const active = sel === i;
                      return (
                        <Rect
                          key={`blur-${i}`}
                          ref={(n) => { if (n) boxRefs.current[i] = n; }}
                          x={b.x * sx}
                          y={b.y * sy}
                          width={b.w * sx}
                          height={b.h * sy}
                          fill={b.fill || (active ? "rgba(198,242,78,0.18)" : "rgba(255,255,255,0.12)")}
                          stroke={active ? "#c6f24e" : inRange ? "rgba(255,255,255,0.7)" : "rgba(255,255,255,0.25)"}
                          strokeWidth={active ? 2 : 1}
                          dash={inRange ? undefined : [4, 4]}
                          draggable={active}
                          onClick={() => setSel(i)}
                          onTap={() => setSel(i)}
                          onDragMove={(e) => centerGuide(e.target.x(), e.target.width() * e.target.scaleX())}
                          onDragEnd={(e) => {
                            setGuide(null);
                            const r = readRect(e.target);
                            if (r) patch({ op: "blur_update", index: i, ...r });
                          }}
                          onTransformEnd={(e) => {
                            const r = readRect(e.target);
                            if (r) patch({ op: "blur_update", index: i, ...r });
                          }}
                        />
                      );
                    })}

                  {/* Редактирование титров */}
                  {lane === "titles" &&
                    titles.map((ti, i) => {
                      const inRange = scrub >= ti.start && scrub <= ti.end;
                      const active = selT === i;
                      const [bx, by, bw, bh] = (ti.bbox || [0, 0, vw, 60]) as number[];
                      return (
                        <Rect
                          key={`title-${i}`}
                          ref={(n) => { if (n) titleRefs.current[i] = n; }}
                          x={bx * sx}
                          y={by * sy}
                          width={bw * sx}
                          height={bh * sy}
                          fill={active ? "rgba(198,242,78,0.15)" : inRange ? "rgba(255,255,255,0.08)" : "rgba(255,255,255,0.03)"}
                          stroke={active ? "#c6f24e" : inRange ? "rgba(255,255,255,0.7)" : "rgba(255,255,255,0.25)"}
                          strokeWidth={active ? 2 : 1}
                          dash={inRange ? undefined : [4, 4]}
                          draggable={active}
                          onClick={() => setSelT(i)}
                          onTap={() => setSelT(i)}
                          onDragMove={(e) => centerGuide(e.target.x(), e.target.width() * e.target.scaleX())}
                          onDragEnd={(e) => {
                            setGuide(null);
                            const r = readRect(e.target);
                            if (r) patch({ op: "title_update", index: i, bbox: [r.x, r.y, r.w, r.h] });
                          }}
                          onTransformEnd={(e) => {
                            const r = readRect(e.target);
                            if (r) patch({ op: "title_update", index: i, bbox: [r.x, r.y, r.w, r.h] });
                          }}
                        />
                      );
                    })}

                  {/* Маркеры трансформации (ресайз и масштабирование масок блюра и титров) */}
                  <Transformer
                    ref={trRef}
                    rotateEnabled={false}
                    borderStroke="#c6f24e"
                    anchorStroke="#c6f24e"
                    anchorFill="#101014"
                    anchorSize={8}
                    borderDash={[3, 3]}
                    boundBoxFunc={(_, b) => ({ ...b, width: Math.max(20, b.width), height: Math.max(12, b.height) })}
                  />
                </Layer>
              </Stage>
            )}
          </>
        )}
        {busy && (
          <div className="absolute top-2 right-2 text-[11px] text-[var(--color-accent-2)] bg-black/60 px-2 py-0.5 rounded">
            updating…
          </div>
        )}
      </div>
    </div>
  );
}
