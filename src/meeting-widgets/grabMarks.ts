import type { GrabMark, GrabMarkKind } from "@/bindings";

/** Marks drawn on a grab. Everything is in the frozen screen's pixels; `s`
 * is how many of them make one CSS pixel, so marks look the same size on
 * any screen and are drawn into the picture at its full resolution. */

export type Point = { x: number; y: number };
export type Rect = { x: number; y: number; width: number; height: number };
export type Tool = GrabMarkKind;

export type Mark = {
  id: number;
  kind: Tool;
  color: string;
  /** Arrow, box and highlight: start and end. Pen: the stroke. Text and
   * pin: where it was put. */
  points: Point[];
  /** What a text mark says. */
  label?: string;
  /** When it was drawn, on `performance.now()`. */
  at: number;
};

/** One piece of a mark, drawn the same way in the overlay and the picture. */
export type Shape =
  | {
      d: string;
      stroke?: string;
      fill?: string;
      width?: number;
      opacity?: number;
    }
  | {
      text: string;
      x: number;
      y: number;
      size: number;
      fill: string;
      /** A white edge so text reads on any background. */
      halo?: number;
      anchor: "start" | "middle";
    };

/** The ink: drawn into the picture, so the same in light and dark themes. */
export const COLORS = ["#da5893", "#e39b00", "#2f6fde"] as const;
export const FONT = '"Segoe UI", system-ui, sans-serif';

// Sizes in CSS pixels.
const STROKE = 3;
const HIGHLIGHT = 20;
const HEAD = 14;
const TAG = 22;
const PIN = 26;
export const TEXT_SIZE = 20;
/** How near an arrow's end must come to a mark to join it. */
const REACH = 12;

const f = (n: number) => Math.round(n * 10) / 10;

export const fromPoints = (a: Point, b: Point): Rect => ({
  x: Math.min(a.x, b.x),
  y: Math.min(a.y, b.y),
  width: Math.abs(a.x - b.x),
  height: Math.abs(a.y - b.y),
});

export const inside = (p: Point, r: Rect, pad = 0) =>
  p.x >= r.x - pad &&
  p.x <= r.x + r.width + pad &&
  p.y >= r.y - pad &&
  p.y <= r.y + r.height + pad;

export const clampTo = (p: Point, r: Rect): Point => ({
  x: Math.min(Math.max(p.x, r.x), r.x + r.width),
  y: Math.min(Math.max(p.y, r.y), r.y + r.height),
});

const roundRect = (r: Rect, radius: number) => {
  const k = Math.min(radius, r.width / 2, r.height / 2);
  const [x, y, w, h] = [r.x, r.y, r.width, r.height].map(f);
  const c = f(k);
  return (
    `M${f(x + k)} ${y}H${f(x + w - k)}A${c} ${c} 0 0 1 ${f(x + w)} ${f(y + k)}` +
    `V${f(y + h - k)}A${c} ${c} 0 0 1 ${f(x + w - k)} ${f(y + h)}` +
    `H${f(x + k)}A${c} ${c} 0 0 1 ${x} ${f(y + h - k)}` +
    `V${f(y + k)}A${c} ${c} 0 0 1 ${f(x + k)} ${y}Z`
  );
};

const line = (points: Point[]) =>
  points.map((p, i) => `${i ? "L" : "M"}${f(p.x)} ${f(p.y)}`).join("");

/** A rough width for a text mark, before it is measured. */
const textWidth = (label: string, s: number) =>
  label.length * TEXT_SIZE * 0.55 * s;

/** Where a mark's number sits: its tail, corner or start. */
const tagAt = (mark: Mark, s: number): Point => {
  const [a] = mark.points;
  switch (mark.kind) {
    case "box": {
      const corner = fromPoints(a, mark.points[1] ?? a);
      return { x: corner.x, y: corner.y };
    }
    case "highlight":
      return { x: a.x, y: a.y - (HIGHLIGHT / 2) * s };
    case "text":
      return { x: a.x - (TAG / 2 + 3) * s, y: a.y };
    default:
      return a;
  }
};

const tag = (n: number, at: Point, color: string, s: number): Shape[] => {
  const half = (TAG / 2) * s;
  return [
    {
      d: roundRect(
        { x: at.x - half, y: at.y - half, width: TAG * s, height: TAG * s },
        6 * s,
      ),
      fill: color,
    },
    {
      text: String(n),
      x: f(at.x),
      y: f(at.y + 4.5 * s),
      size: 13 * s,
      fill: "#ffffff",
      anchor: "middle",
    },
  ];
};

/** How mark number `n` looks. */
export const shapes = (mark: Mark, n: number, s: number): Shape[] => {
  const { color, points } = mark;
  const [a] = points;
  const b = points[points.length - 1];
  const stroke = { stroke: color, width: STROKE * s };
  switch (mark.kind) {
    case "arrow": {
      const angle = Math.atan2(b.y - a.y, b.x - a.x);
      const arm = (turn: number) => ({
        x: b.x - HEAD * s * Math.cos(angle + turn),
        y: b.y - HEAD * s * Math.sin(angle + turn),
      });
      return [
        { d: line([a, b]), ...stroke },
        { d: line([arm(0.5), b, arm(-0.5)]), ...stroke },
        ...tag(n, a, color, s),
      ];
    }
    case "box":
      return [
        { d: roundRect(fromPoints(a, b), 6 * s), ...stroke },
        ...tag(n, tagAt(mark, s), color, s),
      ];
    case "highlight":
      return [
        { d: line([a, b]), stroke: color, width: HIGHLIGHT * s, opacity: 0.35 },
        ...tag(n, tagAt(mark, s), color, s),
      ];
    case "pen":
      return [{ d: line(points), ...stroke }, ...tag(n, a, color, s)];
    case "text":
      return [
        {
          text: mark.label ?? "",
          x: f(a.x),
          y: f(a.y + TEXT_SIZE * 0.35 * s),
          size: TEXT_SIZE * s,
          fill: color,
          halo: 4 * s,
          anchor: "start",
        },
        ...tag(n, tagAt(mark, s), color, s),
      ];
    case "pin": {
      const half = (PIN / 2) * s;
      return [
        {
          d: line([
            { x: a.x - 8 * s, y: a.y - 12 * s },
            { x: a.x + 8 * s, y: a.y - 12 * s },
            a,
          ]).concat("Z"),
          fill: color,
        },
        {
          d: roundRect(
            { x: a.x - half, y: a.y - 36 * s, width: PIN * s, height: PIN * s },
            7 * s,
          ),
          fill: color,
        },
        {
          text: String(n),
          x: f(a.x),
          y: f(a.y - 18 * s),
          size: 14 * s,
          fill: "#ffffff",
          anchor: "middle",
        },
      ];
    }
  }
};

