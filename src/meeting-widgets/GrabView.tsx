import React, {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { convertFileSrc } from "@tauri-apps/api/core";
import {
  ArrowUpRight,
  Highlighter,
  Keyboard,
  MapPin,
  PenLine,
  RectangleHorizontal,
  Type,
  Undo2,
  type LucideIcon,
} from "lucide-react";
import { commands, events, type GrabCapture, type GrabFrame } from "@/bindings";
import {
  COLORS,
  FONT,
  TEXT_SIZE,
  clampTo,
  fit,
  fromPoints,
  inside,
  markAt,
  paint,
  shapes,
  toGrabMarks,
  type Mark,
  type Point,
  type Rect,
  type Shape,
  type Tool,
} from "./grabMarks";
import { GrabShortcuts } from "./GrabShortcuts";

/** The picture sent to Claude is at most this wide, as with screenshots. */
const MAX_WIDTH = 1600;
/** The pill's thumbnails, at twice their size for high-DPI screens. */
const THUMBNAIL_WIDTH = 96;
/** A press that moves less than this (CSS pixels) is a click, not a drag. */
const CLICK_SLOP = 4;
/** The smallest grab, in screen pixels. */
const MIN_GRAB = 8;
/** The dim layer over everything not being grabbed. */
const DIM = "rgba(24, 20, 28, 0.55)";
/** The toolbar's height and its gap from the grab, in CSS pixels. */
const BAR = 52;
const BAR_GAP = 10;

const TOOLS: { tool: Tool; key: string; icon: LucideIcon }[] = [
  { tool: "arrow", key: "A", icon: ArrowUpRight },
  { tool: "box", key: "R", icon: RectangleHorizontal },
  { tool: "highlight", key: "H", icon: Highlighter },
  { tool: "pen", key: "P", icon: PenLine },
  { tool: "text", key: "T", icon: Type },
  { tool: "pin", key: "N", icon: MapPin },
];
const COLOR_NAMES = ["pink", "amber", "blue"] as const;

/** `part` of `image` with `marks` drawn in, as a PNG data URL at most
 * `maxWidth` wide. The marks are in the overlay's pixels, where the part
 * shows at `shown`. */
const crop = (
  image: HTMLImageElement,
  part: Rect,
  shown: Rect,
  maxWidth: number,
  marks: Shape[],
) => {
  const scale = Math.min(1, maxWidth / part.width);
  const canvas = document.createElement("canvas");
  canvas.width = Math.max(1, Math.round(part.width * scale));
  canvas.height = Math.max(1, Math.round(part.height * scale));
  const context = canvas.getContext("2d");
  if (context) {
    context.drawImage(
      image,
      part.x,
      part.y,
      part.width,
      part.height,
      0,
      0,
      canvas.width,
      canvas.height,
    );
    const k = canvas.width / shown.width;
    context.setTransform(k, 0, 0, k, -shown.x * k, -shown.y * k);
    paint(context, marks);
  }
  return canvas.toDataURL("image/png");
};

/** Where a capture opened again shows: as large as fits between the hint
 * and the toolbar, never larger than it is. */
const placed = (frame: GrabFrame, capture: GrabCapture): Rect =>
  fit(capture.width, capture.height, {
    x: frame.width * 0.06,
    y: frame.height * 0.09,
    width: frame.width * 0.88,
    height: frame.height * 0.68,
  });

const ShapeView: React.FC<{ shape: Shape }> = ({ shape }) =>
  "d" in shape ? (
    <path
      d={shape.d}
      fill={shape.fill ?? "none"}
      stroke={shape.stroke}
      strokeWidth={shape.width}
      opacity={shape.opacity}
      strokeLinecap="round"
      strokeLinejoin="round"
    />
  ) : (
    <text
      x={shape.x}
      y={shape.y}
      fontSize={shape.size}
      fontWeight={700}
      fontFamily={FONT}
      fill={shape.fill}
      textAnchor={shape.anchor}
      stroke={shape.halo ? "#ffffff" : undefined}
      strokeWidth={shape.halo}
      strokeLinejoin="round"
      paintOrder="stroke"
    >
      {shape.text}
    </text>
  );

type History = { past: Mark[][]; marks: Mark[]; future: Mark[][] };
const EMPTY: History = { past: [], marks: [], future: [] };

const smallKey =
  "font-[inherit] text-[10px] leading-[11px] px-[3px] rounded border border-line-strong border-b-2 bg-background text-plum";
const key =
  "font-[inherit] text-[10px] leading-3 px-[3px] rounded border border-line-strong border-b-2 bg-background text-plum";
const iconButton =
  "relative size-11 rounded-[9px] flex items-center justify-center cursor-pointer";
const separator = (
  <span aria-hidden="true" className="w-px h-7 mx-1.5 bg-line" />
);

/** The grab overlay: the screen under the mouse, frozen. Drag a region,
 * click a window or press F for the whole screen, mark it up, then Enter
 * adds it to the question (Ctrl+Enter adds it and asks); Esc drops it. */
export const GrabView: React.FC = () => {
  const { t } = useTranslation();
  const [frame, setFrame] = useState<GrabFrame | null>(null);
  const [image, setImage] = useState<HTMLImageElement | null>(null);
  const [drag, setDrag] = useState<{ start: Point; end: Point } | null>(null);
  const [mouse, setMouse] = useState<Point | null>(null);
  const [picked, setPicked] = useState<{ rect: Rect; of: string } | null>(null);
  // The corner that stays put while a corner handle is dragged.
  const [resize, setResize] = useState<Point | null>(null);
  const [tool, setTool] = useState<Tool>("arrow");
  const [color, setColor] = useState(0);
  const [history, setHistory] = useState<History>(EMPTY);
  const [draft, setDraft] = useState<Mark | null>(null);
  const [typing, setTyping] = useState<{ at: Point; value: string } | null>(
    null,
  );
  const [selected, setSelected] = useState<number | null>(null);
  const [keysOpen, setKeysOpen] = useState(false);
  const [barWidth, setBarWidth] = useState(0);
  const bar = useRef<HTMLDivElement>(null);
  const label = useRef<HTMLInputElement>(null);
  const nextId = useRef(1);
  const busy = useRef(false);
  // The label being typed, also kept where a blur straight after Enter
  // sees that it is already done.
  const typingNow = useRef<{ at: Point; value: string } | null>(null);
  const marks = history.marks;

  const show = useCallback((next: GrabFrame | null) => {
    setFrame(next);
    setImage(null);
    setDrag(null);
    // A capture opened again is already picked: all of it.
    setPicked(
      next?.capture
        ? { rect: placed(next, next.capture), of: next.under_mouse }
        : null,
    );
    setResize(null);
    setHistory(EMPTY);
    setDraft(null);
    typingNow.current = null;
    setTyping(null);
    setSelected(null);
    setKeysOpen(false);
    busy.current = false;
    if (!next) return;
    const img = new Image();
    // The asset protocol allows this page's origin, so the canvas that
    // crops the grab isn't tainted.
    img.crossOrigin = "anonymous";
    img.onload = () => setImage(img);
    img.src = convertFileSrc(next.path);
  }, []);

  useEffect(() => {
    commands.getAskGrab().then(show);
    const unlisten = events.askGrabEvent.listen(({ payload }) =>
      show(payload.frame),
    );
    return () => {
      unlisten.then((u) => u());
    };
  }, [show]);

  // Screen pixels per CSS pixel: the overlay covers the screen it froze.
  const scale = frame ? frame.width / window.innerWidth : 1;
  const at = (e: React.MouseEvent): Point => ({
    x: Math.round(e.clientX * scale),
    y: Math.round(e.clientY * scale),
  });
  const windowAt = (p: Point) =>
    frame?.windows.find(
      (w) =>
        p.x >= w.x && p.x < w.x + w.width && p.y >= w.y && p.y < w.y + w.height,
    ) ?? null;

  const commit = useCallback(
    (change: (marks: Mark[]) => Mark[]) =>
      setHistory((h) => ({
        past: [...h.past, h.marks],
        marks: change(h.marks),
        future: [],
      })),
    [],
  );
  const undo = () =>
    setHistory((h) =>
      h.past.length
        ? {
            past: h.past.slice(0, -1),
            marks: h.past[h.past.length - 1],
            future: [h.marks, ...h.future],
          }
        : h,
    );
  const redo = () =>
    setHistory((h) =>
      h.future.length
        ? {
            past: [...h.past, h.marks],
            marks: h.future[0],
            future: h.future.slice(1),
          }
        : h,
    );

  const type = useCallback((next: { at: Point; value: string } | null) => {
    typingNow.current = next;
    setTyping(next);
  }, []);
  /** A text label is done: Enter, a click elsewhere, or the toolbar. */
  const finishTyping = useCallback(() => {
    const current = typingNow.current;
    type(null);
    const text = current?.value.trim();
    if (!current || !text) return;
    commit((list) => [
      ...list,
      {
        id: nextId.current++,
        kind: "text",
        color: COLORS[color],
        points: [current.at],
        label: text,
        at: performance.now(),
      },
    ]);
  }, [color, commit, type]);

  useEffect(() => {
    if (typing) label.current?.focus();
  }, [typing]);

  const drop = useCallback(() => {
    if (busy.current) return;
    commands.closeAskGrab();
  }, []);

  const add = useCallback(
    async (askNow: boolean) => {
      if (!picked || !image || !frame || busy.current) return;
      busy.current = true;
      const now = performance.now();
      const drawn = marks.flatMap((m, i) =>
        shapes(m, frame.first_mark + i, scale),
      );
      // A frozen screen is cropped where the grab is; a capture opened again
      // is all used, at its own size.
      const part = frame.capture
        ? {
            x: 0,
            y: 0,
            width: frame.capture.width,
            height: frame.capture.height,
          }
        : picked.rect;
      const png = crop(image, part, picked.rect, MAX_WIDTH, drawn).replace(
        /^data:image\/png;base64,/,
        "",
      );
      const thumbnail = crop(image, part, picked.rect, THUMBNAIL_WIDTH, drawn);
      const result = await commands.finishAskGrab({
        png,
        thumbnail,
        of: picked.of,
        marks: toGrabMarks(marks, picked.rect, frame.first_mark, now, scale),
        ask_now: askNow,
      });
      if (result.status === "error") {
        console.error("Couldn't add the grab:", result.error);
        busy.current = false;
      }
    },
    [picked, image, frame, marks, scale],
  );

  // Esc goes to a label being typed first, then drops the grab.
  const escape = useCallback(() => {
    if (typingNow.current) type(null);
    else drop();
  }, [type, drop]);
  const escapeNow = useRef(escape);
  useLayoutEffect(() => {
    escapeNow.current = escape;
  }, [escape]);
  useEffect(() => {
    // Handy's cancel shortcut takes Esc before this page sees it.
    const unlisten = events.askGrabEscapeEvent.listen(() =>
      escapeNow.current(),
    );
    return () => {
      unlisten.then((u) => u());
    };
  }, []);

  useEffect(() => {
    if (!frame) return;
    const onKey = (e: KeyboardEvent) => {
      // A label being typed has the keys to itself.
      if (typing) return;
      const ctrl = e.ctrlKey || e.metaKey;
      const letter = e.key.toLowerCase();
      if (e.key === "Escape") {
        e.preventDefault();
        drop();
      } else if (e.key === "?") {
        e.preventDefault();
        setKeysOpen((open) => !open);
      } else if (!picked) {
        if (letter === "f" && !ctrl && !e.altKey) {
          e.preventDefault();
          setPicked({
            rect: { x: 0, y: 0, width: frame.width, height: frame.height },
            of: t("ask.grab.wholeScreen"),
          });
        }
      } else if (e.key === "Enter") {
        e.preventDefault();
        add(ctrl);
      } else if (ctrl && (letter === "y" || (letter === "z" && e.shiftKey))) {
        e.preventDefault();
        redo();
      } else if (ctrl && letter === "z") {
        e.preventDefault();
        undo();
      } else if (
        (e.key === "Delete" || e.key === "Backspace") &&
        selected !== null
      ) {
        e.preventDefault();
        commit((list) => list.filter((m) => m.id !== selected));
        setSelected(null);
      } else if (!ctrl && !e.altKey) {
        const picks = TOOLS.find((x) => x.key.toLowerCase() === letter);
        const swatch = ["1", "2", "3"].indexOf(e.key);
        if (picks) setTool(picks.tool);
        else if (swatch >= 0) setColor(swatch);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [frame, picked, typing, selected, add, drop, commit, t]);

  const hasPick = picked !== null;
  useLayoutEffect(() => {
    if (hasPick && bar.current) setBarWidth(bar.current.offsetWidth);
  }, [hasPick]);

  if (!frame || !image) {
    return <div className="fixed inset-0 bg-black" />;
  }

  const whole = { x: 0, y: 0, width: frame.width, height: frame.height };
  const onMouseDown = (e: React.MouseEvent) => {
    if (e.button !== 0) return;
    const p = at(e);
    if (!picked) {
      setDrag({ start: p, end: p });
      return;
    }
    // Keep the focus (and a label being typed) where it is.
    e.preventDefault();
    if (typing) {
      finishTyping();
      return;
    }
    const hit = markAt(marks, p, scale);
    setSelected(hit ? hit.id : null);
    if (hit || !inside(p, picked.rect)) return;
    if (tool === "text") {
      type({ at: p, value: "" });
    } else if (tool === "pin") {
      commit((list) => [
        ...list,
        {
          id: nextId.current++,
          kind: "pin",
          color: COLORS[color],
          points: [p],
          at: performance.now(),
        },
      ]);
    } else {
      setDraft({
        id: nextId.current++,
        kind: tool,
        color: COLORS[color],
        points: [p],
        at: 0,
      });
    }
  };
  const onMouseMove = (e: React.MouseEvent) => {
    const p = at(e);
    setMouse(p);
    if (drag) setDrag({ ...drag, end: p });
    if (resize && picked) {
      const rect = fromPoints(resize, clampTo(p, whole));
      if (rect.width >= MIN_GRAB && rect.height >= MIN_GRAB) {
        setPicked({ ...picked, rect });
      }
    }
    if (draft && picked) {
      const q = clampTo(p, picked.rect);
      const [start] = draft.points;
      const last = draft.points[draft.points.length - 1];
      if (draft.kind === "pen") {
        if (Math.hypot(q.x - last.x, q.y - last.y) >= 2 * scale) {
          setDraft({ ...draft, points: [...draft.points, q] });
        }
      } else {
        setDraft({ ...draft, points: [start, q] });
      }
    }
  };
  const onMouseUp = (e: React.MouseEvent) => {
    if (resize) {
      setResize(null);
      return;
    }
    if (draft) {
      const xs = draft.points.map((p) => p.x);
      const ys = draft.points.map((p) => p.y);
      const reach = Math.hypot(
        Math.max(...xs) - Math.min(...xs),
        Math.max(...ys) - Math.min(...ys),
      );
      setDraft(null);
      // A click with a drawing tool is not a mark.
      if (reach >= CLICK_SLOP * scale) {
        const done = { ...draft, at: performance.now() };
        commit((list) => [...list, done]);
      }
      return;
    }
    if (!drag) return;
    const end = at(e);
    setDrag(null);
    const moved = Math.hypot(end.x - drag.start.x, end.y - drag.start.y);
    if (moved < CLICK_SLOP * scale) {
      const target = windowAt(end);
      if (target) setPicked({ rect: target, of: target.label });
      return;
    }
    setPicked({
      rect: fromPoints(drag.start, end),
      of: frame.under_mouse || t("ask.grab.region"),
    });
  };

  const hovered = !picked && !drag && mouse ? windowAt(mouse) : null;
  const shown: Rect | null = picked
    ? picked.rect
    : drag
      ? fromPoints(drag.start, drag.end)
      : hovered;
  // The dim layer with a hole where the grab is.
  const mask = shown
    ? `M0 0H${frame.width}V${frame.height}H0Z M${shown.x} ${shown.y}h${shown.width}v${shown.height}h${-shown.width}Z`
    : `M0 0H${frame.width}V${frame.height}H0Z`;
  const css = (n: number) => n / scale;
  const number = (mark: Mark) => frame.first_mark + marks.indexOf(mark);
  const chosen = marks.find((m) => m.id === selected);

  // The toolbar goes under the grab, else over it, else inside its foot;
  // centred on it and kept on the screen.
  let barLeft = 0;
  let barTop = 0;
  if (picked) {
    const r = picked.rect;
    const below = css(r.y + r.height) + BAR_GAP;
    const above = css(r.y) - BAR_GAP - BAR;
    barTop =
      below + BAR + 72 < window.innerHeight
        ? below
        : above > 56
          ? above
          : Math.min(window.innerHeight, css(r.y + r.height)) - BAR - 80;
    barLeft = Math.min(
      Math.max(8, css(r.x + r.width / 2) - barWidth / 2),
      Math.max(8, window.innerWidth - barWidth - 8),
    );
  }
  const corners = picked
    ? [
        [picked.rect.x, picked.rect.y],
        [picked.rect.x + picked.rect.width, picked.rect.y],
        [picked.rect.x, picked.rect.y + picked.rect.height],
        [picked.rect.x + picked.rect.width, picked.rect.y + picked.rect.height],
      ]
    : [];

  return (
    <div
      className={`fixed inset-0 select-none overflow-hidden font-ui text-text ${
        picked && tool === "text" && !resize
          ? "cursor-text"
          : "cursor-crosshair"
      }`}
      onMouseDown={onMouseDown}
      onMouseMove={onMouseMove}
      onMouseUp={onMouseUp}
    >
      {frame.capture && picked ? (
        <>
          <div className="absolute inset-0 bg-[rgb(24,20,28)]" />
          <img
            src={image.src}
            alt=""
            draggable={false}
            className="absolute"
            style={{
              left: css(picked.rect.x),
              top: css(picked.rect.y),
              width: css(picked.rect.width),
              height: css(picked.rect.height),
            }}
          />
        </>
      ) : (
        <img
          src={image.src}
          alt=""
          draggable={false}
          className="absolute inset-0 w-screen h-screen"
        />
      )}
      <svg
        className="absolute inset-0 w-screen h-screen"
        viewBox={`0 0 ${frame.width} ${frame.height}`}
        preserveAspectRatio="none"
        aria-hidden="true"
      >
        <path d={mask} fill={DIM} fillRule="evenodd" />
        {shown && (
          <rect
            x={shown.x}
            y={shown.y}
            width={shown.width}
            height={shown.height}
            strokeWidth={2 * scale}
            style={{
              fill: hovered ? "rgba(218, 88, 147, 0.12)" : "none",
              stroke: "var(--color-background-ui)",
            }}
          />
        )}
        {picked && (
          <defs>
            {/* Marks show only where they will be in the picture. */}
            <clipPath id="grab">
              <rect
                x={picked.rect.x}
                y={picked.rect.y}
                width={picked.rect.width}
                height={picked.rect.height}
              />
            </clipPath>
          </defs>
        )}
        {picked && (
          <g clipPath="url(#grab)">
            {[...marks, ...(draft ? [draft] : [])].map((mark) =>
              shapes(
                mark,
                mark === draft ? frame.first_mark + marks.length : number(mark),
                scale,
              ).map((shape, i) => (
                <ShapeView key={`${mark.id}-${i}`} shape={shape} />
              )),
            )}
          </g>
        )}
        {/* The picked mark's number gets a ring. */}
        {chosen &&
          shapes(chosen, number(chosen), scale).map((shape, i) =>
            "d" in shape && shape.fill ? (
              <path
                key={i}
                d={shape.d}
                fill="none"
                stroke="#ffffff"
                strokeWidth={3 * scale}
                style={{ filter: "drop-shadow(0 0 2px rgba(0,0,0,0.6))" }}
              />
            ) : null,
          )}
      </svg>

      {picked &&
        !typing &&
        !frame.capture &&
        corners.map(([x, y], i) => (
          <span
            key={i}
            aria-hidden="true"
            className={`absolute size-2.5 border-2 border-background-ui bg-white ${
              i === 0 || i === 3 ? "cursor-nwse-resize" : "cursor-nesw-resize"
            }`}
            style={{ left: css(x) - 5, top: css(y) - 5 }}
            onMouseDown={(e) => {
              if (e.button !== 0) return;
              e.stopPropagation();
              e.preventDefault();
              const [ox, oy] = corners[3 - i];
              setResize({ x: ox, y: oy });
            }}
          />
        ))}

      {shown && (drag || picked) && !frame.capture && (
        <span
          className="absolute h-[22px] px-2 flex items-center rounded-md bg-[rgba(31,26,34,0.92)] text-white text-xs tabular-nums pointer-events-none"
          style={{
            left: css(shown.x),
            top: Math.max(4, css(shown.y) - 30),
          }}
        >
          {t("ask.grab.size", {
            width: Math.round(shown.width),
            height: Math.round(shown.height),
          })}
        </span>
      )}

      {typing && (
        <input
          ref={label}
          value={typing.value}
          placeholder={t("ask.grab.labelPlaceholder")}
          aria-label={t("ask.grab.tools.text")}
          className="absolute bg-transparent outline-1 outline-dashed outline-white/80 px-0.5 placeholder:text-white/70"
          style={{
            left: css(typing.at.x),
            top: css(typing.at.y) - TEXT_SIZE * 0.7,
            font: `700 ${TEXT_SIZE}px ${FONT}`,
            color: COLORS[color],
            textShadow:
              "0 0 2px #fff, 0 0 2px #fff, 0 0 3px #fff, 0 0 4px #fff",
            width: `${Math.max(8, typing.value.length + 2)}ch`,
          }}
          onChange={(e) => type({ ...typing, value: e.target.value })}
          onMouseDown={(e) => e.stopPropagation()}
          onBlur={finishTyping}
          onKeyDown={(e) => {
            // Enter finishes the label, not the grab.
            if (e.key === "Enter") {
              e.preventDefault();
              e.stopPropagation();
              finishTyping();
            } else if (e.key === "Escape") {
              e.preventDefault();
              e.stopPropagation();
              type(null);
            }
          }}
        />
      )}

      {picked ? (
        <>
          <div className="absolute top-3 inset-x-0 flex justify-center pointer-events-none">
            <span className="h-[34px] px-3.5 flex items-center gap-2.5 rounded-full bg-[rgba(31,26,34,0.92)] text-white text-[13px]">
              <span className="size-2 rounded-full bg-logo-primary" />
              {t("ask.grab.stillListening")}
              <span
                aria-hidden="true"
                className="size-[3px] rounded-full bg-white/70"
              />
              <span className="text-white/90">
                {frame.capture
                  ? t("ask.grab.backHint")
                  : t("ask.grab.marksHint")}
              </span>
            </span>
          </div>
          <div
            ref={bar}
            className="absolute h-[52px] flex items-center gap-0.5 px-1.5 rounded-xl border border-line bg-background shadow-lg text-[13px] cursor-default"
            style={{ left: barLeft, top: barTop }}
            onMouseDown={(e) => e.stopPropagation()}
          >
            {TOOLS.map(({ tool: id, key: letter, icon: Icon }) => {
              const on = id === tool;
              const name = t("ask.grab.withKey", {
                name: t(`ask.grab.tools.${id}`),
                key: letter,
              });
              return (
                <button
                  key={id}
                  type="button"
                  aria-label={name}
                  aria-pressed={on}
                  title={name}
                  className={`${iconButton} ${
                    on
                      ? "bg-background-ui/10 text-background-ui"
                      : "text-ink-2 hover:bg-wash"
                  }`}
                  onClick={() => setTool(id)}
                >
                  <Icon size={18} strokeWidth={2} aria-hidden="true" />
                  <kbd
                    className={`absolute right-[3px] bottom-[3px] ${smallKey}`}
                  >
                    {letter}
                  </kbd>
                </button>
              );
            })}
            {separator}
            {COLORS.map((hex, i) => {
              const name = t("ask.grab.withKey", {
                name: t(`ask.grab.colors.${COLOR_NAMES[i]}`),
                key: i + 1,
              });
              return (
                <button
                  key={hex}
                  type="button"
                  aria-label={name}
                  aria-pressed={i === color}
                  title={name}
                  className="relative w-9 h-11 rounded-[9px] flex items-center justify-center cursor-pointer hover:bg-wash"
                  onClick={() => setColor(i)}
                >
                  <span
                    className="size-[18px] rounded-full"
                    style={{
                      background: hex,
                      boxShadow:
                        i === color
                          ? `0 0 0 2px var(--color-background), 0 0 0 4px ${hex}`
                          : "none",
                    }}
                  />
                  <kbd className={`absolute right-px bottom-[3px] ${smallKey}`}>
                    {i + 1}
                  </kbd>
                </button>
              );
            })}
            {separator}
            <button
              type="button"
              aria-label={t("ask.grab.undo")}
              title={t("ask.grab.undo")}
              disabled={history.past.length === 0}
              className={`${iconButton} text-ink-2 hover:bg-wash disabled:opacity-40 disabled:cursor-default`}
              onClick={undo}
            >
              <Undo2 size={18} strokeWidth={2} aria-hidden="true" />
            </button>
            <button
              type="button"
              aria-label={t("ask.grab.allKeys")}
              aria-pressed={keysOpen}
              title={t("ask.grab.allKeys")}
              className={`${iconButton} ${
                keysOpen
                  ? "bg-background-ui/10 text-background-ui"
                  : "text-ink-2 hover:bg-wash"
              }`}
              onClick={() => setKeysOpen((open) => !open)}
            >
              <Keyboard size={18} strokeWidth={2} aria-hidden="true" />
            </button>
            {separator}
            {frame.capture ? (
              <button
                type="button"
                className="h-9 px-2.5 flex items-center gap-1.5 rounded-[9px] border border-line-strong bg-background hover:bg-wash cursor-pointer"
                onClick={() =>
                  frame.capture && commands.removeAskCapture(frame.capture.n)
                }
              >
                {t("ask.grab.takeOut")}
              </button>
            ) : (
              <button
                type="button"
                className="h-9 px-2.5 flex items-center gap-1.5 rounded-[9px] border border-line-strong bg-background hover:bg-wash cursor-pointer"
                onClick={drop}
              >
                {t("ask.grab.drop")}
                <kbd className={key}>{t("ask.grab.keyEsc")}</kbd>
              </button>
            )}
            <button
              type="button"
              className="h-9 ml-1 px-2.5 flex items-center gap-1.5 rounded-[9px] border border-line-strong bg-background hover:bg-wash cursor-pointer"
              onClick={() => add(false)}
            >
              {frame.capture ? t("ask.grab.done") : t("ask.grab.add")}
              <kbd className={key}>{t("ask.grab.keyEnter")}</kbd>
            </button>
            <button
              type="button"
              className="h-9 ml-1 px-2.5 flex items-center gap-1.5 rounded-[9px] bg-background-ui text-white font-semibold cursor-pointer"
              onClick={() => add(true)}
            >
              {frame.capture
                ? t("ask.grab.doneAndAsk")
                : t("ask.grab.addAndAsk")}
              <kbd className="font-[inherit] text-[10px] leading-3 px-[3px] rounded border border-white/50 border-b-2 bg-black/15 text-white">
                {t("ask.grab.keyCtrlEnter")}
              </kbd>
            </button>
          </div>
        </>
      ) : (
        <div className="absolute bottom-6 inset-x-0 flex justify-center pointer-events-none">
          <span className="h-8 px-4 flex items-center rounded-full bg-[rgba(31,26,34,0.92)] text-white text-[13px]">
            {t("ask.grab.pickHint")}
          </span>
        </div>
      )}

      {keysOpen && (
        <div
          className="absolute right-4 top-14 cursor-default"
          onMouseDown={(e) => e.stopPropagation()}
        >
          <GrabShortcuts />
        </div>
      )}
    </div>
  );
};
