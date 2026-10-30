import React from "react";
import { useTranslation } from "react-i18next";
import {
  ChevronRight,
  CircleDot,
  Bookmark,
  RotateCcw,
  Search,
  Settings,
  Square,
  Star,
  Users,
} from "lucide-react";
import type { MeetingListItem, RecordingInfo } from "@/bindings";
import { Button } from "../ui/Button";
import {
  formatDuration,
  formatElapsed,
  formatTime,
  formatWhen,
} from "./format";
import { useElapsed } from "./useElapsed";

interface MeetingListProps {
  meetings: MeetingListItem[];
  loaded: boolean;
  loadFailed: boolean;
  query: string;
  recording: RecordingInfo | null;
  shortcut: string;
  onQuery: (query: string) => void;
  onStart: () => void;
  onStop: () => void;
  onBookmark: () => void;
  onOpen: (id: string) => void;
  onSettings: () => void;
  onRetry: (id: string) => void;
  onDiscard: (id: string) => void;
}

/** Chip and dot colours by status; the rest are working states. */
const BUSY_STYLE = { tone: "bg-wash text-ink-2", dot: "bg-ink-3" };
const STATUS_STYLE: Partial<
  Record<MeetingListItem["status"], typeof BUSY_STYLE>
> = {
  recording: { tone: "bg-logo-primary/30 text-text", dot: "bg-background-ui" },
  ready: { tone: "bg-moss-wash text-moss", dot: "bg-moss" },
  failed: { tone: "bg-error-wash text-error", dot: "bg-error" },
  interrupted: { tone: "bg-warning-wash text-warning", dot: "bg-warning" },
};

/** The status chip on a meeting row. */
const StatusChip: React.FC<{ meeting: MeetingListItem; elapsed: number }> = ({
  meeting,
  elapsed,
}) => {
  const { t } = useTranslation();
  const busy = ["recording", "transcribing", "labelling", "summarising"];
  let label: string;
  switch (meeting.status) {
    case "recording":
      label = t("meetings.status.recording", {
        elapsed: formatElapsed(elapsed),
      });
      break;
    case "transcribing":
      label = t("meetings.status.transcribing", {
        percent: meeting.progress ?? 0,
      });
      break;
    default:
      label = t(`meetings.status.${meeting.status}`);
  }
  const { tone, dot } = STATUS_STYLE[meeting.status] ?? BUSY_STYLE;
  return (
    <span
      className={`inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full text-xs whitespace-nowrap ${tone}`}
    >
      <span
        className={`w-1.5 h-1.5 rounded-full ${dot} ${busy.includes(meeting.status) ? "animate-pulse" : ""}`}
        aria-hidden="true"
      />
      {label}
    </span>
  );
};

