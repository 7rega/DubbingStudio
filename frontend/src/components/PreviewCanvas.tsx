// PreviewCanvas — the editing heart. Shows the engine's REAL rendered frame (WYSIWYG) with a react-konva
// overlay: drag/resize the blur boxes, drag/resize the titles, and drag the subtitle band directly on the
// frame -> PATCH the Project -> re-render the frame. Which objects are interactive follows the LEFT lane
// (subs | blur | titles): on the titles tab you edit titles, on the mask tab you edit blur zones — они не
// перехватывают клики друг друга. (A 60fps JASSUB live layer over HTML5 <video> is the M2 upgrade.)
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Stage, Layer, Rect, Line, Transformer } from "react-konva";
import type Konva from "konva";
import { api, type Project } from "../lib/api";
import { useStore } from "../store";

type Lane = "subs" | "blur" | "titles";
type Props = { pid: string; project: Project; scrub: number; rendered: boolean; lane: Lane; playing?: boolean; onChanged: (fresh: Project) => void };

export default function PreviewCanvas({ pid, project, scrub, rendered, lane, playing = false, onChanged }: Props) {
  const { t } = useTranslation();
  const rev = useStore((s) => s.rev);
  const bump = useStore((s) => s.bump);
  const sel = useStore((s) => s.selBlur);        // SHARED with the left blur list (click list <-> click canvas)
  const setSel = useStore((s) => s.setSelBlur);
  const selT = useStore((s) => s.selTitle);      // SHARED with the left titles list
  const setSelT = useStore((s) => s.setSelTitle);
  const wrap = useRef<HTMLDivElement>(null);
  const trRef = useRef<Konva.Transformer>(null);
  const boxRefs = useRef<Record<number, Konva.Rect>>({});
  const titleRefs = useRef<Record<number, Konva.Rect>>({});
  const [disp, setDisp] = useState({ w: 0, h: 0 });
  const [guide, setGuide] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);

  const vw = project.meta.width || 1, vh = project.meta.height || 1;
  const sx = disp.w / vw, sy = disp.h / vh;
  const previewSrc = rendered ? api.outputUrl(pid) : api.previewUrl(pid, scrub, rev);

  // Backpressure превью-кадра: <img> тянет СЛЕДУЮЩИЙ кадр только когда загрузился предыдущий (onLoad),
  // и всегда самый свежий scrub — промежуточные пропускаем. Без этого плей тикает быстрее серверного
  // рендера (~150мс/кадр, сериализовано) → очередь растёт, кадр отстаёт от звука → «фриз/слайдшоу».
  // Теперь превью едет на реальной скорости сервера (свежий кадр под звук, с потерей промежуточных —
  // ровно «пусть с потерей кадров, но видно»). В rendered-режиме (<video>) не нужно.
  const [imgSrc, setImgSrc] = useState(() => api.previewUrl(pid, scrub, rev));
  const loadingRef = useRef(false);
  const pendingRef = useRef<string | null>(null);
  // Сброс backpressure-латча при смене rendered/pid: когда rendered флипается в true, <img> размонтируется
  // и его onLoad уже НЕ придёт -> loadingRef остался бы навсегда true, и при возврате в preview кадр замер бы.
  useEffect(() => { loadingRef.current = false; pendingRef.current = null; }, [rendered, pid]);
  useEffect(() => {
    if (rendered) return;
    const want = api.previewUrl(pid, scrub, rev, playing);   // при плее -> lr=1 (низкое разрешение на больших видео)
    if (want === imgSrc) return;                                    // этот кадр уже показан/грузится
    if (loadingRef.current) { pendingRef.current = want; return; }  // сервер занят — запомнить самый свежий
    loadingRef.current = true; setImgSrc(want);
  }, [pid, scrub, rev, rendered, playing, imgSrc]);
  const onFrameSettled = () => {
    const next = pendingRef.current;
    pendingRef.current = null;
    if (next && next !== imgSrc) setImgSrc(next);   // грузим самый свежий из отложенных (loadingRef остаётся true)
    else loadingRef.current = false;                // очередь пуста — свободны
  };

  // fit the overlay to the displayed media (preserve aspect)
  useEffect(() => {
    const el = wrap.current; if (!el) return;
    const measure = () => {
      const cw = el.clientWidth, ch = el.clientHeight;
      if (cw <= 0 || ch <= 0) return;                 // container momentarily 0 -> don't collapse the overlay to 0x0
      const scale = Math.min(cw / vw, ch / vh);
      setDisp({ w: Math.round(vw * scale), h: Math.round(vh * scale) });
    };
    const ro = new ResizeObserver(measure);
    ro.observe(el); measure();                        // measure immediately + on every resize
    return () => ro.disconnect();
  }, [vw, vh]);

  // attach the resize transformer to the selected object of the ACTIVE lane (blur box or title)
  useEffect(() => {
    let node: Konva.Rect | null = null;
    if (lane === "blur" && sel != null) {
      const hidden = (project.captions.blur_boxes || [])[sel]?.hidden;   // no resize handles on a disabled zone
      if (!hidden) node = boxRefs.current[sel] ?? null;
    } else if (lane === "titles" && selT != null) {
      node = titleRefs.current[selT] ?? null;
    }
    if (node && node.getLayer() && trRef.current) {   // getLayer() != null -> узел ещё смонтирован (не устаревший реф)
      trRef.current.nodes([node]); trRef.current.getLayer()?.batchDraw();
    } else trRef.current?.nodes([]);
  }, [sel, selT, lane, disp, project]);

  // гасим зависшую центральную направляющую при перемотке/смене вкладки (onDragEnd мог не прийти, если Rect размонтировался в процессе drag)
  useEffect(() => { setGuide(null); }, [scrub, lane]);

  async function patch(edit: Record<string, unknown>) {
    setBusy(true);
    // api.patch returns the authoritative merged Project -> hand it to the parent. A 2nd GET would clobber an
    // un-persisted tgt_text edit still sitting dirty in the store (textarea not yet blurred).
    try { const fresh = await api.patch(pid, edit); onChanged(fresh); bump(); } finally { setBusy(false); }  // bump -> frame refetches
  }

  const blurs = project.captions.blur_boxes || [];
  const titles = project.captions.titles || [];
  const subY = project.captions.sub_y ?? Math.round(vh * 0.82);

  // Центральная направляющая: подсветить, если центр объекта в пределах 8px от центра кадра.
  const centerGuide = (nx: number, wPx: number) =>
    setGuide(Math.abs(nx + wPx / 2 - disp.w / 2) < 8 ? disp.w / 2 : null);
  // Нормализованный прямоугольник в видео-пикселях после transform (сбрасывает scale); null при нулевом масштабе.
  const readRect = (n: Konva.Node): { x: number; y: number; w: number; h: number } | null => {
    if (!sx || !sy) { n.scaleX(1); n.scaleY(1); return null; }
    const w = Math.round((n.width() * n.scaleX()) / sx);
    const h = Math.round((n.height() * n.scaleY()) / sy);
    n.scaleX(1); n.scaleY(1);
    return { x: Math.round(n.x() / sx), y: Math.round(n.y() / sy), w, h };
  };

  return (
    <div ref={wrap} className="relative w-full h-full min-h-0 overflow-hidden grid place-items-center bg-black/40 rounded-xl">
      <div className="relative" style={{ width: disp.w, height: disp.h }}>
        {rendered
          ? <video src={previewSrc} controls className="absolute inset-0 w-full h-full rounded-lg" />
          : <img src={imgSrc} alt="frame" onLoad={onFrameSettled} onError={onFrameSettled}
                 className="absolute inset-0 w-full h-full rounded-lg" />}
        {!rendered && disp.w > 0 && (
          <Stage width={disp.w} height={disp.h} className="absolute inset-0"
                 onMouseDown={(e) => { if (e.target === e.target.getStage()) { setSel(null); setSelT(null); } }}>
            <Layer>
              {/* subtitle band — draggable vertically; drop -> PATCH sub_y. Only on the subtitles lane. */}
              {lane === "subs" && (
                <Rect x={0} y={subY * sy - 14} width={disp.w} height={28} fill="rgba(198,242,78,0.05)"
                      stroke="rgba(198,242,78,0.32)" dash={[6, 4]} draggable
                      dragBoundFunc={(p) => ({ x: 0, y: p.y })}
                      onDragEnd={(e) => { if (!sy) return; patch({ op: "subpos", sub_y: Math.round((e.target.y() + 14) / sy) }); }} />
              )}
              {/* cover zones — ONLY on the mask lane. Selected zone outlined (cyan); rest invisible but clickable
                  (select via canvas-click or the left list). Active on THIS frame; draggable unless hidden. */}
              {lane === "blur" && blurs.map((b, i) => {
                  if (!(scrub >= b.t0 - 0.6 && scrub <= b.t1 + 0.4)) return null; // не на этом кадре
                  const on = sel === i, hid = !!b.hidden;
                  return (
                    <Rect key={`b${i}`} ref={(n) => { if (n) boxRefs.current[i] = n; else delete boxRefs.current[i]; }}
                          x={b.x * sx} y={b.y * sy} width={b.w * sx} height={b.h * sy}
                          fill={on ? (hid ? "rgba(255,255,255,0.06)" : "rgba(91,224,200,0.18)") : "rgba(255,255,255,0.001)"}
                          stroke={on ? "#5be0c8" : undefined} strokeWidth={on ? 2.5 : 0}
                          dash={on && hid ? [6, 4] : undefined} draggable={!hid}
                          onClick={() => setSel(i)} onTap={() => setSel(i)}
                          onDragMove={(e) => centerGuide(e.target.x(), b.w * sx)}
                          onDragEnd={(e) => { setGuide(null); if (!sx || !sy) return; patch({ op: "blur", idx: i, x: Math.round(e.target.x() / sx), y: Math.round(e.target.y() / sy) }); }}
                          onTransformEnd={(e) => {
                            const r = readRect(e.target);
                            if (r) patch({ op: "blur", idx: i, x: r.x, y: r.y, w: r.w, h: r.h });
                          }} />
                  );
                })}
              {/* titles — ONLY on the titles lane. bbox=[x,y,w,h] в видео-пикселях (совпадает с ass::emit_title).
                  Все титры на этом кадре обведены пунктиром (видно, что кликабельны); выбранный — сплошной.
                  Drag/resize -> PATCH op:"title" bbox. */}
              {lane === "titles" && titles.map((ti, i) => {
                  if ((ti.bbox?.length ?? 0) < 4 || !(scrub >= ti.start - 0.1 && scrub <= ti.end + 0.1)) return null;
                  const [bx, by, bw, bh] = ti.bbox as number[];
                  const on = selT === i;
                  return (
                    <Rect key={`t${i}`} ref={(n) => { if (n) titleRefs.current[i] = n; else delete titleRefs.current[i]; }}
                          x={bx * sx} y={by * sy} width={bw * sx} height={bh * sy}
                          fill={on ? "rgba(198,242,78,0.16)" : "rgba(198,242,78,0.001)"}
                          stroke={on ? "#c6f24e" : "rgba(198,242,78,0.55)"} strokeWidth={on ? 2.5 : 1}
                          dash={on ? undefined : [5, 4]} draggable
                          onClick={() => setSelT(i)} onTap={() => setSelT(i)}
                          onDragMove={(e) => centerGuide(e.target.x(), bw * sx)}
                          onDragEnd={(e) => { setGuide(null); if (!sx || !sy) return; patch({ op: "title", idx: i, bbox: [Math.round(e.target.x() / sx), Math.round(e.target.y() / sy), bw, bh] }); }}
                          onTransformEnd={(e) => {
                            const r = readRect(e.target);
                            if (r) patch({ op: "title", idx: i, bbox: [r.x, r.y, r.w, r.h] });
                          }} />
                  );
                })}
              {guide != null && <Line points={[guide, 0, guide, disp.h]} stroke="#5be0c8" dash={[4, 4]} />}
              <Transformer ref={trRef} rotateEnabled={false} ignoreStroke
                           boundBoxFunc={(_, b) => ({ ...b, width: Math.max(20, b.width), height: Math.max(12, b.height) })} />
            </Layer>
          </Stage>
        )}
        {busy && <div className="absolute top-2 right-2 text-[11px] text-[var(--color-accent-2)] bg-black/60 px-2 py-0.5 rounded">updating…</div>}
        {playing && !rendered && (
          <div className="absolute top-2 left-1/2 -translate-x-1/2 max-w-[92%] text-center text-[10.5px] leading-tight text-white/90 bg-black/60 backdrop-blur-sm px-2.5 py-1 rounded-full pointer-events-none">
            {t("play.lagNotice")}
          </div>
        )}
      </div>
    </div>
  );
}
