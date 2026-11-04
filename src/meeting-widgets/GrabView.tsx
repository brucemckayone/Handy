import React, { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { convertFileSrc } from "@tauri-apps/api/core";
import { commands, events, type GrabFrame } from "@/bindings";

/** Part of the frozen screen, in its pixels. */
type Rect = { x: number; y: number; width: number; height: number };
type Point = { x: number; y: number };

/** The picture sent to Claude is at most this wide, as with screenshots. */
const MAX_WIDTH = 1600;
/** The pill's thumbnails, at twice their size for high-DPI screens. */
const THUMBNAIL_WIDTH = 96;
/** A press that moves less than this (screen pixels) is a click, not a drag. */
const CLICK_SLOP = 4;
/** The dim layer over everything not being grabbed. */
const DIM = "rgba(24, 20, 28, 0.55)";

const fromPoints = (a: Point, b: Point): Rect => ({
  x: Math.min(a.x, b.x),
  y: Math.min(a.y, b.y),
  width: Math.abs(a.x - b.x),
  height: Math.abs(a.y - b.y),
});

/** `part` of `image` as a PNG data URL, at most `maxWidth` wide. */
const crop = (image: HTMLImageElement, part: Rect, maxWidth: number) => {
  const scale = Math.min(1, maxWidth / part.width);
  const canvas = document.createElement("canvas");
  canvas.width = Math.max(1, Math.round(part.width * scale));
  canvas.height = Math.max(1, Math.round(part.height * scale));
  const context = canvas.getContext("2d");
  context?.drawImage(
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
  return canvas.toDataURL("image/png");
};

const kbd =
  "font-[inherit] text-[11px] leading-[15px] px-1 rounded border border-line border-b-2 bg-background text-text";

/** The grab overlay: the screen under the mouse, frozen. Drag a region,
 * click a window or press F for the whole screen, then Enter adds it to the
 * question (Ctrl+Enter adds it and asks); Esc drops it. */
export const GrabView: React.FC = () => {
  const { t } = useTranslation();
  const [frame, setFrame] = useState<GrabFrame | null>(null);
  const [image, setImage] = useState<HTMLImageElement | null>(null);
  const [drag, setDrag] = useState<{ start: Point; end: Point } | null>(null);
  const [mouse, setMouse] = useState<Point | null>(null);
  const [picked, setPicked] = useState<{ rect: Rect; of: string } | null>(null);
  const busy = useRef(false);

  const show = useCallback((next: GrabFrame | null) => {
    setFrame(next);
    setImage(null);
    setDrag(null);
    setPicked(null);
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

  const drop = useCallback(() => {
    if (busy.current) return;
    commands.closeAskGrab();
  }, []);

  const add = useCallback(
    async (askNow: boolean) => {
      if (!picked || !image || !frame || busy.current) return;
      busy.current = true;
      const png = crop(image, picked.rect, MAX_WIDTH).replace(
        /^data:image\/png;base64,/,
        "",
      );
      const thumbnail = crop(image, picked.rect, THUMBNAIL_WIDTH);
      const result = await commands.finishAskGrab({
        png,
        thumbnail,
        of: picked.of,
        marks: [],
        ask_now: askNow,
      });
      if (result.status === "error") {
        console.error("Couldn't add the grab:", result.error);
        busy.current = false;
      }
    },
    [picked, image, frame],
  );

  useEffect(() => {
    if (!frame) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        drop();
      } else if (e.key === "Enter" && picked) {
        e.preventDefault();
        add(e.ctrlKey || e.metaKey);
      } else if ((e.key === "f" || e.key === "F") && !picked) {
        e.preventDefault();
        setPicked({
          rect: { x: 0, y: 0, width: frame.width, height: frame.height },
          of: t("ask.grab.wholeScreen"),
        });
      }
    };
    window.addEventListener("keydown", onKey);
    // Handy's cancel shortcut takes Esc before this page sees it.
    const unlisten = events.askGrabEscapeEvent.listen(() => drop());
    return () => {
      window.removeEventListener("keydown", onKey);
      unlisten.then((u) => u());
    };
  }, [frame, picked, add, drop, t]);

  if (!frame || !image) {
    return <div className="fixed inset-0 bg-black" />;
  }

  const onMouseDown = (e: React.MouseEvent) => {
    if (picked || e.button !== 0) return;
    const p = at(e);
    setDrag({ start: p, end: p });
  };
  const onMouseMove = (e: React.MouseEvent) => {
    const p = at(e);
    setMouse(p);
    if (drag) setDrag({ ...drag, end: p });
  };
  const onMouseUp = (e: React.MouseEvent) => {
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
  const toolbarBelow =
    picked && css(picked.rect.y + picked.rect.height) + 56 < window.innerHeight;

  return (
    <div
      className={`fixed inset-0 select-none overflow-hidden font-ui text-text ${
        picked ? "cursor-default" : "cursor-crosshair"
      }`}
      onMouseDown={onMouseDown}
      onMouseMove={onMouseMove}
      onMouseUp={onMouseUp}
    >
      <img
        src={image.src}
        alt=""
        draggable={false}
        className="absolute inset-0 w-screen h-screen"
      />
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
              stroke: "var(--color-logo-primary)",
            }}
          />
        )}
      </svg>

      {shown && (drag || picked) && (
        <span
          className="absolute h-6 px-2 flex items-center rounded-md bg-[rgba(31,26,34,0.92)] text-white text-xs tabular-nums"
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

      {picked ? (
        <>
          <div className="absolute top-3 inset-x-0 flex justify-center pointer-events-none">
            <span className="h-7 px-3 flex items-center gap-2 rounded-full bg-[rgba(31,26,34,0.92)] text-white text-xs">
              {t("ask.grab.stillListening")}
            </span>
          </div>
          <div
            className="absolute h-10 flex items-center gap-1.5 px-1.5 rounded-[10px] border border-line bg-background shadow-lg text-[13px]"
            style={{
              left: css(picked.rect.x),
              top: toolbarBelow
                ? css(picked.rect.y + picked.rect.height) + 8
                : Math.max(8, css(picked.rect.y) - 48),
            }}
            onMouseDown={(e) => e.stopPropagation()}
          >
            <button
              type="button"
              className="h-7 px-2.5 flex items-center gap-1.5 rounded-md hover:bg-wash cursor-pointer"
              onClick={drop}
            >
              {t("ask.grab.drop")}
              <kbd className={kbd}>{t("ask.grab.keyEsc")}</kbd>
            </button>
            <button
              type="button"
              className="h-7 px-2.5 flex items-center gap-1.5 rounded-md hover:bg-wash cursor-pointer"
              onClick={() => add(false)}
            >
              {t("ask.grab.add")}
              <kbd className={kbd}>{t("ask.grab.keyEnter")}</kbd>
            </button>
            <button
              type="button"
              className="h-7 px-2.5 flex items-center gap-1.5 rounded-md bg-logo-primary text-white font-semibold cursor-pointer"
              onClick={() => add(true)}
            >
              {t("ask.grab.addAndAsk")}
              <kbd className={kbd}>{t("ask.grab.keyCtrlEnter")}</kbd>
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
    </div>
  );
};
