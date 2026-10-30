import React, { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Bookmark, Check, Square } from "lucide-react";
import { commands, events, type RecordingInfo } from "@/bindings";
import { formatElapsed } from "@/components/meetings/format";
import { useElapsed } from "@/components/meetings/useElapsed";

const BARS = [0.55, 1, 0.75, 0.45];

/** A peak level (0..1) as a meter height (0..1), on a -60..0 dB scale. */
const meterLevel = (peak: number): number => {
  if (peak <= 0) return 0;
  const db = 20 * Math.log10(peak);
  return Math.min(1, Math.max(0, (db + 60) / 60));
};

const Meter: React.FC<{
  level: number;
  label: string;
  title: string;
  color: string;
}> = ({ level, label, title, color }) => (
  <span title={title} className="inline-flex items-center gap-2">
    <span className="text-xs text-ink-3">{label}</span>
    <span aria-hidden="true" className="flex items-center gap-[3px] h-[22px]">
      {BARS.map((weight, i) => (
        <span
          key={i}
          className="block w-1 rounded-sm transition-[height] duration-100"
          style={{
            height: `${Math.max(4, Math.round(22 * level * weight))}px`,
            background: color,
          }}
        />
      ))}
    </span>
  </span>
);

/** The pill shown while a meeting records: time, both tracks' levels, and
 * bookmark and stop. Always visible while recording. */
export const MeetingPill: React.FC = () => {
  const { t } = useTranslation();
  const [recording, setRecording] = useState<RecordingInfo | null>(null);
  const [levels, setLevels] = useState({ me: 0, them: 0 });
  const [message, setMessage] = useState<string | null>(null);
  const [bookmarkSeconds, setBookmarkSeconds] = useState(30);
  const smoothed = useRef({ me: 0, them: 0 });
  const messageTimer = useRef<ReturnType<typeof setTimeout>>();
  const elapsed = useElapsed(recording?.started_at);

  useEffect(() => {
    commands.getMeetingRecording().then(setRecording);
    commands.getAppSettings().then((result) => {
      if (result.status === "ok")
        setBookmarkSeconds(result.data.meeting?.bookmark_seconds ?? 30);
    });
    const unlisteners = [
      events.meetingRecordingEvent.listen(({ payload }) => {
        setRecording(payload.recording);
        if (!payload.recording) {
          smoothed.current = { me: 0, them: 0 };
          setLevels({ me: 0, them: 0 });
        }
      }),
      events.meetingLevelsEvent.listen(({ payload }) => {
        // Rise fast, fall slowly, so speech reads as a steady meter.
        const ease = (prev: number, next: number) =>
          next > prev ? next : prev * 0.8 + next * 0.2;
        smoothed.current = {
          me: ease(smoothed.current.me, meterLevel(payload.me)),
          them: ease(smoothed.current.them, meterLevel(payload.them)),
        };
        setLevels(smoothed.current);
      }),
      events.meetingPillMessageEvent.listen(({ payload }) => {
        if (payload.message !== "bookmarked") return;
        commands.getAppSettings().then((result) => {
          const seconds =
            result.status === "ok"
              ? (result.data.meeting?.bookmark_seconds ?? 30)
              : 30;
          setBookmarkSeconds(seconds);
          setMessage(t("meetings.pill.bookmarked", { seconds }));
        });
        clearTimeout(messageTimer.current);
        messageTimer.current = setTimeout(() => setMessage(null), 1800);
      }),
    ];
    return () => {
      clearTimeout(messageTimer.current);
      unlisteners.forEach((u) => u.then((fn) => fn()));
    };
  }, [t]);

  if (!recording) return null;

  return (
    <div className="w-full h-full flex items-center justify-center p-1.5 font-ui">
      {message ? (
        <div
          role="status"
          className="h-[52px] flex items-center gap-2.5 px-5 rounded-full border border-line bg-background text-sm text-ink-2 shadow-md"
        >
          <Check className="w-4 h-4 text-moss" aria-hidden="true" />
          {message}
        </div>
      ) : (
        <div
          role="toolbar"
          aria-label={t("meetings.pill.label")}
          className="h-[52px] flex items-center gap-3.5 ps-4 pe-2 rounded-full border border-line bg-background text-sm text-text shadow-md"
        >
          <span className="inline-flex items-center gap-2 text-[15px] font-semibold tabular-nums">
            <span
              className="w-[9px] h-[9px] rounded-full bg-background-ui animate-pulse"
              aria-hidden="true"
            />
            {formatElapsed(elapsed)}
          </span>
          <span className="w-px h-6 bg-line" aria-hidden="true" />
          <Meter
            level={levels.me}
            label={t("meetings.pill.you")}
            title={t("meetings.pill.yourMic")}
            color="var(--color-background-ui)"
          />
          <Meter
            level={levels.them}
            label={t("meetings.pill.them")}
            title={t("meetings.pill.callAudio")}
            color="var(--color-ink-2)"
          />
          {recording.dictating && (
            <span className="px-2 py-0.5 rounded-full bg-logo-primary/30 text-xs">
              {t("meetings.pill.dictating")}
            </span>
          )}
          <span className="inline-flex gap-1.5">
            <button
              type="button"
              onClick={() => commands.addMeetingBookmark()}
              aria-label={t("meetings.pill.bookmark", {
                seconds: bookmarkSeconds,
              })}
              title={t("meetings.pill.bookmark", { seconds: bookmarkSeconds })}
              className="w-9 h-9 flex items-center justify-center rounded-full bg-wash text-ink-2 hover:bg-logo-primary/30 hover:scale-105 transition-transform cursor-pointer"
            >
              <Bookmark className="w-4 h-4" aria-hidden="true" />
            </button>
            <button
              type="button"
              onClick={() => commands.stopMeeting()}
              aria-label={t("meetings.pill.stop")}
              title={t("meetings.pill.stop")}
              className="w-9 h-9 flex items-center justify-center rounded-full bg-wash text-ink-2 hover:bg-logo-primary/30 hover:scale-105 transition-transform cursor-pointer"
            >
              <Square
                className="w-[13px] h-[13px] fill-current"
                aria-hidden="true"
              />
            </button>
          </span>
        </div>
      )}
    </div>
  );
};
