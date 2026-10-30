import React, {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { save } from "@tauri-apps/plugin-dialog";
import {
  Bookmark,
  Check,
  ChevronDown,
  ChevronLeft,
  ClipboardList,
  Copy,
  Download,
  Info,
  Keyboard,
  Play,
  RotateCcw,
  Sparkles,
  Square,
  Star,
  Trash2,
} from "lucide-react";
import { commands, events, type Meeting, type SummaryEngine } from "@/bindings";
import { usePressAway } from "@/hooks/usePressAway";
import { useSettings } from "@/hooks/useSettings";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { useReadAloud } from "../read-aloud/useReadAloud";
import {
  bookmarkLabel,
  formatClock,
  formatDuration,
  formatWhen,
  ownerName,
  speakerColor,
  speakerName,
  summaryToRead,
  toTurns,
} from "./format";
import {
  actionItemsMarkdown,
  markdownFileName,
  meetingMarkdown,
} from "./exportMarkdown";

interface MeetingDetailProps {
  id: string;
  onBack: () => void;
}

/** A small menu under a button; closes when clicking elsewhere. */
const Menu: React.FC<{
  label: string;
  icon: React.ComponentType<{ className?: string }>;
  children: (close: () => void) => React.ReactNode;
}> = ({ label, icon: Icon, children }) => {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  usePressAway(open, ref, () => setOpen(false));
  return (
    <div ref={ref} className="relative">
      <Button
        variant="secondary"
        size="sm"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
      >
        <span className="inline-flex items-center gap-1">
          <Icon className="w-3.5 h-3.5" />
          {label}
          <ChevronDown className="w-3 h-3" />
        </span>
      </Button>
      {open && (
        <div
          role="menu"
          className="absolute end-0 top-full mt-1 z-20 min-w-56 p-1 rounded-lg border border-line bg-background shadow-lg"
        >
          {children(() => setOpen(false))}
        </div>
      )}
    </div>
  );
};

const MenuItem: React.FC<{
  onClick: () => void;
  icon: React.ComponentType<{ className?: string }>;
  title: string;
  detail?: string;
}> = ({ onClick, icon: Icon, title, detail }) => (
  <button
    role="menuitem"
    type="button"
    onClick={onClick}
    className="w-full flex items-start gap-2 px-2.5 py-1.5 rounded-md text-start text-sm hover:bg-wash cursor-pointer"
  >
    <Icon className="w-4 h-4 mt-0.5 shrink-0 text-ink-3" />
    <span className="flex flex-col">
      <span>{title}</span>
      {detail && <span className="text-xs text-ink-3">{detail}</span>}
    </span>
  </button>
);

/** One speaker's name, renamed in place for this meeting only. */
const SpeakerChip: React.FC<{
  meeting: Meeting;
  id: string;
  share: string;
  onRename: (name: string | null) => void;
}> = ({ meeting, id, share, onRename }) => {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const current = meeting.speakers.find((s) => s.id === id)?.name ?? "";
  const [draft, setDraft] = useState(current);
  const unnamed = !current && id.startsWith("s");
  const finish = () => {
    setEditing(false);
    if (draft.trim() !== current) onRename(draft.trim() || null);
  };
  const dot = (
    <span
      className="w-2 h-2 rounded-full shrink-0"
      style={{ background: speakerColor(id) }}
      aria-hidden="true"
    />
  );
  if (editing) {
    return (
      <span className="inline-flex items-center gap-1.5 ps-2 pe-1 py-0.5 rounded-full border border-logo-primary bg-background">
        {dot}
        <input
          autoFocus
          aria-label={t("meetings.detail.nameLabel")}
          placeholder={t("meetings.detail.namePlaceholder")}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") finish();
            if (e.key === "Escape") {
              setDraft(current);
              setEditing(false);
            }
          }}
          onBlur={finish}
          className="w-28 text-sm bg-transparent focus:outline-none"
        />
        <button
          type="button"
          onMouseDown={(e) => e.preventDefault()}
          onClick={finish}
          aria-label={t("meetings.detail.saveName")}
          className="p-0.5 rounded-full hover:bg-wash cursor-pointer"
        >
          <Check className="w-3.5 h-3.5" />
        </button>
      </span>
    );
  }
  return (
    <button
      type="button"
      onClick={() => {
        setDraft(current);
        setEditing(true);
      }}
      title={
        unnamed ? t("meetings.detail.whoIsThis") : t("meetings.detail.rename")
      }
      className="inline-flex items-center gap-1.5 px-2 py-0.5 rounded-full border border-line hover:border-logo-primary text-sm cursor-pointer"
    >
      {dot}
      {speakerName(meeting, id, t)}
      <span className="text-xs text-ink-3">
        {id === "me"
          ? t("meetings.detail.yourMic")
          : unnamed
            ? t("meetings.detail.whoIsThis")
            : share}
      </span>
    </button>
  );
};

