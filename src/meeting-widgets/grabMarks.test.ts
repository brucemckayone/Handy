import assert from "node:assert/strict";
import {
  fit,
  joins,
  markAt,
  shapes,
  toGrabMarks,
  type Mark,
} from "./grabMarks";

const grab = { x: 100, y: 100, width: 400, height: 200 };
const mark = (id: number, kind: Mark["kind"], points: number[][]): Mark => ({
  id,
  kind,
  color: "#da5893",
  points: points.map(([x, y]) => ({ x, y })),
  at: 1000 * id,
});

const box = mark(1, "box", [
  [120, 120],
  [200, 180],
]);
const pin = mark(2, "pin", [[400, 250]]);
const arrow = mark(3, "arrow", [
  [210, 175],
  [395, 240],
]);
const stray = mark(4, "arrow", [
  [300, 110],
  [310, 290],
]);
const marks = [box, pin, arrow, stray];

// An arrow that runs from one mark to another joins them; one that doesn't,
// or that only reaches marks drawn after it, joins nothing.
assert.deepEqual(joins(marks, arrow, 1), [box, pin]);
assert.equal(joins(marks, stray, 1), null);
assert.equal(joins([arrow, box, pin], arrow, 1), null);

// Numbers count on from the question's marks so far, each timed back from
// when the grab is added, and placed in percent of the grab.
const noted = toGrabMarks(marks, grab, 3, 5000, 1);
assert.deepEqual(
  noted.map((m) => [m.n, m.kind, m.ago_ms, m.joins]),
  [
    [3, "box", 4000, null],
    [4, "pin", 3000, null],
    [5, "arrow", 2000, [3, 4]],
    [6, "arrow", 1000, null],
  ],
);
assert.deepEqual(
  [noted[0].x0, noted[0].x1, noted[0].y0, noted[0].y1],
  [5, 25, 10, 40],
);
assert.ok(
  noted.every((m) => m.x0 >= 0 && m.x1 <= 100 && m.y0 >= 0 && m.y1 <= 100),
);

// Only text marks carry a label, and a blank one carries none.
const label = { ...mark(5, "text", [[150, 150]]), label: "  this one " };
const blank = { ...mark(6, "text", [[150, 200]]), label: "   " };
assert.deepEqual(
  toGrabMarks([label, blank], grab, 1, 6000, 1).map((m) => m.label),
  ["this one", null],
);

// A click on a mark's number picks it; the frontmost wins.
assert.equal(markAt(marks, { x: 121, y: 121 }, 1), box);
assert.equal(markAt(marks, { x: 400, y: 230 }, 1), pin);
assert.equal(markAt(marks, { x: 160, y: 150 }, 1), null);

// Each mark shows its number; a pin is its own number.
const text = (m: Mark, n: number) =>
  shapes(m, n, 1).flatMap((s) => ("text" in s ? [s.text] : []));
assert.deepEqual(text(box, 7), ["7"]);
assert.deepEqual(text(pin, 8), ["8"]);
assert.deepEqual(text(label, 9), ["  this one ", "9"]);

// A capture opened again fits the screen, centred, and never grows.
const area = { x: 0, y: 0, width: 1000, height: 500 };
assert.deepEqual(fit(2000, 500, area), {
  x: 0,
  y: 125,
  width: 1000,
  height: 250,
});
assert.deepEqual(fit(200, 100, area), {
  x: 400,
  y: 200,
  width: 200,
  height: 100,
});

console.log("grabMarks: all assertions passed");