const MeetingRow: React.FC<{
  meeting: MeetingListItem;
  recording: RecordingInfo | null;
  onOpen: () => void;
  onBookmark: () => void;
  onRetry: () => void;
}> = ({ meeting, recording, onOpen, onBookmark, onRetry }) => {
  const { t, i18n } = useTranslation();
  const isRecording =
    meeting.status === "recording" && recording?.id === meeting.id;
  const elapsed = useElapsed(isRecording ? meeting.started_at : null);

  const meta: string[] = [];
  meta.push(
    isRecording
      ? t("meetings.meta.started", {
          time: formatTime(meeting.started_at, i18n.language),
        })
      : formatWhen(meeting.started_at, i18n.language),
  );
  if (meeting.duration_ms > 0)
    meta.push(formatDuration(meeting.duration_ms, t));
  if (meeting.speaker_count > 0) {
    meta.push(t("meetings.meta.speakers", { count: meeting.speaker_count }));
  } else if (isRecording) {
    meta.push(t("meetings.meta.youAndCall"));
  }
  if (meeting.bookmark_count > 0) {
    meta.push(t("meetings.meta.bookmarks", { count: meeting.bookmark_count }));
  }

  const title = meeting.title || t("meetings.untitled");
  const failedMessage =
    meeting.error === "no_audio"
      ? t("meetings.failed.noAudio")
      : t("meetings.failed.generic", { error: meeting.error ?? "" });

  return (
    <div className="px-4 py-3 space-y-1.5">
      <div className="flex items-start justify-between gap-3">
        <button
          type="button"
          onClick={onOpen}
          className="text-start text-sm font-semibold hover:text-logo-primary cursor-pointer truncate"
        >
          {meeting.starred && (
            <Star
              className="w-3.5 h-3.5 inline me-1 -mt-0.5 text-logo-primary fill-current"
              aria-hidden="true"
            />
          )}
          {title}
        </button>
        <div className="flex items-center gap-2 shrink-0">
          {isRecording && (
            <Button variant="secondary" size="sm" onClick={onBookmark}>
              <span className="inline-flex items-center gap-1">
                <Bookmark className="w-3 h-3" aria-hidden="true" />
                {t("meetings.detail.bookmark")}
              </span>
            </Button>
          )}
          <StatusChip meeting={meeting} elapsed={elapsed} />
        </div>
      </div>
      <div className="text-xs text-ink-3">{meta.join(" · ")}</div>
      {meeting.status === "transcribing" && (
        <div
          className="h-1 rounded-full bg-line overflow-hidden"
          role="progressbar"
          aria-valuenow={meeting.progress ?? 0}
          aria-valuemin={0}
          aria-valuemax={100}
        >
          <div
            className="h-full bg-background-ui transition-[width] duration-500"
            style={{ width: `${meeting.progress ?? 0}%` }}
          />
        </div>
      )}
      {meeting.status === "ready" && meeting.overview && (
        <p className="text-sm text-ink-2 line-clamp-2">{meeting.overview}</p>
      )}
      {meeting.status === "ready" && meeting.error === "no_speech" && (
        <p className="text-sm text-ink-3">{t("meetings.noSpeech")}</p>
      )}
      {meeting.status === "ready" && (
        <div className="flex items-center justify-between text-xs text-ink-3">
          <span>
            {meeting.open_actions > 0
              ? t("meetings.meta.actions", { count: meeting.open_actions })
              : ""}
          </span>
          <button
            type="button"
            onClick={onOpen}
            className="inline-flex items-center gap-0.5 font-medium text-logo-primary hover:underline cursor-pointer"
          >
            {t("meetings.meta.open")}
            <ChevronRight
              className="w-3.5 h-3.5 rtl:rotate-180"
              aria-hidden="true"
            />
          </button>
        </div>
      )}
      {meeting.status === "failed" && (
        <div className="flex items-center justify-between gap-2 text-xs">
          <span className="text-error">{failedMessage}</span>
          {meeting.has_audio && (
            <Button variant="secondary" size="sm" onClick={onRetry}>
              <span className="inline-flex items-center gap-1">
                <RotateCcw className="w-3 h-3" aria-hidden="true" />
                {t("meetings.failed.retry")}
              </span>
            </Button>
          )}
        </div>
      )}
    </div>
  );
};