/** What a mark covers. */
export const bounds = (mark: Mark, s: number): Rect => {
  const [a] = mark.points;
  if (mark.kind === "pin") {
    return {
      x: a.x - (PIN / 2) * s,
      y: a.y - 36 * s,
      width: PIN * s,
      height: 36 * s,
    };
  }
  if (mark.kind === "text") {
    const height = TEXT_SIZE * s;
    return {
      x: a.x,
      y: a.y - height / 2,
      width: textWidth(mark.label ?? "", s),
      height,
    };
  }
  const xs = mark.points.map((p) => p.x);
  const ys = mark.points.map((p) => p.y);
  const pad = mark.kind === "highlight" ? (HIGHLIGHT / 2) * s : 0;
  return {
    x: Math.min(...xs) - pad,
    y: Math.min(...ys) - pad,
    width: Math.max(...xs) - Math.min(...xs) + pad * 2,
    height: Math.max(...ys) - Math.min(...ys) + pad * 2,
  };
};

/** The mark whose number (or pin, or text) is at `p`, frontmost first:
 * that is what a click picks. */
export const markAt = (marks: Mark[], p: Point, s: number): Mark | null => {
  for (let i = marks.length - 1; i >= 0; i--) {
    const mark = marks[i];
    const at = tagAt(mark, s);
    const half = (TAG / 2) * s;
    const number = {
      x: at.x - half,
      y: at.y - half,
      width: TAG * s,
      height: TAG * s,
    };
    const own = mark.kind === "pin" || mark.kind === "text";
    if (inside(p, number) || (own && inside(p, bounds(mark, s)))) return mark;
  }
  return null;
};

/** The marks an arrow runs between, when it starts and ends on others
 * drawn before it. */
export const joins = (
  marks: Mark[],
  arrow: Mark,
  s: number,
): [Mark, Mark] | null => {
  if (arrow.kind !== "arrow") return null;
  const before = marks
    .slice(0, marks.indexOf(arrow))
    .filter((m) => m.kind !== "arrow");
  const on = (p: Point) => {
    for (let i = before.length - 1; i >= 0; i--) {
      if (inside(p, bounds(before[i], s), REACH * s)) return before[i];
    }
    return null;
  };
  const from = on(arrow.points[0]);
  const to = on(arrow.points[arrow.points.length - 1]);
  return from && to && from !== to ? [from, to] : null;
};

const percent = (n: number) => Math.min(100, Math.max(0, Math.round(n * 100)));

/** The marks as Handy notes them: numbered on from `first`, placed in
 * percent of the grab, and timed back from `now`. */
export const toGrabMarks = (
  marks: Mark[],
  grab: Rect,
  first: number,
  now: number,
  s: number,
): GrabMark[] => {
  const number = (mark: Mark) => first + marks.indexOf(mark);
  return marks.map((mark) => {
    const b = bounds(mark, s);
    const ends = joins(marks, mark, s);
    return {
      n: number(mark),
      kind: mark.kind,
      ago_ms: Math.max(0, Math.round(now - mark.at)),
      x0: percent((b.x - grab.x) / grab.width),
      x1: percent((b.x + b.width - grab.x) / grab.width),
      y0: percent((b.y - grab.y) / grab.height),
      y1: percent((b.y + b.height - grab.y) / grab.height),
      joins: ends ? [number(ends[0]), number(ends[1])] : null,
      label: mark.kind === "text" ? (mark.label ?? "").trim() || null : null,
    };
  });
};

/** Draws shapes on a canvas already set to the frozen screen's pixels. */
export const paint = (context: CanvasRenderingContext2D, list: Shape[]) => {
  context.lineCap = "round";
  context.lineJoin = "round";
  for (const shape of list) {
    context.save();
    if ("d" in shape) {
      const path = new Path2D(shape.d);
      context.globalAlpha = shape.opacity ?? 1;
      if (shape.fill) {
        context.fillStyle = shape.fill;
        context.fill(path);
      }
      if (shape.stroke) {
        context.strokeStyle = shape.stroke;
        context.lineWidth = shape.width ?? 1;
        context.stroke(path);
      }
    } else {
      context.font = `700 ${shape.size}px ${FONT}`;
      context.textAlign = shape.anchor === "middle" ? "center" : "left";
      context.textBaseline = "alphabetic";
      if (shape.halo) {
        context.strokeStyle = "#ffffff";
        context.lineWidth = shape.halo;
        context.strokeText(shape.text, shape.x, shape.y);
      }
      context.fillStyle = shape.fill;
      context.fillText(shape.text, shape.x, shape.y);
    }
    context.restore();
  }
};
