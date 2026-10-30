import type { TFunction } from "i18next";
import type { Bookmark, Meeting } from "@/bindings";

/** `m:ss`, or `h:mm:ss` from an hour. */
export const formatClock = (ms: number): string => {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor(total / 60) % 60;
  const s = String(total % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
};

/** A recording's running time: `mm:ss`, or `h:mm:ss` from an hour. */
export const formatElapsed = (ms: number): string => {
  const clock = formatClock(ms);
  return clock.length === 4 ? `0${clock}` : clock;
};

/** "47 min" or "1 h 03 min". */
export const formatDuration = (ms: number, t: TFunction): string => {
  const minutes = Math.max(1, Math.round(ms / 60000));
  if (minutes < 60) return t("meetings.meta.minutes", { count: minutes });
  return t("meetings.meta.hours", {
    hours: Math.floor(minutes / 60),
    minutes: String(minutes % 60).padStart(2, "0"),
  });
};

export const formatWhen = (startedAt: number, locale: string): string => {
  const date = new Date(startedAt);
  const today = new Date();
  const sameDay = date.toDateString() === today.toDateString();
  const time = new Intl.DateTimeFormat(locale, {
    hour: "2-digit",
    minute: "2-digit",
  }).format(date);
  if (sameDay) return time;
  const day = new Intl.DateTimeFormat(locale, {
    weekday: "short",
    day: "numeric",
    month: "short",
  }).format(date);
  return `${day}, ${time}`;
};

export const formatTime = (startedAt: number, locale: string): string =>
  new Intl.DateTimeFormat(locale, {
    hour: "2-digit",
    minute: "2-digit",
  }).format(new Date(startedAt));

/** What a speaker is called: their name for this meeting, else a label. */
export const speakerName = (
  meeting: Pick<Meeting, "speakers">,
  id: string,
  t: TFunction,
): string => {
  const named = meeting.speakers.find((s) => s.id === id)?.name?.trim();
  if (named) return named;
  if (id === "me") return t("meetings.detail.me");
  if (id === "them") return t("meetings.detail.them");
  const number = id.startsWith("s") ? id.slice(1) : id;
  return t("meetings.detail.speaker", { number });
};

/** An action item's owner: a speaker id, or a name the summary gave. */
export const ownerName = (
  meeting: Pick<Meeting, "speakers">,
  owner: string,
  t: TFunction,
): string =>
  meeting.speakers.some((s) => s.id === owner)
    ? speakerName(meeting, owner, t)
    : owner;

/** The summary as text to read aloud: overview, decisions, action items. */
export const summaryToRead = (meeting: Meeting, t: TFunction): string => {
  const summary = meeting.summary;
  if (!summary) return "";
  const parts = [summary.overview];
  if (summary.decisions.length > 0) {
    parts.push(
      t("meetings.markdown.decisions"),
      summary.decisions.map((d) => `- ${d.text}`).join("\n"),
    );
  }
  if (summary.action_items.length > 0) {
    parts.push(
      t("meetings.markdown.actions"),
      summary.action_items
        .map((a) =>
          a.owner
            ? `- ${ownerName(meeting, a.owner, t)}: ${a.text}`
            : `- ${a.text}`,
        )
        .join("\n"),
    );
  }
  return parts.join("\n\n");
};

/** The label the summary gave a bookmark, if it gave one. */
export const bookmarkLabel = (
  meeting: Pick<Meeting, "summary">,
  bookmark: Bookmark,
): string | undefined =>
  meeting.summary?.bookmarks.find(
    (n) => Math.abs(n.start_ms - bookmark.start_ms) < 15000,
  )?.label;

/** One colour per speaker, by position, from the theme's palette. */
const SPEAKER_COLORS = [
  "var(--color-background-ui)",
  "#3b82c4",
  "#c27c1a",
  "var(--color-moss)",
  "#8a5cc2",
  "var(--color-ink-3)",
];

export const speakerColor = (id: string): string => {
  if (id === "me") return SPEAKER_COLORS[0];
  if (id === "them") return SPEAKER_COLORS[5];
  const n = parseInt(id.slice(1), 10);
  return SPEAKER_COLORS[Number.isFinite(n) ? ((n - 1) % 4) + 1 : 5];
};

/** Consecutive segments by one speaker, as one turn. */
export interface Turn {
  speaker: string;
  startMs: number;
  endMs: number;
  text: string;
}

export const toTurns = (meeting: Pick<Meeting, "segments">): Turn[] => {
  const turns: Turn[] = [];
  for (const segment of meeting.segments) {
    const last = turns[turns.length - 1];
    // A long pause starts a new turn even for the same speaker.
    if (
      last &&
      last.speaker === segment.speaker &&
      segment.start_ms - last.endMs < 8000
    ) {
      last.text = `${last.text} ${segment.text}`;
      last.endMs = Math.max(last.endMs, segment.end_ms);
    } else {
      turns.push({
        speaker: segment.speaker,
        startMs: segment.start_ms,
        endMs: segment.end_ms,
        text: segment.text,
      });
    }
  }
  return turns;
};