export const MeetingList: React.FC<MeetingListProps> = ({
  meetings,
  loaded,
  loadFailed,
  query,
  recording,
  shortcut,
  onQuery,
  onStart,
  onStop,
  onBookmark,
  onOpen,
  onSettings,
  onRetry,
  onDiscard,
}) => {
  const { t, i18n } = useTranslation();
  const elapsed = useElapsed(recording?.started_at);
  const interrupted = meetings.filter((m) => m.status === "interrupted");
  const listed = meetings.filter((m) => m.status !== "interrupted");

  let body: React.ReactNode;
  if (!loaded) {
    body = (
      <div className="px-4 py-3 text-center text-ink-2">
        {t("meetings.loading")}
      </div>
    );
  } else if (loadFailed) {
    body = (
      <div className="px-4 py-3 text-center text-error">
        {t("meetings.loadFailed")}
      </div>
    );
  } else if (listed.length === 0 && query.trim()) {
    body = (
      <div className="px-4 py-3 text-center text-ink-2">
        {t("meetings.noMatch")}
      </div>
    );
  } else if (listed.length === 0) {
    body = (
      <div className="px-6 py-8 flex flex-col items-center text-center gap-2">
        <Users className="w-8 h-8 text-ink-3" aria-hidden="true" />
        <h3 className="text-sm font-semibold">{t("meetings.empty.title")}</h3>
        <p className="text-sm text-ink-2 max-w-md">
          {t("meetings.empty.body")}
        </p>
        <p className="text-xs text-ink-3 max-w-md">
          {t("meetings.empty.hint", { shortcut })}
        </p>
        {!recording && (
          <Button className="mt-2" onClick={onStart}>
            {t("meetings.empty.start")}
          </Button>
        )}
      </div>
    );
  } else {
    body = (
      <div className="divide-y divide-line">
        {listed.map((meeting) => (
          <MeetingRow
            key={meeting.id}
            meeting={meeting}
            recording={recording}
            onOpen={() => onOpen(meeting.id)}
            onBookmark={onBookmark}
            onRetry={() => onRetry(meeting.id)}
          />
        ))}
      </div>
    );
  }

  return (
    <div className="max-w-3xl w-full mx-auto space-y-4">
      <div className="px-4 flex items-center justify-between gap-2">
        <h2 className="text-xs font-medium text-ink-3 uppercase tracking-wide">
          {t("meetings.title")}
        </h2>
        <div className="flex items-center gap-2">
          {recording ? (
            <Button size="sm" onClick={onStop}>
              <span className="inline-flex items-center gap-1.5 tabular-nums">
                <Square className="w-3 h-3 fill-current" aria-hidden="true" />
                {t("meetings.stop", { elapsed: formatElapsed(elapsed) })}
              </span>
            </Button>
          ) : (
            <Button size="sm" onClick={onStart}>
              <span className="inline-flex items-center gap-1.5">
                <CircleDot className="w-3.5 h-3.5" aria-hidden="true" />
                {t("meetings.start")}
              </span>
            </Button>
          )}
          <button
            type="button"
            onClick={onSettings}
            aria-label={t("meetings.settings")}
            title={t("meetings.settings")}
            className="p-1.5 rounded-md text-ink-3 hover:text-logo-primary cursor-pointer"
          >
            <Settings className="w-4 h-4" aria-hidden="true" />
          </button>
        </div>
      </div>

      {interrupted.map((meeting) => (
        <div
          key={meeting.id}
          role="status"
          className="mx-0 flex items-start gap-3 px-4 py-3 rounded-lg border border-warning bg-warning-wash"
        >
          <div className="flex-1 min-w-0">
            <div className="text-sm font-semibold">
              {t("meetings.recovered.title")}
            </div>
            <div className="text-sm text-ink-2">
              {t("meetings.recovered.body", {
                time: formatTime(meeting.started_at, i18n.language),
                duration: formatDuration(meeting.duration_ms, t),
              })}
            </div>
          </div>
          <div className="flex gap-2 shrink-0">
            <Button
              variant="warning"
              size="sm"
              onClick={() => onRetry(meeting.id)}
            >
              {t("meetings.recovered.transcribe")}
            </Button>
            <Button
              variant="danger-ghost"
              size="sm"
              onClick={() => onDiscard(meeting.id)}
            >
              {t("meetings.recovered.discard")}
            </Button>
          </div>
        </div>
      ))}

      <label className="relative block">
        <span className="sr-only">{t("meetings.searchLabel")}</span>
        <Search
          className="absolute start-3 top-1/2 -translate-y-1/2 w-4 h-4 text-ink-3"
          aria-hidden="true"
        />
        <input
          type="search"
          value={query}
          onChange={(e) => onQuery(e.target.value)}
          placeholder={t("meetings.search")}
          className="w-full ps-9 pe-3 py-1.5 text-sm rounded-lg border border-line bg-background focus:outline-none focus:border-logo-primary"
        />
      </label>

      <div className="bg-background border border-line rounded-lg overflow-visible">
        {body}
      </div>
    </div>
  );
};
