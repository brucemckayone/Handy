import type { TFunction } from "i18next";
import type { Meeting } from "@/bindings";
import {
  bookmarkLabel,
  formatClock,
  formatDuration,
  ownerName,
  speakerName,
  toTurns,
} from "./format";

const actionLine = (meeting: Meeting, index: number, t: TFunction): string => {
  const item = meeting.summary!.action_items[index];
  const details: string[] = [];
  if (item.owner) details.push(ownerName(meeting, item.owner, t));
  if (item.due) details.push(t("meetings.markdown.due", { due: item.due }));
  if (item.at_ms != null) details.push(formatClock(item.at_ms));
  const suffix = details.length ? ` (${details.join(", ")})` : "";
  return `- [${item.done ? "x" : " "}] ${item.text}${suffix}`;
};

/** The meeting's action items as a Markdown checklist. */
export const actionItemsMarkdown = (meeting: Meeting, t: TFunction): string =>
  (meeting.summary?.action_items ?? [])
    .map((_, i) => actionLine(meeting, i, t))
    .join("\n");

/** The whole meeting as Markdown: summary, decisions, action items,
 * bookmarks and the transcript. */
export const meetingMarkdown = (
  meeting: Meeting,
  t: TFunction,
  locale: string,
): string => {
  const lines: string[] = [];
  const title = meeting.title || t("meetings.untitled");
  lines.push(`# ${title}`, "");
  const date = new Intl.DateTimeFormat(locale, {
    dateStyle: "long",
    timeStyle: "short",
  }).format(new Date(meeting.started_at));
  lines.push(`- **${t("meetings.markdown.date")}:** ${date}`);
  lines.push(
    `- **${t("meetings.markdown.duration")}:** ${formatDuration(meeting.duration_ms, t)}`,
  );
  if (meeting.speakers.length) {
    const names = meeting.speakers.map((s) => speakerName(meeting, s.id, t));
    lines.push(`- **${t("meetings.markdown.speakers")}:** ${names.join(", ")}`);
  }
  lines.push("");

  const summary = meeting.summary;
  if (summary) {
    lines.push(
      `## ${t("meetings.markdown.summary")}`,
      "",
      summary.overview,
      "",
    );
    if (summary.decisions.length) {
      lines.push(`## ${t("meetings.markdown.decisions")}`, "");
      for (const d of summary.decisions) {
        const at = d.at_ms != null ? ` (${formatClock(d.at_ms)})` : "";
        lines.push(`- ${d.text}${at}`);
      }
      lines.push("");
    }
    if (summary.action_items.length) {
      lines.push(
        `## ${t("meetings.markdown.actions")}`,
        "",
        actionItemsMarkdown(meeting, t),
        "",
      );
    }
  }

  if (meeting.bookmarks.length) {
    lines.push(`## ${t("meetings.markdown.bookmarks")}`, "");
    for (const b of meeting.bookmarks) {
      const label = bookmarkLabel(meeting, b);
      const range = `${formatClock(b.start_ms)}–${formatClock(b.end_ms)}`;
      lines.push(label ? `- ${range}: ${label}` : `- ${range}`);
    }
    lines.push("");
  }

  if (meeting.segments.length) {
    lines.push(`## ${t("meetings.markdown.transcript")}`, "");
    for (const turn of toTurns(meeting)) {
      lines.push(
        `**[${formatClock(turn.startMs)}] ${speakerName(meeting, turn.speaker, t)}:** ${turn.text}`,
        "",
      );
    }
  }
  return lines.join("\n").trimEnd() + "\n";
};

/** A file name for the meeting, safe on every platform. */
export const markdownFileName = (meeting: Meeting, t: TFunction): string => {
  const title = (meeting.title || t("meetings.untitled"))
    .replace(/[\\/:*?"<>|]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
  const day = new Date(meeting.started_at).toISOString().slice(0, 10);
  return `${day} ${title}.md`;
};