type TranscriptItem =
  | { kind: "turn"; at: number; turn: ReturnType<typeof toTurns>[number] }
  | { kind: "bookmark"; at: number; end: number; label?: string }
  | { kind: "dictation"; at: number; end: number; text: string | null };

export const MeetingDetail: React.FC<MeetingDetailProps> = ({ id, onBack }) => {
  const { t, i18n } = useTranslation();
  const { settings } = useSettings();
  const [meeting, setMeeting] = useState<Meeting | null>(null);
  const [missing, setMissing] = useState(false);
  const [tab, setTab] = useState<"summary" | "transcript">("summary");
  const { speaking, read, stop } = useReadAloud();
  const [activeMs, setActiveMs] = useState<number | null>(null);
  const [shownDictation, setShownDictation] = useState<number | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [title, setTitle] = useState("");
  const scrollPending = useRef(false);
  // While the title is being typed, a refresh mustn't overwrite it.
  const editingTitle = useRef(false);

  const load = useCallback(async () => {
    const result = await commands.getMeeting(id);
    if (result.status === "ok") {
      if (result.data) {
        setMeeting(result.data);
        if (!editingTitle.current) setTitle(result.data.title);
      } else {
        setMissing(true);
      }
    }
  }, [id]);

  useEffect(() => {
    load();
    const unlisten = events.meetingChangedEvent.listen(({ payload }) => {
      if (payload.id !== id) return;
      if (payload.meeting) load();
      else setMissing(true);
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, [id, load]);

  useEffect(() => {
    if (!scrollPending.current) return;
    scrollPending.current = false;
    document
      .querySelector('[data-turn-active="yes"]')
      ?.scrollIntoView({ block: "center", behavior: "smooth" });
  });

  const jump = (ms: number) => {
    scrollPending.current = true;
    setActiveMs(ms);
    setTab("transcript");
  };

  const turns = useMemo(() => (meeting ? toTurns(meeting) : []), [meeting]);

  const items = useMemo<TranscriptItem[]>(() => {
    if (!meeting) return [];
    const list: TranscriptItem[] = turns.map((turn) => ({
      kind: "turn",
      at: turn.startMs,
      turn,
    }));
    for (const b of meeting.bookmarks) {
      const label = bookmarkLabel(meeting, b);
      list.push({ kind: "bookmark", at: b.start_ms, end: b.end_ms, label });
    }
    if (settings?.meeting?.leave_out_dictation ?? true) {
      for (const d of meeting.dictations) {
        list.push({
          kind: "dictation",
          at: d.start_ms,
          end: d.end_ms,
          text: d.text,
        });
      }
    }
    return list.sort((a, b) => a.at - b.at);
  }, [meeting, turns, settings?.meeting?.leave_out_dictation]);

  if (missing) {
    return (
      <div className="max-w-3xl w-full mx-auto space-y-4">
        <BackLink onBack={onBack} />
      </div>
    );
  }
  if (!meeting) {
    return (
      <div className="max-w-3xl w-full mx-auto px-4 py-3 text-center text-ink-2">
        {t("meetings.loading")}
      </div>
    );
  }

  const duration =
    meeting.duration_ms ||
    meeting.segments.reduce((max, s) => Math.max(max, s.end_ms), 0);
  const talk: Record<string, number> = {};
  for (const s of meeting.segments) {
    talk[s.speaker] =
      (talk[s.speaker] ?? 0) + Math.max(0, s.end_ms - s.start_ms);
  }
  const totalTalk = Object.values(talk).reduce((a, b) => a + b, 0) || 1;
  const share = (speaker: string) =>
    `${Math.round(((talk[speaker] ?? 0) / totalTalk) * 100)}%`;
  const speakers = meeting.speakers.map((s) => s.id);
  const summary = meeting.summary;
  const openCount = summary?.action_items.filter((a) => !a.done).length ?? 0;
  const engineName = (engine: SummaryEngine) => t(`meetings.engines.${engine}`);
  const busySummarising = meeting.status === "summarising";
  const localModel =
    settings?.post_process_models?.custom?.trim() || "llama3.2:3b";

  const meta = [
    formatWhen(meeting.started_at, i18n.language),
    formatDuration(meeting.duration_ms, t),
  ];
  if (speakers.length) {
    meta.push(t("meetings.meta.speakers", { count: speakers.length }));
  }
  meta.push(
    meeting.has_audio
      ? t("meetings.meta.audioKept")
      : t("meetings.meta.audioDeleted"),
  );

  const saveTitle = async () => {
    if (title.trim() === meeting.title) return;
    await commands.renameMeeting(meeting.id, title);
  };

  const rename = async (speaker: string, name: string | null) => {
    await commands.renameMeetingSpeaker(meeting.id, speaker, name);
  };

  const copy = async (text: string, message: string) => {
    const result = await commands.copyMeetingText(text);
    if (result.status === "ok") toast.success(message);
    else
      toast.error(t("meetings.detail.exportFailed", { error: result.error }));
  };

  const exportCopy = () =>
    copy(
      meetingMarkdown(meeting, t, i18n.language),
      t("meetings.detail.copiedMarkdown"),
    );

  const exportActions = () =>
    copy(
      actionItemsMarkdown(meeting, t),
      t("meetings.detail.copiedActions", {
        count: summary?.action_items.length ?? 0,
      }),
    );

  const exportSave = async () => {
    try {
      const path = await save({
        defaultPath: markdownFileName(meeting, t),
        filters: [{ name: "Markdown", extensions: ["md"] }],
      });
      if (!path) return;
      const result = await commands.saveMeetingMarkdown(
        path,
        meetingMarkdown(meeting, t, i18n.language),
      );
      if (result.status === "ok") {
        const file = path.split(/[\\/]/).pop() ?? path;
        toast.success(t("meetings.detail.savedFile", { file }));
      } else {
        toast.error(t("meetings.detail.exportFailed", { error: result.error }));
      }
    } catch (error) {
      toast.error(t("meetings.detail.exportFailed", { error: String(error) }));
    }
  };

  const summarise = (engine: SummaryEngine | null) =>
    commands.summariseMeeting(meeting.id, engine);

  const summaryError = (error: string) =>
    t(`meetings.summaryErrors.${error}`, {
      defaultValue: t("meetings.detail.summaryFailed", { error }),
    });

  const statusNote = (() => {
    switch (meeting.status) {
      case "recording":
        return t("meetings.status.recording", { elapsed: "" });
      case "waiting":
        return t("meetings.status.waiting");
      case "transcribing":
        return t("meetings.status.transcribing", {
          percent: meeting.progress ?? 0,
        });
      case "labelling":
        return t("meetings.status.labelling");
      case "interrupted":
        return t("meetings.status.interrupted");
      default:
        return null;
    }
  })();

  return (
    <div className="max-w-3xl w-full mx-auto space-y-4">
      <div className="px-1 space-y-1">
        <BackLink onBack={onBack} />
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0 flex-1">
            <input
              aria-label={t("meetings.detail.titleLabel")}
              title={t("meetings.detail.titleHint")}
              value={title}
              placeholder={t("meetings.untitled")}
              onChange={(e) => setTitle(e.target.value)}
              onFocus={() => {
                editingTitle.current = true;
              }}
              onBlur={() => {
                editingTitle.current = false;
                saveTitle();
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") (e.target as HTMLInputElement).blur();
              }}
              className="w-full text-lg font-semibold bg-transparent rounded-md px-1 -mx-1 border border-transparent hover:border-line focus:border-logo-primary focus:outline-none"
            />
            <div className="text-xs text-ink-3">{meta.join(" · ")}</div>
          </div>
          <div className="flex items-center gap-1.5 shrink-0">
            {meeting.segments.length > 0 && (
              <Menu label={t("meetings.detail.export")} icon={Download}>
                {(close) => (
                  <>
                    <MenuItem
                      icon={Copy}
                      title={t("meetings.detail.copyMarkdown")}
                      onClick={() => {
                        close();
                        exportCopy();
                      }}
                    />
                    {(summary?.action_items.length ?? 0) > 0 && (
                      <MenuItem
                        icon={ClipboardList}
                        title={t("meetings.detail.copyActions")}
                        onClick={() => {
                          close();
                          exportActions();
                        }}
                      />
                    )}
                    <MenuItem
                      icon={Download}
                      title={t("meetings.detail.saveMarkdown")}
                      onClick={() => {
                        close();
                        exportSave();
                      }}
                    />
                  </>
                )}
              </Menu>
            )}
            {meeting.segments.length > 0 && !busySummarising && (
              <Menu
                label={
                  summary
                    ? t("meetings.detail.resummarise")
                    : t("meetings.detail.summarise")
                }
                icon={Sparkles}
              >
                {(close) => (
                  <>
                    <MenuItem
                      icon={Sparkles}
                      title={t("meetings.engines.claude_code")}
                      detail={t("meetings.engines.claude_codeDetail")}
                      onClick={() => {
                        close();
                        summarise("claude_code");
                        setTab("summary");
                      }}
                    />
                    <MenuItem
                      icon={Sparkles}
                      title={t("meetings.engines.local")}
                      detail={t("meetings.engines.localDetail", {
                        model: localModel,
                      })}
                      onClick={() => {
                        close();
                        summarise("local");
                        setTab("summary");
                      }}
                    />
                  </>
                )}
              </Menu>
            )}
            <button
              type="button"
              onClick={() =>
                commands.setMeetingStarred(meeting.id, !meeting.starred)
              }
              aria-pressed={meeting.starred}
              aria-label={t("meetings.star")}
              title={t("meetings.star")}
              className={`p-1.5 rounded-md cursor-pointer ${meeting.starred ? "text-logo-primary" : "text-ink-3 hover:text-logo-primary"}`}
            >
              <Star
                className={`w-4 h-4 ${meeting.starred ? "fill-current" : ""}`}
              />
            </button>
            {meeting.status !== "recording" && (
              <button
                type="button"
                onClick={() => setConfirmDelete(true)}
                aria-label={t("meetings.delete")}
                title={t("meetings.delete")}
                className="p-1.5 rounded-md text-ink-3 hover:text-error cursor-pointer"
              >
                <Trash2 className="w-4 h-4" />
              </button>
            )}
          </div>
        </div>
      </div>

      {statusNote && (
        <div className="px-4 py-2 rounded-lg bg-wash text-sm text-ink-2 flex items-center justify-between gap-2">
          <span>{statusNote}</span>
          {meeting.status === "interrupted" && (
            <Button
              variant="secondary"
              size="sm"
              onClick={() => commands.retryMeeting(meeting.id)}
            >
              {t("meetings.recovered.transcribe")}
            </Button>
          )}
        </div>
      )}
      {meeting.status === "failed" && (
        <div className="px-4 py-2 rounded-lg bg-error-wash text-sm flex items-center justify-between gap-2">
          <span className="text-error">
            {meeting.error === "no_audio"
              ? t("meetings.failed.noAudio")
              : t("meetings.failed.generic", { error: meeting.error ?? "" })}
          </span>
          {meeting.has_audio && (
            <Button
              variant="secondary"
              size="sm"
              onClick={() => commands.retryMeeting(meeting.id)}
            >
              <span className="inline-flex items-center gap-1">
                <RotateCcw className="w-3 h-3" />
                {t("meetings.failed.retry")}
              </span>
            </Button>
          )}
        </div>
      )}
      {meeting.problems.length > 0 && (
        <ul className="px-4 text-xs text-warning space-y-0.5">
          {meeting.problems.map((p, i) => (
            <li key={i}>
              {t(`meetings.problems.${p.kind}`, {
                track: t(`meetings.problems.${p.track}`),
                time: formatClock(p.at_ms),
              })}
            </li>
          ))}
        </ul>
      )}

      {speakers.length > 0 && (
        <div className="flex items-center gap-2 flex-wrap px-1">
          <span className="text-xs font-medium text-ink-3 uppercase tracking-wide">
            {t("meetings.detail.speakers")}
          </span>
          {speakers.map((s) => (
            <SpeakerChip
              key={s}
              meeting={meeting}
              id={s}
              share={share(s)}
              onRename={(name) => rename(s, name)}
            />
          ))}
        </div>
      )}

      {meeting.segments.length > 0 && duration > 0 && (
        <section
          aria-labelledby="who-spoke-when"
          className="bg-background border border-line rounded-lg px-4 py-3 space-y-1.5"
        >
          <div className="flex items-baseline justify-between gap-2">
            <h3
              id="who-spoke-when"
              className="text-xs font-medium text-ink-3 uppercase tracking-wide"
            >
              {t("meetings.detail.timeline")}
            </h3>
            <span className="text-xs text-ink-3">
              {t("meetings.detail.timelineHint")}
            </span>
          </div>
          {meeting.bookmarks.length > 0 && (
            <div className="grid grid-cols-[6rem_1fr_2.5rem] gap-2 items-center">
              <span />
              <div className="relative h-4">
                {meeting.bookmarks.map((b, i) => {
                  const range = `${formatClock(b.start_ms)}–${formatClock(b.end_ms)}`;
                  return (
                    <button
                      key={i}
                      type="button"
                      onClick={() => jump(b.start_ms)}
                      aria-label={t("meetings.detail.bookmarkTick", { range })}
                      title={t("meetings.detail.bookmarkTick", { range })}
                      className="absolute -translate-x-1/2 text-logo-primary cursor-pointer"
                      style={{ left: `${(b.start_ms / duration) * 100}%` }}
                    >
                      <Bookmark className="w-3.5 h-3.5 fill-current" />
                    </button>
                  );
                })}
              </div>
              <span />
            </div>
          )}
          {speakers.map((s) => {
            const name = speakerName(meeting, s, t);
            return (
              <div
                key={s}
                className="grid grid-cols-[6rem_1fr_2.5rem] gap-2 items-center"
              >
                <span
                  className="text-xs truncate"
                  style={{ color: speakerColor(s) }}
                >
                  {name}
                </span>
                <div className="relative h-3 rounded bg-wash overflow-hidden">
                  {turns
                    .filter((turn) => turn.speaker === s)
                    .map((turn, i) => {
                      const active =
                        activeMs !== null &&
                        activeMs >= turn.startMs &&
                        activeMs < Math.max(turn.endMs, turn.startMs + 1000);
                      return (
                        <button
                          key={i}
                          type="button"
                          onClick={() => jump(turn.startMs)}
                          aria-label={t("meetings.detail.segment", {
                            name,
                            from: formatClock(turn.startMs),
                            to: formatClock(turn.endMs),
                          })}
                          title={t("meetings.detail.segment", {
                            name,
                            from: formatClock(turn.startMs),
                            to: formatClock(turn.endMs),
                          })}
                          className={`absolute top-0 h-full rounded-sm cursor-pointer ${active ? "ring-2 ring-text" : ""}`}
                          style={{
                            left: `${(turn.startMs / duration) * 100}%`,
                            width: `max(2px, ${((turn.endMs - turn.startMs) / duration) * 100}%)`,
                            background: speakerColor(s),
                          }}
                        />
                      );
                    })}
                </div>
                <span className="text-xs text-ink-3 text-end tabular-nums">
                  {share(s)}
                </span>
              </div>
            );
          })}
          <div className="grid grid-cols-[6rem_1fr_2.5rem] gap-2">
            <span />
            <div className="flex justify-between text-[11px] text-ink-3 tabular-nums">
              <span>0:00</span>
              <span>{formatClock(duration / 3)}</span>
              <span>{formatClock((duration * 2) / 3)}</span>
              <span>{formatClock(duration)}</span>
            </div>
            <span />
          </div>
        </section>
      )}

      <div
        role="tablist"
        aria-label={t("meetings.title")}
        className="flex gap-4 px-1 border-b border-line"
      >
        {(["summary", "transcript"] as const).map((key) => (
          <button
            key={key}
            role="tab"
            type="button"
            aria-selected={tab === key}
            onClick={() => setTab(key)}
            className={`pb-1.5 -mb-px text-sm font-medium border-b-2 cursor-pointer ${tab === key ? "border-background-ui text-text" : "border-transparent text-ink-3 hover:text-text"}`}
          >
            {key === "summary"
              ? t("meetings.detail.summaryTab")
              : t("meetings.detail.transcriptTab")}
          </button>
        ))}
      </div>

      {tab === "summary" ? (
        <div role="tabpanel" className="space-y-4 px-1">
          {busySummarising ? (
            <p className="text-sm text-ink-2 animate-pulse">
              {t("meetings.detail.summarising", {
                engine: engineName(
                  settings?.meeting?.summary_engine ?? "claude_code",
                ),
              })}
            </p>
          ) : meeting.segments.length === 0 ? (
            <p className="text-sm text-ink-2">
              {meeting.status === "ready" && meeting.error === "no_speech"
                ? t("meetings.noSpeech")
                : t("meetings.detail.waitingForTranscript")}
            </p>
          ) : !summary ? (
            <div className="space-y-2">
              {meeting.summary_error ? (
                <p className="text-sm text-error">
                  {summaryError(meeting.summary_error)}
                </p>
              ) : (
                <p className="text-sm text-ink-2">
                  {t("meetings.detail.noSummary")}
                </p>
              )}
              <Button size="sm" onClick={() => summarise(null)}>
                {t("meetings.detail.summarise")}
              </Button>
            </div>
          ) : (
            <>
              {meeting.summary_error && (
                <p className="text-sm text-error">
                  {summaryError(meeting.summary_error)}
                </p>
              )}
              <div className="flex items-start gap-3">
                <p className="flex-1 text-sm leading-relaxed select-text">
                  {summary.overview}
                </p>
                <Button
                  variant="secondary"
                  size="sm"
                  onClick={() =>
                    speaking ? stop() : read(summaryToRead(meeting, t))
                  }
                >
                  <span className="inline-flex items-center gap-1">
                    {speaking ? (
                      <Square className="w-3 h-3 fill-current" />
                    ) : (
                      <Play className="w-3 h-3" />
                    )}
                    {speaking
                      ? t("speech.meeting.stop")
                      : t("speech.meeting.listen")}
                  </span>
                </Button>
              </div>
              {summary.decisions.length > 0 && (
                <section className="space-y-1.5">
                  <h3 className="text-xs font-medium text-ink-3 uppercase tracking-wide">
                    {t("meetings.detail.decisions")}
                  </h3>
                  {summary.decisions.map((d, i) => (
                    <div key={i} className="flex gap-2 text-sm">
                      {d.at_ms != null ? (
                        <TimeButton ms={d.at_ms} onJump={jump} />
                      ) : (
                        <span className="w-10" />
                      )}
                      <span className="select-text">{d.text}</span>
                    </div>
                  ))}
                </section>
              )}
              {summary.action_items.length > 0 && (
                <section className="space-y-1.5">
                  <h3 className="text-xs font-medium text-ink-3 uppercase tracking-wide">
                    {t("meetings.detail.actions", { count: openCount })}
                  </h3>
                  <div className="border border-line rounded-lg divide-y divide-line">
                    {summary.action_items.map((a, i) => {
                      const mine = a.owner === "me";
                      return (
                        <div
                          key={i}
                          className={`flex items-start gap-2.5 px-3 py-2 ${mine ? "bg-logo-primary/10" : ""}`}
                        >
                          <input
                            type="checkbox"
                            checked={a.done}
                            onChange={() =>
                              commands.setMeetingActionDone(
                                meeting.id,
                                i,
                                !a.done,
                              )
                            }
                            aria-label={t("meetings.detail.actionDone", {
                              text: a.text,
                            })}
                            className="mt-1 accent-background-ui"
                          />
                          <div className="flex-1 min-w-0 space-y-0.5">
                            <div
                              className={`text-sm select-text ${a.done ? "line-through text-ink-3" : ""}`}
                            >
                              {a.text}
                            </div>
                            <div className="flex items-center gap-2 flex-wrap text-xs text-ink-3">
                              {a.owner && (
                                <span
                                  className="inline-flex items-center gap-1"
                                  style={{ color: speakerColor(a.owner) }}
                                >
                                  <span
                                    className="w-1.5 h-1.5 rounded-full"
                                    style={{
                                      background: speakerColor(a.owner),
                                    }}
                                    aria-hidden="true"
                                  />
                                  {ownerName(meeting, a.owner, t)}
                                </span>
                              )}
                              {a.due && (
                                <span>
                                  {t("meetings.detail.due", { due: a.due })}
                                </span>
                              )}
                              {mine && (
                                <span className="px-1.5 rounded bg-logo-primary/30 text-text">
                                  {t("meetings.detail.yours")}
                                </span>
                              )}
                              {a.at_ms != null && (
                                <button
                                  type="button"
                                  onClick={() => jump(a.at_ms!)}
                                  className="hover:text-logo-primary cursor-pointer"
                                >
                                  {t("meetings.detail.at", {
                                    time: formatClock(a.at_ms),
                                  })}
                                </button>
                              )}
                            </div>
                          </div>
                        </div>
                      );
                    })}
                  </div>
                </section>
              )}
              {meeting.bookmarks.length > 0 && (
                <section className="space-y-1.5">
                  <h3 className="text-xs font-medium text-ink-3 uppercase tracking-wide">
                    {t("meetings.detail.bookmarks")}
                  </h3>
                  {meeting.bookmarks.map((b, i) => {
                    const label = bookmarkLabel(meeting, b);
                    return (
                      <button
                        key={i}
                        type="button"
                        onClick={() => jump(b.start_ms)}
                        className="flex items-center gap-2 text-sm hover:text-logo-primary cursor-pointer"
                      >
                        <Bookmark className="w-3.5 h-3.5 text-logo-primary" />
                        <span className="tabular-nums text-ink-3">
                          {formatClock(b.start_ms)}–{formatClock(b.end_ms)}
                        </span>
                        {label && <span>{label}</span>}
                      </button>
                    );
                  })}
                </section>
              )}
              <p className="flex items-center gap-1.5 text-xs text-ink-3">
                <Info className="w-3.5 h-3.5" />
                {summary.engine === "claude_code"
                  ? t("meetings.detail.summarisedClaude")
                  : t("meetings.detail.summarisedLocal")}
              </p>
            </>
          )}
        </div>
      ) : (
        <div role="tabpanel" className="space-y-1 px-1">
          {turns.length === 0 && (
            <p className="text-sm text-ink-2">
              {meeting.error === "no_speech"
                ? t("meetings.noSpeech")
                : t("meetings.detail.noTranscript")}
            </p>
          )}
          {items.map((item, i) => {
            if (item.kind === "bookmark") {
              return (
                <div
                  key={`b${i}`}
                  className="flex items-center gap-2 px-2 py-1 rounded-md bg-logo-primary/10 text-xs"
                >
                  <Bookmark className="w-3.5 h-3.5 text-logo-primary" />
                  <span className="font-medium">
                    {t("meetings.detail.bookmark")}
                  </span>
                  <span className="tabular-nums text-ink-3">
                    {formatClock(item.at)}–{formatClock(item.end)}
                  </span>
                  {item.label && <span>{item.label}</span>}
                </div>
              );
            }
            if (item.kind === "dictation") {
              const shown = shownDictation === i;
              return (
                <div
                  key={`d${i}`}
                  className="px-2 py-1 rounded-md bg-wash text-xs space-y-1"
                >
                  <div className="flex items-center gap-2">
                    <Keyboard className="w-3.5 h-3.5 text-ink-3" />
                    <span className="flex-1 text-ink-2">
                      {t("meetings.detail.dictation", {
                        time: formatClock(item.at),
                        seconds: Math.max(
                          1,
                          Math.round((item.end - item.at) / 1000),
                        ),
                      })}
                    </span>
                    {item.text && (
                      <button
                        type="button"
                        onClick={() => setShownDictation(shown ? null : i)}
                        className="font-medium hover:text-logo-primary cursor-pointer"
                      >
                        {shown
                          ? t("meetings.detail.hide")
                          : t("meetings.detail.show")}
                      </button>
                    )}
                  </div>
                  {shown && item.text && (
                    <div className="ps-5 italic text-ink-2 select-text">
                      “{item.text}”
                    </div>
                  )}
                </div>
              );
            }
            const { turn } = item;
            const active =
              activeMs !== null &&
              activeMs >= turn.startMs &&
              (i + 1 >= items.length || activeMs < items[i + 1].at);
            return (
              <div
                key={`t${i}`}
                data-turn-active={active ? "yes" : "no"}
                className={`group grid grid-cols-[3.25rem_1fr_auto] gap-2 px-2 py-1.5 rounded-md ${active ? "bg-logo-primary/20" : ""}`}
              >
                <TimeButton ms={turn.startMs} onJump={jump} />
                <div className="min-w-0">
                  <div
                    className="text-xs font-semibold"
                    style={{ color: speakerColor(turn.speaker) }}
                  >
                    {speakerName(meeting, turn.speaker, t)}
                  </div>
                  <div className="text-sm leading-relaxed select-text">
                    {turn.text}
                  </div>
                </div>
                <button
                  type="button"
                  onClick={() => read(turn.text)}
                  aria-label={t("speech.meeting.listenTurn")}
                  title={t("speech.meeting.listenTurn")}
                  className="self-start mt-4 p-1 rounded-md text-ink-3 opacity-0 group-hover:opacity-100 focus:opacity-100 hover:text-logo-primary cursor-pointer"
                >
                  <Play className="w-3.5 h-3.5" aria-hidden="true" />
                </button>
              </div>
            );
          })}
        </div>
      )}

      <Dialog
        open={confirmDelete}
        onOpenChange={setConfirmDelete}
        closeLabel={t("meetings.close")}
        title={t("meetings.deleteTitle")}
        description={t("meetings.deleteBody")}
        footer={
          <div className="flex justify-end gap-2">
            <Button variant="secondary" onClick={() => setConfirmDelete(false)}>
              {t("meetings.cancel")}
            </Button>
            <Button
              variant="danger"
              onClick={async () => {
                setConfirmDelete(false);
                const result = await commands.deleteMeeting(meeting.id);
                if (result.status === "ok") onBack();
                else toast.error(result.error);
              }}
            >
              {t("meetings.deleteConfirm")}
            </Button>
          </div>
        }
      >
        <span />
      </Dialog>
    </div>
  );
};

const BackLink: React.FC<{ onBack: () => void }> = ({ onBack }) => {
  const { t } = useTranslation();
  return (
    <button
      type="button"
      onClick={onBack}
      className="inline-flex items-center gap-0.5 text-xs font-medium text-ink-3 hover:text-logo-primary cursor-pointer"
    >
      <ChevronLeft className="w-3.5 h-3.5 rtl:rotate-180" />
      {t("meetings.back")}
    </button>
  );
};

const TimeButton: React.FC<{ ms: number; onJump: (ms: number) => void }> = ({
  ms,
  onJump,
}) => {
  const { t } = useTranslation();
  const time = formatClock(ms);
  return (
    <button
      type="button"
      onClick={() => onJump(ms)}
      aria-label={t("meetings.detail.jumpTo", { time })}
      className="w-10 text-start text-xs tabular-nums text-ink-3 hover:text-logo-primary cursor-pointer pt-0.5"
    >
      {time}
    </button>
  );
};
